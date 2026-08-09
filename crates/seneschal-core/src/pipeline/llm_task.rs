use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use super::frames::PipelineFrame;
use super::fsm::PipelineState;
use super::state::PipelineEvents;
use crate::llm::{LlmProvider, LlmSession, RequestOptions, StreamToken};
use seneschal_common::ControlBroadcast;
use seneschal_common::ControlEvent;
use seneschal_common::db::Database;
use seneschal_common::events::ProactiveEvent;
use seneschal_common::tools::ToolRegistry;
use seneschal_common::tui_events::{InputSource, TuiEvent, TuiEventTx};

/// Monotonically increasing counter for tagging each pipeline run with a unique ID.
static PIPELINE_RUN_ID: AtomicU64 = AtomicU64::new(0);

/// Maximum number of sequential tool calls allowed per user turn.
pub const MAX_TOOL_ITERATIONS: usize = 5;

/// LLM task: receives transcript frames, runs the LLM+tools pipeline, fires events.
#[allow(clippy::too_many_arguments)]
pub async fn llm_task(
    events: Arc<PipelineEvents>,
    pipeline_state_tx: Arc<watch::Sender<PipelineState>>,
    mut pipeline_state_rx: watch::Receiver<PipelineState>,
    sentences_tx: mpsc::Sender<PipelineFrame>,
    llm_tx: mpsc::Sender<PipelineFrame>,
    mut transcript_rx: mpsc::Receiver<PipelineFrame>,
    t_llm_post_send: Arc<Mutex<Option<Instant>>>,
    llm_session: Arc<Mutex<LlmSession>>,
    llm_client: Arc<dyn LlmProvider>,
    db: Database,
    session_id: uuid::Uuid,
    tools: Arc<std::sync::Mutex<ToolRegistry>>,
    turn_commit_counter: Arc<AtomicU64>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    filler_controller: Arc<crate::audio::filler::FillerController>,
    llm_temperature: f32,
    llm_thinking: bool,
    tui_tx: Option<TuiEventTx>,
    // Optional Control/SSE bus. When Some, live transcript/tokens/tools are published
    // for companions and dashboards. Always typed in common (no feature gate).
    control_broadcast: Option<ControlBroadcast>,
) {
    let pipeline_id = PIPELINE_RUN_ID.fetch_add(1, Ordering::SeqCst);
    let mut cancel_rx = events.barge_in_tx.subscribe();
    // Original content of the first transcript of a pending user turn, captured
    // at user-commit for SQLite reconciliation on assistant-commit. Persists
    // across loop iterations so repeated barge-ins still resolve to the same
    // original SQLite row.
    let mut pending_user_original: Option<String> = None;

    loop {
        // Block until a transcript frame arrives; ignore cancels while idle.
        let frame = loop {
            tokio::select! {
                frame = transcript_rx.recv() => {
                    match frame {
                        Some(f) => break f,
                        None => return, // channel closed — exit
                    }
                }
                _ = cancel_rx.recv() => {}
            }
        };

        // Decode the incoming frame into (text, tool_continuation, is_text_input, is_system_notification).
        let (text, tool_continuation, is_text_input, is_system_notification) = match frame {
            PipelineFrame::TranscriptReady { text, .. } => (text, false, false, false),
            PipelineFrame::TextInput { text } => (text, false, true, false),
            PipelineFrame::SystemNotification { text } => (text, false, false, true),
            PipelineFrame::AgentResult {
                tool_call_id: Some(_),
                ..
            } => (String::new(), true, false, false),
            _ => continue, // unexpected frame type — wait for next
        };

        // Wait for consolidation to finish before starting a new turn.
        loop {
            if !matches!(*pipeline_state_rx.borrow(), PipelineState::Paused { .. }) {
                break;
            }
            pipeline_state_rx.changed().await.ok();
        }

        let _ = pipeline_state_tx.send(PipelineState::Thinking {
            utterance_id: pipeline_id,
        });

        if tool_continuation {
            info!(target: "pipeline", "[pipe={}] Tool result delivered — continuing turn", pipeline_id);
        } else if is_system_notification {
            info!(target: "pipeline", "[pipe={}] SystemNotification: {}", pipeline_id, text);
        } else {
            info!(target: "pipeline", "[pipe={}] User: {}", pipeline_id, text);
        }

        if let Some(ref tx) = tui_tx
            && !tool_continuation
            && !is_system_notification
        {
            let source = if is_text_input {
                InputSource::Text
            } else {
                InputSource::Voice
            };
            tx.send(TuiEvent::UserMessage {
                text: text.clone(),
                source,
            })
            .ok();
            tx.send(TuiEvent::StateChange(
                seneschal_common::tui_events::PipelineState::Thinking,
            ))
            .ok();
        }
        if !tool_continuation && !is_system_notification {
            if let Some(ref ctrl) = control_broadcast {
                ctrl.send(ControlEvent::Transcript {
                    utterance_id: pipeline_id,
                    text: text.clone(),
                });
            }
        }

        if is_system_notification {
            if let Some(ref tx) = tui_tx {
                tx.send(TuiEvent::SystemNotification { text: text.clone() })
                    .ok();
            }
            if let Some(ref ctrl) = control_broadcast {
                ctrl.send(ControlEvent::SystemNotification { text: text.clone() });
            }
            {
                let mut s = llm_session.lock().unwrap();
                s.add_internal_notification(&text);
            }
            turn_commit_counter.fetch_add(1, Ordering::SeqCst);
        } else if !tool_continuation {
            let appended = {
                let mut s = llm_session.lock().unwrap();
                // Snapshot the original user content (if a previous user turn is
                // pending) for SQLite reconciliation on assistant-commit. We do
                // this BEFORE mutation so we get the pre-append text.
                if s.is_user_message_pending
                    && let Some(last) = s.messages.last()
                    && last["role"].as_str() == Some("user")
                {
                    pending_user_original = last["content"].as_str().map(str::to_string);
                }
                let result = s.update_last_user_turn(&text);
                if !result {
                    s.add_user_turn(&text);
                }
                turn_commit_counter.fetch_add(1, Ordering::SeqCst);
                result
            };
            if !appended {
                // Only the FIRST transcript of a pending user turn is persisted
                // immediately; subsequent barge-in appends are reconciled in
                // SQLite on assistant-commit.
                let db_c = db.clone();
                let text_c = text.clone();
                tokio::spawn(async move {
                    if let Err(e) = db_c.save_message(session_id, "User", &text_c).await {
                        warn!(target: "db", "Failed to save User message: {}", e);
                    }
                });
            }
        } else {
            turn_commit_counter.fetch_add(1, Ordering::SeqCst);
        }

        // (assistant_text / llm_post_finished flags removed; channel carries this now)

        let tool_defs = tools.lock().unwrap().tool_definitions();
        info!(
            target: "pipeline",
            "LLM request: {} tool(s) available: {:?}",
            tool_defs.len(),
            tool_defs
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
                .collect::<Vec<_>>()
        );
        let mut messages = llm_session.lock().unwrap().all_messages_api();
        let base_msg_len = messages.len();
        let mut final_response = String::new();
        let mut committed = false;
        let mut cancelled = false;
        let mut first_token_logged = false;

        'pipeline: {
            'tool_loop: for iter in 0..MAX_TOOL_ITERATIONS {
                let request_options = RequestOptions::new()
                    .with_temperature(llm_temperature)
                    .with_thinking(llm_thinking);

                info!(target: "performance", "LLM request [pipe={}]", pipeline_id);

                let mut llm_text = String::new();
                let mut tool_call: Option<(String, String)> = None;

                // Always send the full tool schema for KV-cache stability (#191).
                let active_tools: Vec<serde_json::Value> = tool_defs.clone();
                let (token_rx, stream_handle) = match llm_client
                    .stream(&messages, &active_tools, request_options)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        error!(target: "llm", "LLM error: {}", e);
                        if let Some(ref tx) = tui_tx {
                            tx.send(TuiEvent::Error(format!("LLM error: {e}"))).ok();
                        }
                        let _ = sentences_tx
                            .send(super::frames::PipelineFrame::SentenceReady {
                                utterance_id: pipeline_id,
                                sentence: "Lo siento, no pude conectar con el modelo de lenguaje."
                                    .to_string(),
                            })
                            .await;
                        let _ = sentences_tx
                            .send(super::frames::PipelineFrame::LLMResponseDone {
                                utterance_id: pipeline_id,
                                full_text: String::new(),
                            })
                            .await;
                        events.llm_post_finished.notify_one();
                        if let Some(ref tx) = tui_tx {
                            tx.send(TuiEvent::AssistantDone).ok();
                        }
                        break 'pipeline;
                    }
                };

                *t_llm_post_send.lock().unwrap() = Some(Instant::now());

                let mut token_rx = token_rx;

                loop {
                    tokio::select! {
                        token = token_rx.recv() => {
                            match token {
                                Some(StreamToken::Content(t)) => {
                                    let t = if llm_text.is_empty() {
                                        t.trim_start_matches('\n').to_string()
                                    } else {
                                        t
                                    };
                                    if t.is_empty() { continue; }
                                    if !first_token_logged {
                                        first_token_logged = true;
                                        if let Some(t0) = t_llm_post_send.lock().unwrap().as_ref() {
                                            info!(target: "performance", "[+{}ms] LLM first token (TTFT)", t0.elapsed().as_millis());
                                        }
                                    }
                                    llm_text.push_str(&t);
                                    let _ = llm_tx.send(super::frames::PipelineFrame::LLMToken {
                                        utterance_id: pipeline_id,
                                        token: t.clone(),
                                    }).await;
                                    if let Some(ref tx) = tui_tx {
                                        tx.send(TuiEvent::AssistantToken(t.clone())).ok();
                                    }
                                    if let Some(ref ctrl) = control_broadcast {
                                        ctrl.send(ControlEvent::LlmToken {
                                            utterance_id: pipeline_id,
                                            token: t,
                                        });
                                    }
                                }
                                Some(StreamToken::ToolCall { name, args }) => {
                                    info!(target: "pipeline", "ToolCall received: name={} args={}", name, args);
                                    tool_call = Some((name, args));
                                    break;
                                }
                                None => {
                                    let _ = llm_tx.send(super::frames::PipelineFrame::LLMResponseDone {
                                        utterance_id: pipeline_id,
                                        full_text: llm_text.clone(),
                                    }).await;
                                    events.llm_post_finished.notify_one();
                                    if let Some(ref tx) = tui_tx {
                                        tx.send(TuiEvent::AssistantDone).ok();
                                    }
                                    if let Some(ref ctrl) = control_broadcast {
                                        ctrl.send(ControlEvent::LlmDone {
                                            utterance_id: pipeline_id,
                                            full_text: llm_text.clone(),
                                        });
                                    }
                                    break;
                                }
                            }
                        }
                        _ = cancel_rx.recv() => {
                            cancelled = true;
                            drop(token_rx);
                            stream_handle.abort();
                            break;
                        }
                    }
                }

                if cancelled {
                    if !llm_text.is_empty() {
                        let db_c = db.clone();
                        let resp_c = llm_text.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                db_c.save_message(session_id, "Assistant", &resp_c).await
                            {
                                warn!(target: "db", "Failed to save partial assistant message: {}", e);
                            }
                        });
                        llm_session.lock().unwrap().add_assistant_turn(&llm_text);
                        info!(
                            target: "pipeline",
                            "[pipe={}] Cancelled — partial response saved: {} chars",
                            pipeline_id, llm_text.len()
                        );
                    }
                    break 'pipeline;
                }

                match tool_call {
                    Some((name, args)) => {
                        if tools.lock().unwrap().is_background(&name) {
                            let ack_text = if !llm_text.trim().is_empty() {
                                llm_text.clone()
                            } else {
                                "Procesando en segundo plano, le aviso al terminar.".to_string()
                            };
                            let _ = llm_tx
                                .send(super::frames::PipelineFrame::LLMToken {
                                    utterance_id: pipeline_id,
                                    token: ack_text.clone(),
                                })
                                .await;
                            let _ = llm_tx
                                .send(super::frames::PipelineFrame::LLMResponseDone {
                                    utterance_id: pipeline_id,
                                    full_text: ack_text.clone(),
                                })
                                .await;
                            events.llm_post_finished.notify_one();

                            // Start background processing sound
                            filler_controller.start();

                            // Execute the tool directly: background tools spawn the real
                            // work internally and return a delegation placeholder
                            // immediately, so awaiting is cheap.
                            let tc_id = format!("bg_{}_{}_{}", pipeline_id, iter, name);
                            let tool_arc = tools.lock().unwrap().get_tool_arc(&name);
                            let placeholder = match tool_arc {
                                Some(tool) => tool.run(&args).await,
                                None => format!("Unknown tool: {name}"),
                            };
                            info!(
                                target: "pipeline",
                                "Background tool `{}` finished ({} chars): {:?}",
                                name, placeholder.len(), placeholder
                            );

                            // Persist the FULL exchange — assistant(tool_calls) followed
                            // by tool(result). The conversation history must show the
                            // correct tool-call pattern: if we save only the ack text,
                            // the model learns to *narrate* delegations instead of
                            // calling the tool on later turns.
                            let tool_call_msg = serde_json::json!({
                                "role": "assistant",
                                "content": ack_text,
                                "tool_calls": [{
                                    "id": tc_id,
                                    "type": "function",
                                    "function": {"name": &name, "arguments": &args}
                                }]
                            });
                            let tool_result_msg = serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tc_id,
                                "content": placeholder
                            });
                            let exchanges = vec![tool_call_msg, tool_result_msg];
                            {
                                let mut s = llm_session.lock().unwrap();
                                s.add_tool_exchange(exchanges.clone());
                            }
                            {
                                let db_c = db.clone();
                                let ex = exchanges.clone();
                                let ack_db = ack_text.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = db_c.save_tool_exchanges(session_id, &ex).await
                                    {
                                        warn!(target: "db", "Failed to save tool_call exchange: {}", e);
                                    }
                                    if let Err(e) =
                                        db_c.save_message(session_id, "Assistant", &ack_db).await
                                    {
                                        warn!(target: "db", "Failed to save bg ack message: {}", e);
                                    }
                                });
                            }
                            turn_commit_counter.fetch_add(1, Ordering::SeqCst);

                            // Track subtask + notify main loop (stops filler, updates TUI).
                            // main.rs skips session/DB persistence for bg_* ids — already
                            // persisted above.
                            let description =
                                format!("{}: {}", name, args.chars().take(80).collect::<String>());
                            let tracker = tools.lock().unwrap().subtask_tracker.clone();
                            tracker.add(tc_id.clone(), name.clone(), description);
                            if placeholder.starts_with("Error:")
                                || placeholder.starts_with("Unknown tool:")
                            {
                                tracker.fail(&tc_id, placeholder.clone());
                            } else {
                                tracker.complete(&tc_id, placeholder.clone());
                            }
                            proactive_tx
                                .send(ProactiveEvent::AgentResult {
                                    task: name.clone(),
                                    result: placeholder,
                                    tool_call_id: Some(tc_id),
                                    correlation_id: String::new(),
                                })
                                .await
                                .ok();

                            committed = true;
                            break 'pipeline;
                        }

                        let tool_for_exec = tools.lock().unwrap().get_tool_arc(&name);
                        let is_silent = tool_for_exec
                            .as_ref()
                            .map(|t| t.is_silent())
                            .unwrap_or(false);
                        let result = match tool_for_exec {
                            Some(tool) => tool.run(&args).await,
                            None => format!("Unknown tool: {name}"),
                        };
                        info!(target: "pipeline", "Tool[{}] `{}` → {}", iter, name, result);

                        // Inject prompt-build system message exactly once, when "start" action is called.
                        if name == "set_prompt_build"
                            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&args)
                            && parsed["action"].as_str() == Some("start")
                        {
                            llm_session.lock().unwrap().add_system_turn(
                                "[PROMPT-BUILD MODE ACTIVE] You are in prompt-build mode. \
                                 User messages are instructions to modify the prompt. \
                                 After changes, call set_prompt_build(action: \"update\"). \
                                 After saving/copying/sending the prompt, call set_prompt_build(action: \"cancel\"). \
                                 The current prompt text is in your tool call history."
                            );
                        }

                        // Silent tools (e.g. NOOP) suppress all response output.
                        if is_silent {
                            info!(
                                target: "pipeline",
                                "Tool `{}` is silent — suppressing response", name,
                            );
                            committed = false;
                            break 'pipeline;
                        }

                        if let Some(ref tx) = tui_tx {
                            tx.send(TuiEvent::ToolCall {
                                name: name.clone(),
                                result: result.clone(),
                            })
                            .ok();
                        }
                        if let Some(ref ctrl) = control_broadcast {
                            ctrl.send(ControlEvent::ToolCall {
                                name: name.clone(),
                                result: result.clone(),
                            });
                        }

                        // Flush sentence splitter between pre-tool narration and post-tool response
                        let _ = llm_tx
                            .send(PipelineFrame::LLMResponseDone {
                                utterance_id: pipeline_id,
                                full_text: String::new(),
                            })
                            .await;

                        let tool_call_id = format!("call_{}_{}", name, iter);
                        let content_value = if llm_text.trim().is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(llm_text.clone())
                        };
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content_value,
                            "tool_calls": [{
                                "id": tool_call_id,
                                "type": "function",
                                "function": {"name": name, "arguments": args}
                            }]
                        }));
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tool_call_id,
                            "content": result
                        }));

                        if cancel_rx.try_recv().is_ok() {
                            cancelled = true;
                            break 'pipeline;
                        }
                    }
                    None => {
                        final_response = llm_text;
                        break 'tool_loop;
                    }
                }
            }

            if final_response.is_empty() || cancelled {
                break 'pipeline;
            }

            info!(
                target: "pipeline",
                "[pipe={}] Assistant: {}",
                pipeline_id, final_response
            );

            {
                let db_c = db.clone();
                let resp_c = final_response.clone();
                let tool_exchanges_c = messages[base_msg_len..].to_vec();
                tokio::spawn(async move {
                    if !tool_exchanges_c.is_empty()
                        && let Err(e) = db_c
                            .save_tool_exchanges(session_id, &tool_exchanges_c)
                            .await
                    {
                        warn!(target: "db", "Failed to save tool exchanges: {}", e);
                    }
                    if let Err(e) = db_c.save_message(session_id, "Assistant", &resp_c).await {
                        warn!(target: "db", "Failed to save assistant message: {}", e);
                    }
                });
            }
            {
                let mut s = llm_session.lock().unwrap();
                let tool_exchanges = messages[base_msg_len..].to_vec();
                if !tool_exchanges.is_empty() {
                    s.add_tool_exchange(tool_exchanges);
                }
                // Reconcile SQLite user row if a pending user turn was extended by barge-in.
                if let Some(original) = pending_user_original.as_deref() {
                    let new_content = s
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m["role"].as_str() == Some("user"))
                        .and_then(|m| m["content"].as_str())
                        .map(str::to_string);
                    if let Some(new_text) = new_content
                        && new_text != original
                    {
                        let db_c = db.clone();
                        let original_c = original.to_string();
                        tokio::spawn(async move {
                            if let Err(e) = db_c
                                .update_user_message_content(session_id, &original_c, &new_text)
                                .await
                            {
                                warn!(target: "db", "Failed to reconcile user message on commit: {}", e);
                            }
                        });
                    }
                }
                s.add_assistant_turn(&final_response);
            }
            committed = true;
            pending_user_original = None;
        }

        // Cancelled path (above, ~lines 264–282) intentionally does not
        // reconcile the SQLite user row: the next user transcript will
        // either append (in-memory only) or start a new turn, and the
        // final reconcile fires on the next successful assistant-commit.

        if !committed && cancelled {
            info!(
                target: "pipeline",
                "[pipe={}] Cancelled — user message kept in session",
                pipeline_id
            );
        }

        while cancel_rx.try_recv().is_ok() {}
    }
}
