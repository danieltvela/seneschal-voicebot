// seneschal-tui — Terminal user interface for Seneschal.
//
// Status-only TUI showing conversation and pipeline state.

mod app;
pub mod events;
mod input;
mod ui;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use tokio::sync::mpsc;

use app::{Action, App};
use events::{TuiEvent, TuiEventRx};
use input::KeyReader;
use seneschal_common::tools::{ConversationMode, PromptBuildState};
use seneschal_core::pipeline::PipelineFrame;

const TICK_MS: u64 = 33; // ~30fps

/// Run the TUI event loop. Blocks until the user quits.
///
/// `initial_history` is the session message list already loaded from the DB
/// (`role`, `content` pairs — same shape as `get_session_context`). It is
/// seeded into the chat buffer after the splash so a relaunch rehydrates the
/// previous conversation instead of starting empty.
pub async fn run(
    mut event_rx: TuiEventRx,
    transcript_tx: mpsc::Sender<PipelineFrame>,
    tts_muted: Arc<AtomicBool>,
    conv_mode: Arc<Mutex<ConversationMode>>,
    prompt_build_state: Arc<Mutex<PromptBuildState>>,
    initial_history: Vec<(String, String)>,
) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    execute!(io::stdout(), EnableMouseCapture)?;
    execute!(io::stdout(), crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
    )?;

    let mut app = App::new(conv_mode, prompt_build_state);
    let mut keys = KeyReader::new();
    let tick = tokio::time::Duration::from_millis(TICK_MS);

    app.handle_tui_event(TuiEvent::Splash);

    // Rehydrate prior session turns so quit/relaunch shows conversation context.
    let seeded = app.seed_history(&initial_history);
    if seeded > 0 {
        app.handle_tui_event(TuiEvent::SystemNotification {
            text: format!("Session restored ({seeded} messages)"),
        });
        tracing::info!(
            target: "tui",
            seeded,
            "Rehydrated chat history from previous session"
        );
    }

    loop {
        // Render to terminal - fullscreen viewport always fills the screen
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        tokio::select! {
            Some(tui_event) = event_rx.recv() => {
                app.handle_tui_event(tui_event);
                while let Ok(ev) = event_rx.try_recv() {
                    app.handle_tui_event(ev);
                }
            }
            key_result = keys.next() => {
                match key_result {
                    Ok(Some(event)) => {
                        let history_area = app.history_area;
                        if let Some(action) = app.handle_event(event, history_area) {
                            match action {
                                Action::Quit => {
                                    app.should_quit = true;
                                }
                                Action::SubmitToSeneschal(text) => {
                                    transcript_tx.send(PipelineFrame::TextInput { text }).await.ok();
                                }
                                Action::ToggleTts => {
                                    let was_muted = tts_muted.load(Ordering::SeqCst);
                                    tts_muted.store(!was_muted, Ordering::SeqCst);
                                    app.tts_enabled = was_muted;
                                }
                                Action::ScrollToBottom => {
                                    app.auto_scroll_to_bottom = true;
                                }
                            }
                        }
                    }
                    Ok(None) => { app.should_quit = true; }
                    Err(e) => { tracing::error!("Key reader error: {e}"); }
                }
            }
            _ = tokio::time::sleep(tick) => {}
        }

        if app.should_quit {
            break;
        }
    }

    // Final render before exit
    terminal.draw(|frame| ui::render(frame, &mut app))?;
    execute!(io::stdout(), crossterm::cursor::Show)?;
    execute!(io::stdout(), DisableMouseCapture)?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
