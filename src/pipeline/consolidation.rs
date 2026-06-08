use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use super::frames::PipelineFrame;
use super::fsm::{PauseReason, PipelineState};
use super::state::PipelineEvents;
use crate::agents::ProactiveEvent;
use crate::db::{Database, Memory};
use crate::i18n;
use crate::llm::{LlmProvider, LlmSession};
use crate::memory::{build_memory_context, extract_memories};
use crate::profile::{ProfileFact, build_profile_context, extract_facts};

/// Returns the routing instructions section for the system prompt.
///
/// This section tells the LLM when to respond directly vs delegate to Hermes,
/// helping avoid hallucinated answers and unnecessary agent delegation.
/// Must be kept under ~500 tokens; written in Spanish.
pub fn build_routing_section() -> &'static str {
    "\n\n## CUÁNDO RESPONDER DIRECTAMENTE VS DELEGAR A HERMES\n\n\
      ✅ Responde DIRECTAMENTE (sin delegar) cuando:\n\
      - La pregunta tiene una respuesta factual breve que puedes dar \
      desde tus conocimientos generales.\n\
      - El usuario pide información sobre el contexto actual de la conversación.\n\
      - Puedes usar tus herramientas nativas para obtener la respuesta \
      rápidamente.\n\
      - Es una tarea de conversación cotidiana (saludos, preguntas simples, \
      traducciones breves, opinión general).\n\n\
      🔄 Delega a Hermes cuando:\n\
      - Necesitas programar, depurar, o modificar código del sistema.\n\
      - Requieres investigación profunda (múltiples consultas), análisis de \
      documentos o flujos de múltiples pasos.\n\
      - Lees documentos grandes (> 1 página) o informes complejos.\n\
      - La tarea necesita acceso a herramientas externas que tú no tienes \
      (calendario, explorador de archivos, bases de datos, \
      gestores de proyectos, agentes especializados).\n\
      - No estás completamente seguro de la respuesta y delegarías \
      a un especialista.\n\n\
      ⚠️ ADVERTENCIA IMPORTANTE:\n\
      Si no estás completamente seguro de una respuesta factual, \
      NO inventes datos. Delega a Hermes. Es mejor delegar una vez de más \
      que dar una respuesta incorrecta. Nunca digas \"según mi conocimiento\" \
      si podrías estar equivocado — delega.\n\n\
      📏 REGLA DE PRECEDENCIA:\n\
      Cuando varias reglas aplican simultáneamente, prioriza la delegación \
      a Hermes si la incertidumbre supera tu confianza en las herramientas nativas.\n\n\
      EJEMPLOS:\n\
      - \"¿Qué hora es?\" → Responde directamente.\n\
      - \"Busca algo rápido en la web\" → Responde directamente (búsqueda puntual). \
      Investigación profunda con múltiples fuentes → Delega a Hermes.\n\
      - \"Refactoriza el módulo de audio para usar async streams\" → \
      Delega a Hermes.\n\
      - \"¿Cuál es la capital de Francia?\" → Responde directamente.\n\
      - \"Analiza el rendimiento del sistema y optimiza los queries lentos\" → \
      Delega a Hermes.\n\
      - \"Traduce 'hello world' al español\" → Responde directamente.\n\
      - \"Lee este documento corto (< 1 página) y resume\" → Responde directamente.\n\
      - \"Investiga las causas de la caída del servidor ayer y genera \
      un reporte\" → Delega a Hermes."
}

/// Character threshold for L1 saturation detection.
///
/// When the combined length of `[USER PROFILE]` + `[MEMORIES]` sections exceeds
/// this value, a `ProactiveEvent::L1Saturated` is emitted (at most once per session).
/// Set to 4000 chars (~1000 tokens), roughly 50% of a modest context window.
const L1_SATURATION_THRESHOLD_CHARS: usize = 4000;

