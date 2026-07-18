mod app;
pub mod events;
mod input;
mod ui;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::pipeline::PipelineFrame;
use crate::tools::ConversationMode;
use crate::tools::PromptBuildState;
use app::{Action, App};
use events::{TuiEvent, TuiEventRx};
use input::KeyReader;

const TICK_MS: u64 = 33; // ~30fps

/// Run the TUI event loop. Blocks until the user quits.
pub async fn run(
    mut event_rx: TuiEventRx,
    transcript_tx: mpsc::Sender<PipelineFrame>,
    tts_muted: Arc<AtomicBool>,
    conv_mode: Arc<Mutex<ConversationMode>>,
    prompt_build_state: Arc<Mutex<PromptBuildState>>,
) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
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
                        if let Some(action) = app.handle_key_event(event) {
                            match action {
                                Action::Quit => {
                                    app.should_quit = true;
                                }
                                Action::Submit(text) => {
                                    transcript_tx.send(PipelineFrame::TextInput { text }).await.ok();
                                }
                                Action::ToggleTts => {
                                    let was_muted = tts_muted.load(Ordering::SeqCst);
                                    tts_muted.store(!was_muted, Ordering::SeqCst);
                                    app.tts_enabled = was_muted;
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
    execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
