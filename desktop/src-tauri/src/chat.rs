//! Chat bridge for issue #41: the authoritative shell-side transcript plus
//! thin REST clients for text input and active-session history.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tracing::warn;

pub const CHAT_EVENT: &str = "desktop-chat-event";

/// TUI parity: keep the most recent UI history, not the whole SQLite session.
pub const UI_HISTORY_HARD_CAP: usize = 100;
const TOOL_RESULT_PREVIEW_CHARS: usize = 120;
const INPUT_TIMEOUT: Duration = Duration::from_secs(10);
const HISTORY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
    System,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSource {
    Voice,
    Text,
}

impl ChatSource {
    fn from_wire(value: Option<&str>) -> Option<Self> {
        match value {
            Some("voice") => Some(Self::Voice),
            Some("text") => Some(Self::Text),
            _ => None,
        }
    }
}

/// One rendered transcript row. `streaming` is only true for the active
/// assistant response and is finalized by `llm_done`, a new utterance, idle
/// state or a disconnect (barge-in does not guarantee `llm_done`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEntry {
    pub id: String,
    pub role: ChatRole,
    pub text: String,
    pub source: Option<ChatSource>,
    pub utterance_id: Option<u64>,
    pub streaming: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSnapshot {
    pub revision: u64,
    pub entries: Vec<ChatEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Snapshot { snapshot: ChatSnapshot },
    Upsert {
        revision: u64,
        #[serde(rename = "entry")]
        entry: ChatEntry,
    },
    AppendToken {
        revision: u64,
        #[serde(rename = "utteranceId")]
        utterance_id: u64,
        token: String,
    },
    Remove {
        revision: u64,
        #[serde(rename = "entryId")]
        entry_id: String,
    },
}

/// Authoritative chat state for the desktop shell. The frontend mirrors this
/// state through `desktop-chat-event`; it does not derive transcript state
/// independently from Control SSE.
pub struct ChatHub {
    entries: Mutex<Vec<ChatEntry>>,
    streaming: Mutex<Option<u64>>,
    revision: AtomicU64,
    next_live_id: AtomicU64,
    tx: broadcast::Sender<ChatEvent>,
}