/// Check whether the profile + memories context sections exceed the L1
/// saturation threshold and emit `ProactiveEvent::L1Saturated` if so.
///
/// This is a non-blocking check — it sends the event via `try_send` so the
/// pipeline is not delayed. The `already_notified` flag ensures the event is
/// emitted at most once per session (using `compare_exchange` for thread safety).
pub fn check_system_prompt_saturation(
    profile_facts: &[ProfileFact],
    memories: &[Memory],
    proactive_tx: &mpsc::Sender<ProactiveEvent>,
    already_notified: &AtomicBool,
) {
    let profile_len = build_profile_context(profile_facts).len();
    let memory_len = build_memory_context(memories).len();
    // corrections_len reserved for future use (T12 — immutable rules injection)
    let total_chars = profile_len + memory_len;
    let threshold = L1_SATURATION_THRESHOLD_CHARS;

    if total_chars > threshold
        && already_notified
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let event = ProactiveEvent::L1Saturated {
            total_chars,
            threshold,
        };
        if let Err(e) = proactive_tx.try_send(event) {
            warn!(target: "memory", "Failed to send L1Saturated event: {e}");
        } else {
            info!(
                target: "memory",
                "L1 saturation detected: {total_chars} chars > {threshold} threshold — event emitted"
            );
        }
    }
}

/// Assemble the full system prompt from its components.
///
/// Order: base prompt → tool instructions → [IMMUTABLE RULES] → [USER PROFILE] → [MEMORIES] → [ROUTING] → [AGENTS].
/// Tool instructions are placed immediately after the base prompt so the model sees them
/// early and cannot ignore them, even when the rest of the prompt is very long.
pub fn build_system_prompt(
    base_prompt: &str,
    profile_facts: &[ProfileFact],
    memories: &[Memory],
    agent_section: &str,
    tool_section: &str,
    corrections: &[crate::profile::Correction],
) -> String {
    format!(
        "{}{}{}{}{}{}{}",
        base_prompt,
        tool_section,
        crate::profile::build_corrections_context(corrections),
        build_profile_context(profile_facts),
        build_memory_context(memories),
        build_routing_section(),
        agent_section,
    )
}

