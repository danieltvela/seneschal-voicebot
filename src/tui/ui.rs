use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
};

use super::app::{App, ChatMessage, Role};
use super::events::{InputSource, PipelineState};
use crate::tools::ConversationMode;

const MAX_INPUT_ROWS: u16 = 4;

/// Compute layout heights for the five regions.
///
/// Returns `(history, streaming, prompt, input, status)`.
/// All values sum to `total_h`, and `status` is always `1` (pinned to the last row).
fn compute_layout_heights(
    total_h: u16,
    input_h: u16,
    prompt_h: u16,
    streaming_nonempty: bool,
) -> (u16, u16, u16, u16, u16) {
    let status_h = 1u16;
    let after_status = total_h.saturating_sub(status_h);

    // Clamp input to available space
    let input_clamped = input_h.min(after_status);
    let after_input = after_status.saturating_sub(input_clamped);

    // Clamp prompt to remaining space
    let prompt_clamped = prompt_h.min(after_input);
    let remaining = after_input.saturating_sub(prompt_clamped);

    // Split remaining between streaming and history
    let (streaming_h, history_h) = if !streaming_nonempty || remaining < 3 {
        (0, remaining)
    } else {
        let sh = (remaining / 3).max(3).min(remaining);
        (sh, remaining.saturating_sub(sh))
    };

    (
        history_h,
        streaming_h,
        prompt_clamped,
        input_clamped,
        status_h,
    )
}

/// Render the fullscreen TUI.
///
/// The layout (top → bottom) is:
///   1. Message history (scrollable, auto-scroll to bottom)
///   2. Streaming preview (when assistant is speaking)
///   3. Prompt-build display (when active)
///   4. Text input
///   5. Status bar (always the last row)
pub fn render(frame: &mut Frame, app: &mut App) {
    let total = frame.area();
    let width = total.width as usize;

    // Input height: wraps at terminal width (no border so full width available).
    let input_height =
        input_display_lines(&app.input, width).clamp(1, MAX_INPUT_ROWS as usize) as u16;

    // Prompt-build display height: show only when active.
    let prompt_active = app.prompt_build_state.lock().unwrap().is_active();
    let prompt_height = if prompt_active {
        let prompt_text = app
            .prompt_build_state
            .lock()
            .unwrap()
            .prompt_text()
            .unwrap_or("")
            .to_string();
        compute_prompt_display_height(&prompt_text, width).min(6) as u16
    } else {
        0
    };

    let (history_height, streaming_height, prompt_h, input_h, status_h) = compute_layout_heights(
        total.height,
        input_height,
        prompt_height,
        !app.streaming_buffer.is_empty(),
    );

    let areas = Layout::vertical([
        Constraint::Length(history_height),
        Constraint::Length(streaming_height),
        Constraint::Length(prompt_h),
        Constraint::Length(input_h),
        Constraint::Length(status_h),
    ])
    .split(total);

    let history_area = areas[0];
    let streaming_area = areas[1];
    let prompt_area = areas[2];
    let input_area = areas[3];
    let status_area = areas[4];

    if history_height > 0 {
        render_history(frame, app, history_area);
    }
    if streaming_height > 0 {
        render_streaming(frame, app, streaming_area);
    }
    if prompt_height > 0 {
        render_prompt_display(frame, app, prompt_area);
    }
    render_input(frame, app, input_area);
    render_status(frame, app, status_area);
}

/// Render the message history, auto-scrolled to the bottom.
fn render_history(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for msg in &app.messages {
        let mut lines = message_lines(msg, area.width);
        lines.push(Line::raw(""));
        all_lines.extend(lines);
    }

    // Auto-scroll to bottom: show the last `area.height` rows.
    let skip = all_lines.len().saturating_sub(area.height as usize);
    let display = Text::from(all_lines[skip..].to_vec());
    frame.render_widget(Paragraph::new(display), area);
}

/// Render the SENECHAL splash screen (blue, centered).
fn render_splash(text: &str, width: usize) -> Vec<Line<'static>> {
    let text = text.to_string(); // Clone to make it 'static
    let mut lines: Vec<Line<'static>> = vec![];

    // Add top border
    lines.push(Line::from(vec![
        Span::raw("┌"),
        Span::raw("─".repeat(width.saturating_sub(2))),
        Span::raw("┐"),
    ]));

    // Add splash content with blue styling
    for line in text.lines() {
        let trimmed = line.trim_end().to_string();
        if !trimmed.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("│ "),
                Span::styled(
                    trimmed,
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));
        }
    }

    // Add bottom border
    lines.push(Line::from(vec![
        Span::raw("└"),
        Span::raw("─".repeat(width.saturating_sub(2))),
        Span::raw("┘"),
    ]));

    lines
}

