# src/tui/ — Terminal UI (ratatui)

## Responsibility

Terminal-based user interface for the Voicebot pipeline. Provides an inline viewport (bottom 6 rows) for real-time streaming display, text input, and status monitoring. Finalized messages are printed to the terminal's normal scrollback buffer above the viewport.

## Design

### Module Structure

| File | Role |
|------|------|
| `mod.rs` | Event loop, terminal setup, viewport management |
| `app.rs` | Application state, event handling, key processing |
| `ui.rs` | Rendering: streaming buffer, input, status bar, message formatting |
| `events.rs` | Event types: `TuiEvent`, `PipelineState`, `InputSource` |
| `input.rs` | Async keyboard event reader (`KeyReader`) |

### Constants

- `TICK_MS = 33` — Render tick interval (~30fps).
- `VIEWPORT_HEIGHT = 6` — Inline viewport height (bottom of terminal).
- `MAX_INPUT_ROWS = 4` — Maximum input area height.

### Application State (`App`)

```rust
pub struct App {
    pub messages: Vec<ChatMessage>,        // Finalized messages
    pub streaming_buffer: String,           // Current LLM streaming text
    pub state: PipelineState,               // Pipeline state
    pub input: String,                      // Text input buffer
    pub cursor: usize,                      // Cursor position in input
    pub tts_enabled: bool,                  // TTS toggle
    pub should_quit: bool,                  // Quit flag
    pub conv_mode: Arc<Mutex<ConversationMode>>, // Shared conversation mode
    pub last_printed_index: usize,          // Last message flushed to scrollback
}
```

### Message Roles

```rust
enum Role {
    User(InputSource),   // Voice or Text input
    Assistant,
    Tool,
    Error,
    System,
    Splash,
}
```

### Layout

```
┌─────────────────────────────┐
│  Terminal scrollback        │  ← Finalized messages (insert_before)
│  (grows upward)             │
├─────────────────────────────┤
│  Jarvis [streaming]         │  ← Streaming area (dynamic height)
│  Current LLM response...    │
├─────────────────────────────┤
│  │ Input text...            │  ← Input area (1-4 rows)
├─────────────────────────────┤
│ voicebot ● LISTENING │ TTS  │  ← Status bar (1 row)
└─────────────────────────────┘
```

## Flow

### Terminal Setup

```
run(event_rx, transcript_tx, tts_muted, conv_mode)
  → enable_raw_mode()
  → Hide cursor
  → CrosstermBackend::new(stdout)
  → Terminal::with_options(backend, Viewport::Inline(6))
  → App::new(conv_mode)
  → KeyReader::new()
  
  → handle_tui_event(Splash)
  → flush_new_messages(terminal, app)
  
  → Main event loop (tokio::select!)
```

### Event Loop

```
loop:
  terminal.draw(|frame| ui::render(frame, &mut app))
  
  select! {
    // Pipeline events (TUI updates)
    Some(tui_event) = event_rx.recv() => {
      app.handle_tui_event(tui_event)
      // Drain buffered events
      while let Ok(ev) = event_rx.try_recv():
        app.handle_tui_event(ev)
      flush_new_messages(terminal, app)
    }
    
    // Terminal resize
    _ = sleep(100ms) => {
      if resize event: terminal.clear()
    }
    
    // Keyboard input
    key_result = keys.next() => {
      match app.handle_key_event(event):
        Action::Quit → app.should_quit = true
        Action::Submit(text) → transcript_tx.send(TextInput { text })
        Action::ToggleTts → toggle tts_muted flag
    }
    
    // Render tick
    _ = sleep(TICK_MS) => {}
  }
  
  if app.should_quit: break
```

### Event Handling

```
handle_tui_event(event):
  StateChange(s) → app.state = s
  UserMessage { text, source } → push ChatMessage { User(source), text }
  AssistantToken(token) → append to streaming_buffer
  AssistantDone → finalize streaming_buffer as ChatMessage { Assistant }
  Error(msg) → push ChatMessage { Error, msg }
  SystemNotification { text } → push ChatMessage { System, text }
  ToolCall { name, result } → push ChatMessage { Tool, "name -> result..." }
  Splash → push ChatMessage { Splash }
```

### Key Handling

```
handle_key_event(event):
  Ctrl+C / Esc → Action::Quit
  Ctrl+T → Action::ToggleTts
  Enter → Action::Submit(input.trim())
  Backspace → delete char before cursor
  Delete → delete char at cursor
  Left/Right → move cursor (char-aware)
  Home/End → move to start/end
  Char(c) → insert at cursor
```

### Message Flushing

```
flush_new_messages(terminal, app):
  while last_printed_index < messages.len():
    msg = messages[last_printed_index]
    lines = ui::message_lines(msg, terminal_width)
    lines.push(Paragraph::raw(""))
    height = lines.len().clamp(1, 50)
    terminal.insert_before(height, |buf| render lines)
    last_printed_index += 1
```

### Rendering

```
render(frame, app):
  total = frame.area()
  input_height = input_display_lines(input, width).clamp(1, 4)
  status_height = 1
  streaming_height = total.height - input_height - status_height (if buffer not empty)
  
  [streaming_area, input_area, status_area] = vertical layout
  
  if streaming_height > 0:
    render_streaming(frame, app, streaming_area)
  render_input(frame, app, input_area)
  render_status(frame, app, status_area)

render_streaming:
  → "┌ Jarvis [streaming]" header
  → Word-wrapped streaming buffer lines
  → "└───" footer
  → Auto-scroll: clip to last `area.height` rows

render_input:
  → "│ " prefix per line
  → Word-wrapped input text
  → Position cursor at correct (x, y)

render_status:
  → " voicebot " (bold gray)
  → State label (color-coded: IDLE=gray, LISTENING=green, TRANSCRIBING=yellow, THINKING=blue, SPEAKING=magenta)
  → "│ TTS ON/OFF │"
  → "│ ACTIVE/AMBIENT/AMBIENT🔒 │"
  → "│ Ctrl+T: toggle TTS  Esc: quit"
```

### Word Wrapping

```
word_wrap_plain(text, width):
  → Handle leading spaces
  → Split by whitespace
  → For each word:
    if fits in current line: append with space
    else: push current line, start new line
  → Words wider than width: hard-wrap char by char
```

## Integration

### Dependencies
- `ratatui` — Terminal UI framework.
- `crossterm` — Terminal event handling, raw mode.
- `futures_util` — Async event stream.
- `tokio::sync::mpsc` — Event channel from pipeline.
- `crate::pipeline::PipelineFrame` — Input injection.
- `crate::tools::ConversationMode` — Shared conversation mode.

### Consumers
- `src/main.rs` — Calls `tui::run(event_rx, transcript_tx, tts_muted, conv_mode)`.
- `src/daemon.rs` — Sends `TuiEvent` via `TuiEventTx` channel.

### Events
- `TuiEventTx` / `TuiEventRx` — Unbounded mpsc channel for pipeline → TUI communication.
- `TuiEvent` variants: `StateChange`, `UserMessage`, `AssistantToken`, `AssistantDone`, `ToolCall`, `SystemNotification`, `Error`, `Splash`.