/// Core consolidation work: extract profile facts + memories, summarize old
/// turns, rebuild the system prompt, and apply the compacted session.
///
/// Called both by `consolidation_task` (recurring) and at startup when the
/// context already exceeds `LLM_IDLE_MIN_CONTEXT_PCT`.
#[allow(clippy::too_many_arguments)]
pub async fn run_consolidation_cycle(
    background_client: &dyn LlmProvider,
    db: &Database,
    session_id: uuid::Uuid,
    llm_session: &Arc<Mutex<LlmSession>>,
    keep_turns: usize,
    base_prompt: &str,
    agent_section: &str,
    tool_section: &str,
    proactive_tx: &mpsc::Sender<ProactiveEvent>,
    already_notified: &AtomicBool,
) {
    let (conversation_text, summary_prompt, turns_to_summarize) = {
        let s = llm_session.lock().unwrap();
        let count = s.summarizable_turn_count(keep_turns);
        let prompt = s.build_summary_prompt(keep_turns);
        let mut conv = String::new();
        for msg in &s.messages[..count.min(s.messages.len())] {
            if let (Some(role), Some(content)) = (msg["role"].as_str(), msg["content"].as_str())
                && (role == "user" || role == "assistant")
            {
                conv.push_str(role);
                conv.push_str(": ");
                conv.push_str(content);
                conv.push_str("\n\n");
            }
        }
        (conv, prompt, count)
    };

    // Profile facts.
    if !conversation_text.is_empty() {
        let facts = extract_facts(background_client, &conversation_text, "").await;
        for fact in facts {
            if let Err(e) = db
                .upsert_profile_fact(&fact.key, &fact.value, fact.confidence)
                .await
            {
                warn!(target: "profile", "Failed to save profile fact '{}': {}", fact.key, e);
            } else {
                debug!(target: "profile", "Profile: {} = {} ({:.0}%)", fact.key, fact.value, fact.confidence * 100.0);
            }
        }
    }

    // Persistent memories.
    let existing_memories = db.load_active_memories().await.unwrap_or_default();
    let mem_result =
        extract_memories(background_client, &conversation_text, &existing_memories).await;
    for id in &mem_result.archive_ids {
        if let Err(e) = db.deactivate_memory(*id).await {
            warn!(target: "memory", "Failed to archive memory id={}: {}", id, e);
        }
    }
    if !mem_result.new_memories.is_empty() {
        info!(target: "memory", "Extracted {} new memories", mem_result.new_memories.len());
        if let Err(e) = db
            .save_memories_batch(&mem_result.new_memories, session_id)
            .await
        {
            warn!(target: "memory", "Failed to save memories: {}", e);
        }
    }
    if !mem_result.archive_ids.is_empty() {
        info!(target: "memory", "Archived {} outdated memories", mem_result.archive_ids.len());
    }

    // Summarize.
    let summary = if let Some(prompt) = summary_prompt {
        match background_client.complete(&prompt).await {
            Ok(s) if !s.is_empty() => {
                info!(target: "memory", "Summary: {}", s);
                Some(s)
            }
            Ok(_) => {
                warn!(target: "memory", "Summarization returned empty result");
                None
            }
            Err(e) => {
                warn!(target: "memory", "Summarization failed: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Persist summary and rebuild system prompt.
    if let Some(ref summary_text) = summary {
        let prev_through_id = db.get_summary_through_id(session_id).await.unwrap_or(0);
        let through_id = db
            .get_message_id_at_offset(
                session_id,
                prev_through_id,
                turns_to_summarize.saturating_sub(1),
            )
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        if through_id > 0
            && let Err(e) = db.save_summary(session_id, summary_text, through_id).await
        {
            warn!(target: "db", "Failed to persist summary: {}", e);
        }
    }

    let fresh_profile = db.load_user_profile().await.unwrap_or_default();
    let fresh_profile_facts: Vec<ProfileFact> = fresh_profile
        .into_iter()
        .map(|(key, value, confidence)| ProfileFact {
            key,
            value,
            confidence,
        })
        .collect();
    let fresh_memories = db.load_active_memories().await.unwrap_or_default();
    let new_system_prompt = build_system_prompt(
        base_prompt,
        &fresh_profile_facts,
        &fresh_memories,
        agent_section,
        tool_section,
        &[],
    );

    // Emit L1 saturation event if context exceeds the threshold (at most once
    // per session — the `already_notified` flag ensures this).
    check_system_prompt_saturation(
        &fresh_profile_facts,
        &fresh_memories,
        proactive_tx,
        already_notified,
    );

    {
        let mut s = llm_session.lock().unwrap();
        if let Some(ref summary_text) = summary {
            s.apply_summary(summary_text, keep_turns);
        }
        s.set_system_prompt(new_system_prompt);
    }

    info!(
        target: "memory",
        "Consolidation complete — prompt rebuilt ({} profile facts, {} memories, {} recent turns kept)",
        fresh_profile_facts.len(), fresh_memories.len(), keep_turns,
    );
}

/// Context consolidation task: blocks on LLM_POST_FINISHED, runs a full
/// memory consolidation cycle when the context window approaches its limit.
#[allow(clippy::too_many_arguments)]
pub async fn consolidation_task(
    events: Arc<PipelineEvents>,
    pipeline_state_tx: Arc<watch::Sender<PipelineState>>,
    mut pipeline_state_rx: watch::Receiver<PipelineState>,
    transcript_tx: mpsc::Sender<PipelineFrame>,
    llm_session: Arc<Mutex<LlmSession>>,
    background_client: Arc<dyn LlmProvider>,
    db: Database,
    session_id: uuid::Uuid,
    context_tokens: usize,
    keep_turns: usize,
    threshold_pct: usize,
    idle_consolidation_secs: u64,
    idle_min_context_pct: usize,
    base_prompt: String,
    agent_section: String,
    tool_section: String,
    language: String,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    already_notified: Arc<AtomicBool>,
) {
    let mut cancel_rx = events.barge_in_tx.subscribe();
    let mut last_turn_at = Instant::now();

    loop {
        let triggered_by_idle = loop {
            let idle_wait = if idle_consolidation_secs > 0 {
                let elapsed = last_turn_at.elapsed().as_secs();
                let remaining = idle_consolidation_secs.saturating_sub(elapsed);
                Duration::from_secs(remaining.clamp(1, 60))
            } else {
                Duration::from_secs(3600)
            };

            tokio::select! {
                _ = events.llm_post_finished.notified() => {
                    last_turn_at = Instant::now();
                    break false;
                }
                _ = tokio::time::sleep(idle_wait) => {
                    let elapsed = last_turn_at.elapsed().as_secs();
                    if idle_consolidation_secs > 0
                        && elapsed >= idle_consolidation_secs
                        && !pipeline_state_rx.borrow().is_busy()
                    {
                        break true;
                    }
                }
                _ = cancel_rx.recv() => {}
            }
        };

        let (needs, approx_tokens, current_pct, msg_count, effective_threshold) = {
            let s = llm_session.lock().unwrap();
            let approx = s.approx_tokens();
            let pct = (approx * 100).checked_div(context_tokens).unwrap_or(0);
            let effective = if triggered_by_idle {
                idle_min_context_pct
            } else {
                threshold_pct
            };
            let needs = s.needs_consolidation(context_tokens, effective);
            (needs, approx, pct, s.messages.len(), effective)
        };
        info!(
            target: "memory",
            "Context check ({}): ~{} tokens / {} max ({}%) — threshold {}% — {} msgs — consolidation {}",
            if triggered_by_idle { "idle" } else { "post-turn" },
            approx_tokens, context_tokens, current_pct, effective_threshold,
            msg_count,
            if needs { "TRIGGERED" } else { "not needed" },
        );
        if !needs {
            while cancel_rx.try_recv().is_ok() {}
            if triggered_by_idle {
                last_turn_at = Instant::now();
            }
            continue;
        }

        if !triggered_by_idle {
            info!(target: "memory", "Context limit approaching — starting announced consolidation");

            // Wait for LLM to finish its current turn before interrupting.
            loop {
                if !pipeline_state_rx.borrow().is_busy() {
                    break;
                }
                pipeline_state_rx.changed().await.ok();
            }
            transcript_tx
                .send(PipelineFrame::SystemNotification {
                    text: i18n::get_notification("reorganize_memory", &language).to_string(),
                })
                .await
                .ok();

            loop {
                tokio::select! {
                    _ = events.llm_post_finished.notified() => { break; }
                    _ = cancel_rx.recv() => {}
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = pipeline_state_tx.send(PipelineState::Paused {
                reason: PauseReason::Consolidation,
            });
            info!(target: "memory", "Pipeline paused — running consolidation...");
        } else {
            info!(target: "memory", "Idle timer — running silent consolidation...");
        }

        run_consolidation_cycle(
            background_client.as_ref(),
            &db,
            session_id,
            &llm_session,
            keep_turns,
            &base_prompt,
            &agent_section,
            &tool_section,
            &proactive_tx,
            &already_notified,
        )
        .await;

        if !triggered_by_idle {
            let _ = pipeline_state_tx.send(PipelineState::Idle);
            let now = chrono::Local::now().format("%H:%M").to_string();
            transcript_tx
                .send(PipelineFrame::SystemNotification {
                    text: i18n::get_notification("memory_reorganized", &language)
                        .replace("{now}", &now)
                        .to_string(),
                })
                .await
                .ok();
            info!(target: "memory", "Consolidation cycle finished — pipeline resumed");
        }

        last_turn_at = Instant::now();
        while cancel_rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;

    use crate::db::Memory;
    use crate::profile::ProfileFact;

    /// Helper: build a profile fact with a long value.
    fn long_fact(value: &str) -> ProfileFact {
        ProfileFact {
            key: "test_key".into(),
            value: value.to_string(),
            confidence: 0.9,
        }
    }

    /// Helper: build a memory with long content.
    fn long_memory(content: &str) -> Memory {
        Memory {
            id: 1,
            content: content.to_string(),
            category: "general".into(),
            source_session_id: None,
            created_at: "".into(),
            updated_at: "".into(),
        }
    }

    #[tokio::test]
    async fn test_saturation_triggers_when_over_threshold() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let notified = AtomicBool::new(false);

        // Build enough data to exceed L1_SATURATION_THRESHOLD_CHARS (4000).
        let facts = vec![long_fact(&"x".repeat(3000))]; // profile ~3000 chars
        let mems = vec![long_memory(&"y".repeat(2000))]; // memories ~2000 chars
        // total ~5000 > 4000

        check_system_prompt_saturation(&facts, &mems, &tx, &notified);

        // Flush — try_send is synchronous so the event should be in the channel.
        let received = rx.try_recv().expect("Expected L1Saturated event");
        match received {
            ProactiveEvent::L1Saturated {
                total_chars,
                threshold,
            } => {
                assert!(
                    total_chars > threshold,
                    "total_chars={total_chars} should exceed threshold={threshold}"
                );
                assert_eq!(threshold, L1_SATURATION_THRESHOLD_CHARS);
            }
            other => panic!("Expected L1Saturated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_saturation_no_event_when_below_threshold() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let notified = AtomicBool::new(false);

        let facts = vec![long_fact("short")];
        let mems = vec![long_memory("tiny")];

        check_system_prompt_saturation(&facts, &mems, &tx, &notified);

        // Channel should be empty — nothing sent.
        assert!(
            rx.try_recv().is_err(),
            "No event should be emitted when below threshold"
        );
    }

    #[tokio::test]
    async fn test_saturation_deduplication() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);
        let notified = AtomicBool::new(false);

        let facts = vec![long_fact(&"x".repeat(3000))];
        let mems = vec![long_memory(&"y".repeat(2000))];

        // First call: should emit.
        check_system_prompt_saturation(&facts, &mems, &tx, &notified);
        assert!(rx.try_recv().is_ok(), "First call should emit an event");

        // Second call: `notified` is now true, should NOT emit.
        check_system_prompt_saturation(&facts, &mems, &tx, &notified);
        assert!(
            rx.try_recv().is_err(),
            "Second call must NOT emit — deduplication failed"
        );
    }

    /// Verify the compare_exchange transition: false → true.
    #[test]
    fn test_atomic_bool_state_transition() {
        let notified = AtomicBool::new(false);

        // First check: false → true succeeds.
        assert!(
            notified
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        );
        assert!(notified.load(Ordering::SeqCst));

        // Second check: false → true fails because already true.
        assert!(
            notified
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        );
        assert!(notified.load(Ordering::SeqCst));
    }

    #[test]
    fn build_system_prompt_order_is_correct() {
        let base = "Eres un asistente.";
        let tools = "[TOOLS]";
        let agents = "[AGENTS]";
        let facts = vec![ProfileFact {
            key: "name".into(),
            value: "Daniel".into(),
            confidence: 0.9,
        }];
        let mems = vec![Memory {
            id: 1,
            content: "Recuerda el café".into(),
            category: "general".into(),
            source_session_id: None,
            created_at: "".into(),
            updated_at: "".into(),
        }];
        let corrections = vec![crate::profile::Correction {
            topic: "color".into(),
            correction_text: "azul".into(),
            confidence: 1.0,
        }];

        let prompt = build_system_prompt(base, &facts, &mems, agents, tools, &corrections);

        let pos_base = prompt.find(base).unwrap();
        let pos_tools = prompt.find(tools).unwrap();
        let pos_immutable = prompt.find("[IMMUTABLE RULES]").unwrap();
        let pos_profile = prompt.find("[USER PROFILE]").unwrap();
        let pos_memories = prompt.find("[MEMORIES]").unwrap();
        let pos_routing = prompt.find("CUÁNDO RESPONDER").unwrap();
        let pos_agents = prompt.find(agents).unwrap();

        assert!(
            pos_base < pos_tools
                && pos_tools < pos_immutable
                && pos_immutable < pos_profile
                && pos_profile < pos_memories
                && pos_memories < pos_routing
                && pos_routing < pos_agents,
            "Prompt sections must appear in the correct order"
        );
    }

    #[test]
    fn build_system_prompt_includes_corrections_when_present() {
        let corrections = vec![crate::profile::Correction {
            topic: "name".into(),
            correction_text: "Daniel".into(),
            confidence: 1.0,
        }];
        let prompt = build_system_prompt("base", &[], &[], "", "", &corrections);
        assert!(prompt.contains("[IMMUTABLE RULES]"));
        assert!(prompt.contains("name -> Daniel"));
    }

    #[test]
    fn build_system_prompt_omits_corrections_when_empty() {
        let prompt = build_system_prompt("base", &[], &[], "", "", &[]);
        assert!(!prompt.contains("[IMMUTABLE RULES]"));
    }

    #[test]
    fn build_system_prompt_omits_profile_when_no_facts() {
        let prompt = build_system_prompt("base", &[], &[], "", "", &[]);
        assert!(!prompt.contains("[USER PROFILE]"));
    }

    #[test]
    fn build_system_prompt_omits_memories_when_empty() {
        let prompt = build_system_prompt("base", &[], &[], "", "", &[]);
        assert!(!prompt.contains("[MEMORIES]"));
    }
}