/// Build display lines for streaming buffer.
fn render_streaming_lines(buffer: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::raw("┌ "),
        Span::styled(
            "seneschal [streaming]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ])];

    for content_line in buffer.lines() {
        let wrapped = word_wrap_plain(&format!("│ {content_line}"), width);
        for row in wrapped {
            lines.push(Line::raw(row));
        }
    }

    lines.push(Line::from(vec![
        Span::raw("└"),
        Span::raw("─".repeat(width - 2)),
        Span::raw("┘"),
    ]));

    lines
}

/// Build display lines for a finalized message.
fn message_lines(msg: &ChatMessage, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut lines: Vec<Line<'static>> = vec![];

    match &msg.role {
        Role::Splash => {
            // Splash screen - show SENECHAL ASCII art
            let splash_text = r#"
  _    _     _            ______             
 | |  | |   (_)          (____  \       _    
 | |  | |__  _  ____ ____ ____)  ) ___ | |_  
  \ \/ / _ \| |/ ___) _  )  __  ( / _ \|  _) 
   \  / |_| | ( (__( (/ /| |__)  ) |_| | |__ 
    \/ \___/|_|\____)____)______/ \___/ \___)
"#;
            lines.extend(render_splash(splash_text, w));
        }
        Role::User(source) => {
            let source_label = match source {
                InputSource::Voice => "voice",
                InputSource::Text => "text",
            };
            let time = msg.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

            lines.push(Line::from(vec![
                Span::raw("┌ "),
                Span::styled(
                    format!("You [{source_label}]"),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(time, Style::default().fg(Color::Rgb(100, 100, 100))),
            ]));

            for content_line in msg.content.lines() {
                let wrapped = word_wrap_plain(content_line, w.saturating_sub(2));
                for line in wrapped {
                    lines.push(Line::from(vec![Span::raw("│ "), Span::raw(line)]));
                }
            }

            let content_lines = msg.content.lines().count();
            if content_lines > 0 {
                lines.push(Line::from(vec![
                    Span::raw("└"),
                    Span::raw("─".repeat(w.saturating_sub(2))),
                    Span::raw("┘"),
                ]));
            }
        }
        Role::Assistant => {
            let time = msg.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

            lines.push(Line::from(vec![
                Span::raw("┌ "),
                Span::styled(
                    "seneschal",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(time, Style::default().fg(Color::Rgb(100, 100, 100))),
            ]));

            for content_line in msg.content.lines() {
                let wrapped = word_wrap_plain(content_line, w.saturating_sub(2));
                for line in wrapped {
                    lines.push(Line::from(vec![Span::raw("│ "), Span::raw(line)]));
                }
            }

            let content_lines = msg.content.lines().count();
            if content_lines > 0 {
                lines.push(Line::from(vec![
                    Span::raw("└"),
                    Span::raw("─".repeat(w.saturating_sub(2))),
                    Span::raw("┘"),
                ]));
            }
        }
        Role::Tool => {
            // Tool call - gray, indented
            let tool_text = format!("  > tool: {}", msg.content);
            for row in word_wrap_plain(&tool_text, w) {
                lines.push(Line::from(vec![Span::styled(
                    row,
                    Style::default().fg(Color::Rgb(100, 100, 100)).italic(),
                )]));
            }
        }
        Role::System => {
            let time = msg.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

            lines.push(Line::from(vec![
                Span::raw("┌ "),
                Span::styled(
                    "System",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(time, Style::default().fg(Color::Rgb(100, 100, 100))),
            ]));

            for content_line in msg.content.lines() {
                let wrapped = word_wrap_plain(content_line, w.saturating_sub(2));
                for line in wrapped {
                    lines.push(Line::from(vec![Span::styled(
                        format!("│ {line}"),
                        Style::default()
                            .fg(Color::Rgb(180, 180, 100))
                            .add_modifier(Modifier::ITALIC),
                    )]));
                }
            }

            let content_lines = msg.content.lines().count();
            if content_lines > 0 {
                lines.push(Line::from(vec![
                    Span::raw("└"),
                    Span::raw("─".repeat(w.saturating_sub(2))),
                    Span::raw("┘"),
                ]));
            }
        }
        Role::Error => {
            let time = msg.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

            lines.push(Line::from(vec![
                Span::raw("┌ "),
                Span::styled(
                    "ERROR",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(time, Style::default().fg(Color::Rgb(100, 100, 100))),
            ]));

            for content_line in msg.content.lines() {
                let wrapped = word_wrap_plain(content_line, w.saturating_sub(2));
                for line in wrapped {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::Red)),
                        Span::styled(line, Style::default().fg(Color::Red)),
                    ]));
                }
            }

            let content_lines = msg.content.lines().count();
            if content_lines > 0 {
                lines.push(Line::from(vec![
                    Span::raw("└"),
                    Span::raw("─".repeat(w.saturating_sub(2))),
                    Span::raw("┘"),
                ]));
            }
        }
    }

    lines
}