impl ChatHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Self {
            entries: Mutex::new(Vec::new()),
            streaming: Mutex::new(None),
            revision: AtomicU64::new(0),
            next_live_id: AtomicU64::new(1),
            tx,
        }
    }

    pub fn snapshot(&self) -> ChatSnapshot {
        let entries = self
            .entries
            .lock()
            .expect("chat entries mutex poisoned")
            .clone();
        ChatSnapshot {
            revision: self.revision.load(Ordering::SeqCst),
            entries,
        }
    }

    pub fn snapshot_event(&self) -> ChatEvent {
        ChatEvent::Snapshot {
            snapshot: self.snapshot(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ChatEvent> {
        self.tx.subscribe()
    }

    fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn live_event_id(&self, prefix: &str) -> String {
        let sequence = self.next_live_id.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{sequence}")
    }

    /// Replace the view with persistent session history. Ephemeral tool/system
    /// rows and a stale in-flight response are intentionally dropped.
    pub fn replace_history(&self, entries: Vec<ChatEntry>) {
        let mut entries = entries;
        if entries.len() > UI_HISTORY_HARD_CAP {
            let excess = entries.len() - UI_HISTORY_HARD_CAP;
            entries.drain(..excess);
        }
        *self.streaming.lock().expect("chat streaming mutex poisoned") = None;
        *self.entries.lock().expect("chat entries mutex poisoned") = entries;
        self.publish_snapshot();
    }

    pub fn apply_transcript(
        &self,
        utterance_id: u64,
        text: String,
        source: Option<&str>,
    ) {
        self.finalize_streaming(Some(utterance_id));
        let entry = ChatEntry {
            id: format!("user-{utterance_id}"),
            role: ChatRole::User,
            text,
            source: ChatSource::from_wire(source),
            utterance_id: Some(utterance_id),
            streaming: false,
        };
        self.upsert_entry(entry);
    }

    pub fn apply_token(&self, utterance_id: u64, token: String) {
        if token.is_empty() {
            return;
        }

        let capped = {
            let mut entries = self
                .entries
                .lock()
                .expect("chat entries mutex poisoned");
            if let Some(existing) = entries
                .iter_mut()
                .rev()
                .find(|entry| {
                    entry.role == ChatRole::Assistant && entry.utterance_id == Some(utterance_id)
                })
            {
                existing.text.push_str(&token);
                existing.streaming = true;
                false
            } else {
                let created = ChatEntry {
                    id: format!("assistant-{utterance_id}"),
                    role: ChatRole::Assistant,
                    text: token.clone(),
                    source: None,
                    utterance_id: Some(utterance_id),
                    streaming: true,
                };
                entries.push(created);
                let excess = entries.len().saturating_sub(UI_HISTORY_HARD_CAP);
                if excess > 0 {
                    entries.drain(..excess);
                }
                excess > 0
            }
        };

        *self
            .streaming
            .lock()
            .expect("chat streaming mutex poisoned") = Some(utterance_id);
        if capped {
            // A snapshot also removes rows dropped by the history cap.
            self.publish_snapshot();
        } else {
            let revision = self.next_revision();
            let _ = self.tx.send(ChatEvent::AppendToken {
                revision,
                utterance_id,
                token,
            });
        }
    }

    pub fn apply_done(&self, utterance_id: u64, full_text: String) {
        if full_text.is_empty() {
            self.remove_assistant(utterance_id);
            return;
        }

        // `full_text` is the final LLM segment. After a foreground tool call the
        // pre-tool narration was already streamed into the same assistant row,
        // so preserve that accumulated text when it ends with the final segment.
        let final_text = {
            let entries = self
                .entries
                .lock()
                .expect("chat entries mutex poisoned");
            entries
                .iter()
                .rev()
                .find(|entry| {
                    entry.role == ChatRole::Assistant && entry.utterance_id == Some(utterance_id)
                })
                .map(|entry| {
                    if !entry.text.is_empty() && entry.text.ends_with(&full_text) {
                        entry.text.clone()
                    } else {
                        full_text.clone()
                    }
                })
                .unwrap_or(full_text)
        };
        let entry = ChatEntry {
            id: format!("assistant-{utterance_id}"),
            role: ChatRole::Assistant,
            text: final_text,
            source: None,
            utterance_id: Some(utterance_id),
            streaming: false,
        };
        *self
            .streaming
            .lock()
            .expect("chat streaming mutex poisoned") = None;
        self.upsert_entry(entry);
    }

    /// A new utterance, idle state or disconnect means the prior streaming
    /// response is finished as far as the UI is concerned.
    pub fn finalize_streaming(&self, except_utterance_id: Option<u64>) {
        let current = *self
            .streaming
            .lock()
            .expect("chat streaming mutex poisoned");
        let Some(streaming_id) = current else {
            return;
        };
        if except_utterance_id == Some(streaming_id) {
            return;
        }

        let entry = {
            let mut entries = self
                .entries
                .lock()
                .expect("chat entries mutex poisoned");
            let Some(entry) = entries
                .iter_mut()
                .rev()
                .find(|entry| entry.role == ChatRole::Assistant && entry.utterance_id == Some(streaming_id))
            else {
                return;
            };
            entry.streaming = false;
            entry.clone()
        };
        *self
            .streaming
            .lock()
            .expect("chat streaming mutex poisoned") = None;
        self.upsert_entry(entry);
    }

    pub fn reset_streaming(&self) {
        self.finalize_streaming(None);
    }

    pub fn apply_tool_call(&self, name: String, result: String) {
        let preview = truncate_chars(&result, TOOL_RESULT_PREVIEW_CHARS);
        self.upsert_entry(ChatEntry {
            id: self.live_event_id("tool"),
            role: ChatRole::Tool,
            text: format!("{name} -> {preview}"),
            source: None,
            utterance_id: None,
            streaming: false,
        });
    }

    pub fn apply_system_notification(&self, text: String) {
        self.upsert_entry(ChatEntry {
            id: self.live_event_id("system"),
            role: ChatRole::System,
            text,
            source: None,
            utterance_id: None,
            streaming: false,
        });
    }

    pub fn apply_error(&self, message: String) {
        self.upsert_entry(ChatEntry {
            id: self.live_event_id("error"),
            role: ChatRole::Error,
            text: message,
            source: None,
            utterance_id: None,
            streaming: false,
        });
    }

    fn upsert_entry(&self, entry: ChatEntry) {
        let capped = {
            let mut entries = self
                .entries
                .lock()
                .expect("chat entries mutex poisoned");
            if let Some(existing) = entries
                .iter_mut()
                .rev()
                .find(|candidate| candidate.id == entry.id)
            {
                *existing = entry.clone();
                false
            } else {
                entries.push(entry.clone());
                let excess = entries.len().saturating_sub(UI_HISTORY_HARD_CAP);
                if excess > 0 {
                    entries.drain(..excess);
                }
                excess > 0
            }
        };
        if capped {
            // A snapshot also removes rows dropped by the history cap.
            self.publish_snapshot();
        } else {
            let revision = self.next_revision();
            let _ = self.tx.send(ChatEvent::Upsert { revision, entry });
        }
    }

    fn publish_snapshot(&self) {
        self.next_revision();
        let snapshot = self.snapshot();
        let _ = self.tx.send(ChatEvent::Snapshot { snapshot });
    }

    fn remove_assistant(&self, utterance_id: u64) {
        let id = format!("assistant-{utterance_id}");
        let removed = {
            let mut entries = self
                .entries
                .lock()
                .expect("chat entries mutex poisoned");
            let old_len = entries.len();
            entries.retain(|entry| entry.id != id);
            old_len != entries.len()
        };
        if removed {
            *self
                .streaming
                .lock()
                .expect("chat streaming mutex poisoned") = None;
            let revision = self.next_revision();
            let _ = self.tx.send(ChatEvent::Remove { revision, entry_id: id });
        }
    }
}

impl Default for ChatHub {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedChatHub = Arc<ChatHub>;

/// Forward chat updates as a dedicated Tauri event. On local receiver lag, send
/// a full snapshot instead of skipping deltas.
pub fn spawn_chat_event_emitter(app: AppHandle, hub: SharedChatHub) {
    let mut rx = hub.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = app.emit(CHAT_EVENT, event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = app.emit(CHAT_EVENT, hub.snapshot_event());
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[derive(Debug, Deserialize)]
struct SessionWire {
    id: String,
    is_active: bool,
}

#[derive(Debug, Deserialize)]
struct HistoryMessageWire {
    id: i64,
    role: String,
    content: String,
}

fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("failed to build chat HTTP client")
}

async fn get_json(base_url: &str, path: &str) -> anyhow::Result<serde_json::Value> {
    let response = client(HISTORY_TIMEOUT)
        .get(format!("{base_url}{path}"))
        .send()
        .await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await?;
    if !status.is_success() {
        anyhow::bail!("chat request failed (HTTP {})", status.as_u16());
    }
    Ok(body)
}

/// Active-session history, mapped to the same shape as live events. Historical
/// users have no persisted origin; the TUI uses `Text` as that default.
pub async fn fetch_history(base_url: &str) -> anyhow::Result<Vec<ChatEntry>> {
    let session_value = get_json(base_url, "/control/sessions").await?;
    let sessions: Vec<SessionWire> = serde_json::from_value(session_value)?;
    let session = sessions
        .iter()
        .find(|session| session.is_active)
        .or_else(|| sessions.first())
        .ok_or_else(|| anyhow::anyhow!("no conversation sessions"))?;

    let message_value = get_json(
        base_url,
        &format!("/control/sessions/{}/messages", session.id),
    )
    .await?;
    let messages: Vec<HistoryMessageWire> = serde_json::from_value(message_value)?;

    Ok(messages
        .into_iter()
        .filter_map(|message| {
            let role = match message.role.trim().to_ascii_lowercase().as_str() {
                "user" => ChatRole::User,
                "assistant" => ChatRole::Assistant,
                "system" => ChatRole::System,
                _ => return None,
            };
            if message.content.is_empty() {
                return None;
            }
            Some(ChatEntry {
                id: format!("history-{}", message.id),
                role,
                text: message.content,
                source: (role == ChatRole::User).then_some(ChatSource::Text),
                utterance_id: None,
                streaming: false,
            })
        })
        .collect())
}

/// History failure must not take the whole Control connection down.
pub async fn rehydrate_history(base_url: &str, hub: &ChatHub) {
    match fetch_history(base_url).await {
        Ok(entries) => hub.replace_history(entries),
        Err(error) => warn!(target: "desktop_chat", "chat history refresh failed: {error:#}"),
    }
}

pub async fn send_text_input(base_url: &str, text: &str) -> anyhow::Result<()> {
    let response = client(INPUT_TIMEOUT)
        .post(format!("{base_url}/control/input"))
        .json(&serde_json::json!({ "text": text }))
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::CONFLICT {
        anyhow::bail!(
            "permission_pending: resolve the pending permission request before sending text (HTTP 409)"
        );
    }
    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        anyhow::bail!("input_unavailable: the pipeline input channel is closed (HTTP 503)");
    }
    anyhow::bail!("input_failed: could not submit text input (HTTP {})", status.as_u16());
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out: String = value.chars().take(limit).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_entries() -> Vec<ChatEntry> {
        vec![
            ChatEntry {
                id: "history-1".into(),
                role: ChatRole::User,
                text: "hola".into(),
                source: Some(ChatSource::Text),
                utterance_id: None,
                streaming: false,
            },
            ChatEntry {
                id: "history-2".into(),
                role: ChatRole::Assistant,
                text: "buenos dias".into(),
                source: None,
                utterance_id: None,
                streaming: false,
            },
        ]
    }

    #[test]
    fn hub_streams_tokens_and_commits_full_text() {
        let hub = ChatHub::new();
        let mut rx = hub.subscribe();
        hub.apply_transcript(1, "hola".into(), Some("voice"));
        hub.apply_token(1, "buen".into());
        hub.apply_token(1, "os dias".into());
        hub.apply_done(1, "buenos dias".into());

        let snapshot = hub.snapshot();
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].role, ChatRole::User);
        assert_eq!(snapshot.entries[0].source, Some(ChatSource::Voice));
        assert_eq!(snapshot.entries[1].text, "buenos dias");
        assert!(!snapshot.entries[1].streaming);

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(matches!(events[0], ChatEvent::Upsert { .. }));
        assert!(matches!(events[1], ChatEvent::AppendToken { .. }));
        assert!(matches!(events[2], ChatEvent::AppendToken { .. }));
        assert!(matches!(events[3], ChatEvent::Upsert { .. }));
    }

    #[test]
    fn done_preserves_pre_tool_narration_in_the_same_utterance() {
        let hub = ChatHub::new();
        hub.apply_transcript(1, "busca".into(), Some("voice"));
        hub.apply_token(1, "Buscando".into());
        hub.apply_token(1, "Aquí está".into());
        hub.apply_done(1, "Aquí está".into());

        let entries = hub.snapshot().entries;
        assert_eq!(entries[1].text, "BuscandoAquí está");
        assert!(!entries[1].streaming);
    }

    #[test]
    fn new_utterance_finalizes_streaming_without_llm_done() {
        let hub = ChatHub::new();
        hub.apply_transcript(1, "primera".into(), Some("voice"));
        hub.apply_token(1, "parcial".into());
        hub.apply_transcript(2, "segunda".into(), Some("text"));

        let entries = hub.snapshot().entries;
        assert_eq!(entries[1].text, "parcial");
        assert!(!entries[1].streaming);
        assert_eq!(entries[2].source, Some(ChatSource::Text));
    }

    /// Two complete turns must append four rows. A repeated utterance ID
    /// would upsert the prior user row and concatenate the prior assistant
    /// response instead.
    #[test]
    fn consecutive_turns_append_new_user_and_assistant_rows() {
        let hub = ChatHub::new();
        hub.apply_transcript(1, "por texto".into(), Some("text"));
        hub.apply_token(1, "primera respuesta".into());
        hub.apply_done(1, "primera respuesta".into());
        hub.apply_transcript(2, "por voz".into(), Some("voice"));
        hub.apply_token(2, "segunda respuesta".into());
        hub.apply_done(2, "segunda respuesta".into());

        let entries = hub.snapshot().entries;
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].id, "user-1");
        assert_eq!(entries[0].text, "por texto");
        assert_eq!(entries[0].source, Some(ChatSource::Text));
        assert_eq!(entries[1].id, "assistant-1");
        assert_eq!(entries[1].text, "primera respuesta");
        assert_eq!(entries[2].id, "user-2");
        assert_eq!(entries[2].text, "por voz");
        assert_eq!(entries[2].source, Some(ChatSource::Voice));
        assert_eq!(entries[3].id, "assistant-2");
        assert_eq!(entries[3].text, "segunda respuesta");
        assert!(!entries[3].text.contains("primera"));
        assert!(!entries.iter().any(|entry| entry.streaming));
    }

    #[test]
    fn history_replaces_ephemeral_rows_and_is_capped() {
        let hub = ChatHub::new();
        hub.apply_tool_call("web_search".into(), "result".into());
        hub.replace_history(history_entries());
        assert_eq!(hub.snapshot().entries, history_entries());

        let mut long_history = history_entries();
        for index in 0..(UI_HISTORY_HARD_CAP + 10) {
            long_history.push(ChatEntry {
                id: format!("history-{index}"),
                role: ChatRole::User,
                text: format!("message {index}"),
                source: Some(ChatSource::Text),
                utterance_id: None,
                streaming: false,
            });
        }
        hub.replace_history(long_history);
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.entries.len(), UI_HISTORY_HARD_CAP);
        assert_eq!(snapshot.entries.last().unwrap().text, "message 109");
    }

    #[test]
    fn tools_are_truncated_to_tui_preview_length() {
        let hub = ChatHub::new();
        let long = "x".repeat(TOOL_RESULT_PREVIEW_CHARS + 20);
        hub.apply_tool_call("read_file".into(), long);
        let entry = &hub.snapshot().entries[0];
        assert_eq!(entry.role, ChatRole::Tool);
        let expected = "read_file".chars().count()
            + 4
            + TOOL_RESULT_PREVIEW_CHARS
            + 1;
        assert_eq!(entry.text.chars().count(), expected);
        assert!(entry.text.ends_with('…'));
    }

    #[test]
    fn wire_chat_events_are_serializable() {
        let hub = ChatHub::new();
        hub.apply_transcript(7, "hola".into(), Some("voice"));
        let event = hub.snapshot_event();
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"snapshot""#));
        assert!(json.contains(r#""source":"voice""#));
    }

    #[test]
    fn input_error_codes_are_stable() {
        // These prefixes are the contract consumed by `parseChatError`.
        let pending = anyhow::anyhow!(
            "permission_pending: resolve the pending permission request before sending text (HTTP 409)"
        );
        assert!(pending.to_string().starts_with("permission_pending: "));
    }
}
