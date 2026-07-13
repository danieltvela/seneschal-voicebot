mod app;
pub mod events;
mod input;
mod ui;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::widgets::Widget;
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::pipeline::PipelineFrame;
use crate::tools::ConversationMode;
use crate::tools::PromptBuildState;
use app::{Action, App};
use events::{TuiEvent, TuiEventRx};
use input::KeyReader;

const TICK_MS: u64 = 33; // ~30fps
/// Height of the inline viewport at the bottom of the terminal.
/// One row is reserved for the status bar, up to four rows for the input
/// area, and the remaining rows are used for the streaming preview.
const VIEWPORT_HEIGHT: u16 = 10;

/// Run the TUI event loop. Blocks until the user quits.
pub async fn run(
    mut event_rx: TuiEventRx,
    transcript_tx: mpsc::Sender<PipelineFrame>,
    tts_muted: Arc<AtomicBool>,
    conv_mode: Arc<Mutex<ConversationMode>>,
    prompt_build_state: Arc<Mutex<PromptBuildState>>,
) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_HEIGHT),
        },
    )?;

    let mut app = App::new(conv_mode, prompt_build_state);
    let mut keys = KeyReader::new();
    let tick = tokio::time::Duration::from_millis(TICK_MS);

    app.handle_tui_event(TuiEvent::Splash);
    flush_new_messages(&mut terminal, &mut app)?;

    loop {
        // Render to terminal - no manual clearing needed with proper viewport
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        tokio::select! {
            Some(tui_event) = event_rx.recv() => {
                app.handle_tui_event(tui_event);
                while let Ok(ev) = event_rx.try_recv() {
                    app.handle_tui_event(ev);
                }
                flush_new_messages(&mut terminal, &mut app)?;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if event::poll(std::time::Duration::from_millis(0)).unwrap_or(false)
                    && let Event::Resize(_, _) = event::read().unwrap_or(Event::Key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::empty())))
                {
                    terminal.clear().unwrap_or_default();
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

    // Flush any remaining messages before exiting.
    flush_new_messages(&mut terminal, &mut app)?;
    execute!(io::stdout(), crossterm::cursor::Show)?;
    disable_raw_mode()?;
    execute!(io::stdout(), crossterm::cursor::MoveToNextLine(1))?;
    Ok(())
}

/// Print any finalized messages that have not yet been written to the
/// terminal's normal scrollback buffer.
fn flush_new_messages(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    while app.last_printed_index < app.messages.len() {
        let msg = &app.messages[app.last_printed_index];
        let width = terminal.size()?.width;
        let mut lines = ui::message_lines(msg, width);
        lines.push(ratatui::text::Line::raw(""));
        let height = lines.len().clamp(1, 50) as u16;
        terminal.insert_before(height, |buf| {
            ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines))
                .render(*buf.area(), buf);
        })?;
        app.last_printed_index += 1;
    }
    Ok(())
}