/// Show the live streaming assistant text, auto-scrolled to the bottom of the area.
fn render_streaming(frame: &mut Frame, app: &App, area: Rect) {
    if app.streaming_buffer.is_empty() && area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let mut all_lines: Vec<Line<'static>> = vec![];

    // Streaming header with border
    all_lines.push(Line::from(vec![
        Span::raw("┌ "),
        Span::styled(
            "seneschal [streaming]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    for content_line in app.streaming_buffer.lines() {
        for row in word_wrap_plain(&format!("│ {content_line}"), width) {
            all_lines.push(Line::raw(row));
        }
    }

    // Add closing border line (always show to maintain visual consistency)
    all_lines.push(Line::from(vec![
        Span::raw("└"),
        Span::raw("─".repeat(width - 2)),
        Span::raw("┘"),
    ]));

    // Clip to the last `area.height` rows (auto-scroll to bottom).
    let skip = all_lines.len().saturating_sub(area.height as usize);
    let display = Text::from(all_lines[skip..].to_vec());
    frame.render_widget(Paragraph::new(display), area);
}

/// Render the prompt-build display (read-only).
fn render_prompt_display(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let prompt_text = app
        .prompt_build_state
        .lock()
        .unwrap()
        .prompt_text()
        .unwrap_or("")
        .to_string();

    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::raw("┌ "),
        Span::styled(
            "PROMPT BUILD",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ])];

    if prompt_text.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("│ ", Style::default().fg(Color::Rgb(100, 100, 100))),
            Span::styled(
                "(awaiting instructions...)",
                Style::default().fg(Color::Rgb(100, 100, 100)).italic(),
            ),
        ]));
    } else {
        for content_line in prompt_text.lines() {
            let wrapped = word_wrap_plain(&format!("│ {content_line}"), width);
            for row in wrapped {
                lines.push(Line::from(vec![Span::styled(
                    row,
                    Style::default().fg(Color::Yellow),
                )]));
            }
        }
    }

    lines.push(Line::from(vec![
        Span::raw("└"),
        Span::raw("─".repeat(width.saturating_sub(2))),
        Span::raw("┘"),
    ]));

    // Clip to area height
    let skip = lines.len().saturating_sub(area.height as usize);
    let display = Text::from(lines[skip..].to_vec());
    frame.render_widget(Paragraph::new(display), area);
}

/// Compute the display height needed for the prompt-build content.
fn compute_prompt_display_height(prompt_text: &str, width: usize) -> usize {
    if prompt_text.is_empty() {
        // Title line + "(awaiting...)" line + bottom border = 3
        return 3;
    }
    let mut total = 2; // title line + bottom border
    for line in prompt_text.lines() {
        let wrapped = word_wrap_plain(&format!("│ {line}"), width);
        total += wrapped.len();
    }
    total
}

/// Render the text input — no border, full width.
fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;

    let text = if app.input.is_empty() {
        Text::from(Line::from(vec![
            Span::styled("┌ ", Style::default().fg(Color::Rgb(100, 100, 100))),
            Span::styled(
                "Type a message... (Enter to send)",
                Style::default().fg(Color::Rgb(100, 100, 100)),
            ),
        ]))
    } else {
        let chars: Vec<char> = app.input.chars().collect();
        let lines: Vec<Line> = if width == 0 {
            vec![Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::Rgb(100, 100, 100))),
                Span::raw(app.input.as_str()),
            ])]
        } else {
            chars
                .chunks(width)
                .map(|chunk| {
                    Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::Rgb(100, 100, 100))),
                        Span::raw(chunk.iter().collect::<String>()),
                    ])
                })
                .collect()
        };
        Text::from(lines)
    };

    frame.render_widget(Paragraph::new(text), area);

    // Position cursor - account for "│ " prefix (2 chars)
    let char_pos = app.input[..app.cursor].chars().count();
    let (row, col) = if width == 0 {
        (0u16, 2u16 + char_pos as u16)
    } else {
        let prefix_offset = 2; // "│ " is 2 characters
        let line_num = char_pos.checked_div(width).unwrap_or(0);
        let col_in_line = char_pos.checked_rem(width).unwrap_or(0);
        (line_num as u16, prefix_offset as u16 + col_in_line as u16)
    };
    frame.set_cursor_position((area.x + col, area.y + row));
}

/// Render the status bar at the bottom of the viewport.
fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let (state_label, state_color) = match app.state {
        PipelineState::Idle => ("● IDLE", Color::Rgb(100, 100, 100)),
        PipelineState::Listening => ("● LISTENING", Color::Green),
        PipelineState::Transcribing => ("● TRANSCRIBING", Color::Yellow),
        PipelineState::Thinking => ("● THINKING", Color::Rgb(100, 100, 255)),
        PipelineState::Speaking => ("● SPEAKING", Color::Magenta),
    };

    let tts_label = if app.tts_enabled { "TTS ON" } else { "TTS OFF" };
    let tts_color = if app.tts_enabled {
        Color::Green
    } else {
        Color::Rgb(100, 100, 100)
    };

    let (conv_label, conv_color) = match *app.conv_mode.lock().unwrap() {
        ConversationMode::Active => ("ACTIVE", Color::Cyan),
        ConversationMode::Ambient => ("AMBIENT", Color::Rgb(100, 100, 100)),
        ConversationMode::AmbientLocked => ("AMBIENT🔒", Color::Yellow),
    };

    let text = Text::from(vec![Line::from(vec![
        Span::styled(
            " seneschal ",
            Style::default().fg(Color::Rgb(200, 200, 200)).bold(),
        ),
        Span::raw(" "),
        Span::styled(state_label, Style::default().fg(state_color)),
        Span::raw(" │ "),
        Span::styled(tts_label, Style::default().fg(tts_color)),
        Span::raw(" │ "),
        Span::styled(conv_label, Style::default().fg(conv_color)),
        Span::raw(" │ "),
        Span::styled(
            "Ctrl+T: toggle TTS  Esc: quit",
            Style::default().fg(Color::Rgb(100, 100, 100)),
        ),
    ])]);

    let block = Block::default().style(Style::default().bg(Color::Rgb(40, 40, 50)));

    frame.render_widget(Paragraph::new(text).block(block), area);
}

/// Number of visual rows the input text occupies with hard-wrap at `width`.
fn input_display_lines(input: &str, width: usize) -> usize {
    if width == 0 || input.is_empty() {
        return 1;
    }
    let char_count = input.chars().count();
    char_count.div_ceil(width)
}

/// Word-wrap `text` to `width` columns. Returns one owned `String` per visual row.
fn word_wrap_plain(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }

    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w: usize = 0;

    let content = text.trim_start_matches(' ');
    let leading = text.len() - content.len();
    for _ in 0..leading {
        if current_w < width {
            current.push(' ');
            current_w += 1;
        } else {
            rows.push(std::mem::take(&mut current));
            current.push(' ');
            current_w = 1;
        }
    }

    let mut after_leading = leading > 0;

    for word in content.split_whitespace() {
        let ww = word.chars().count();
        if after_leading {
            after_leading = false;
            if current_w + ww <= width {
                current.push_str(word);
                current_w += ww;
            } else {
                rows.push(std::mem::take(&mut current));
                current_w = 0;
                place_word_at_row_start(&mut rows, &mut current, &mut current_w, word, ww, width);
            }
        } else if current_w == 0 {
            place_word_at_row_start(&mut rows, &mut current, &mut current_w, word, ww, width);
        } else if current_w + 1 + ww <= width {
            current.push(' ');
            current.push_str(word);
            current_w += 1 + ww;
        } else {
            rows.push(std::mem::take(&mut current));
            current_w = 0;
            place_word_at_row_start(&mut rows, &mut current, &mut current_w, word, ww, width);
        }
    }

    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

fn place_word_at_row_start(
    rows: &mut Vec<String>,
    current: &mut String,
    current_w: &mut usize,
    word: &str,
    ww: usize,
    width: usize,
) {
    if ww <= width {
        current.push_str(word);
        *current_w = ww;
    } else {
        for ch in word.chars() {
            if *current_w >= width {
                rows.push(std::mem::take(current));
                *current_w = 0;
            }
            current.push(ch);
            *current_w += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_returns_one_row() {
        assert_eq!(word_wrap_plain("", 80), vec![""]);
    }

    #[test]
    fn short_line_fits_in_one_row() {
        assert_eq!(word_wrap_plain("hello world", 80), vec!["hello world"]);
    }

    #[test]
    fn line_exactly_at_width_is_one_row() {
        assert_eq!(word_wrap_plain("ab cd", 5), vec!["ab cd"]);
    }

    #[test]
    fn line_one_char_over_wraps_to_two_rows() {
        assert_eq!(word_wrap_plain("ab cde", 5), vec!["ab", "cde"]);
    }

    #[test]
    fn long_line_wraps_correctly() {
        let text = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj";
        assert_eq!(
            word_wrap_plain(text, 20),
            vec!["aaaa bbbb cccc dddd", "eeee ffff gggg hhhh", "iiii jjjj"]
        );
    }

    #[test]
    fn word_wider_than_width_is_hard_wrapped() {
        assert_eq!(word_wrap_plain("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn indented_line_preserves_leading_spaces() {
        assert_eq!(word_wrap_plain("  hello world", 80), vec!["  hello world"]);
    }

    #[test]
    fn indented_line_counts_spaces_in_width() {
        assert_eq!(word_wrap_plain("  ab cd", 6), vec!["  ab", "cd"]);
    }

    #[test]
    fn zero_width_returns_original() {
        assert_eq!(word_wrap_plain("hello world", 0), vec!["hello world"]);
    }

    // Layout height tests

    #[test]
    fn layout_fills_total_height_idle() {
        // total 24, input 1, prompt 0, no streaming
        let (h, s, p, i, st) = compute_layout_heights(24, 1, 0, false);
        assert_eq!(st, 1);
        assert_eq!(i, 1);
        assert_eq!(p, 0);
        assert_eq!(s, 0);
        assert_eq!(h, 22);
        assert_eq!(h + s + p + i + st, 24);
    }

    #[test]
    fn layout_status_always_one() {
        let (_, _, _, _, st) = compute_layout_heights(30, 2, 3, true);
        assert_eq!(st, 1);
    }

    #[test]
    fn layout_tiny_terminal() {
        // total 3, input 1, prompt 0, no streaming
        let (h, s, p, i, st) = compute_layout_heights(3, 1, 0, false);
        assert_eq!(st, 1);
        assert_eq!(i, 1);
        assert_eq!(p, 0);
        assert_eq!(s, 0);
        assert_eq!(h, 1);
        assert_eq!(h + s + p + i + st, 3);
    }

    #[test]
    fn layout_with_streaming_splits_remaining() {
        // total 30, input 2, prompt 0, streaming true
        let (h, s, p, i, st) = compute_layout_heights(30, 2, 0, true);
        assert_eq!(st, 1);
        assert_eq!(i, 2);
        assert_eq!(p, 0);
        assert!(s > 0, "streaming height should be > 0");
        assert!(h > 0, "history height should be > 0");
        assert_eq!(h + s + p + i + st, 30);
    }

    #[test]
    fn layout_sum_always_equals_total() {
        for total in 3..=60u16 {
            for input_h in 1..=4u16 {
                for prompt_h in 0..=6u16 {
                    for streaming in [false, true] {
                        let (h, s, p, i, st) =
                            compute_layout_heights(total, input_h, prompt_h, streaming);
                        let sum = h + s + p + i + st;
                        assert_eq!(
                            sum, total,
                            "total={}, input={}, prompt={}, streaming={}",
                            total, input_h, prompt_h, streaming
                        );
                        assert_eq!(st, 1);
                    }
                }
            }
        }
    }
}
