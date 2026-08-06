use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use super::app::{AgentTaskStatus, App, ChatMessage, Role};
use super::events::{InputSource, PipelineState};
use seneschal_common::classifier::{ClassifierForceMode, Intent};
use seneschal_common::tools::ConversationMode;

const MAX_INPUT_ROWS: u16 = 4;

/// Compute layout heights for conversation stack regions inside `main_h`.
///
/// Returns `(history, streaming, prompt)`. Values sum to `main_h`.
/// Input and status are laid out outside this function (full width).
#[allow(dead_code)]
fn compute_layout_heights(
    total_h: u16,
    input_h: u16,
    prompt_h: u16,
    streaming_nonempty: bool,
) -> (u16, u16, u16, u16, u16) {
    // Legacy signature kept for tests: when called with full terminal height,
    // also returns input/status. Prefer `compute_conversation_heights` for the
    // side-by-side layout path.
    let status_h = 1u16;
    let after_status = total_h.saturating_sub(status_h);
    let input_clamped = input_h.min(after_status);
    let after_input = after_status.saturating_sub(input_clamped);
    let prompt_clamped = prompt_h.min(after_input);
    let remaining = after_input.saturating_sub(prompt_clamped);
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

/// Heights for history/streaming/prompt inside a main pane of height `main_h`.
fn compute_conversation_heights(
    main_h: u16,
    prompt_h: u16,
    streaming_nonempty: bool,
) -> (u16, u16, u16) {
    let prompt_clamped = prompt_h.min(main_h);
    let remaining = main_h.saturating_sub(prompt_clamped);
    let (streaming_h, history_h) = if !streaming_nonempty || remaining < 3 {
        (0, remaining)
    } else {
        let sh = (remaining / 3).max(3).min(remaining);
        (sh, remaining.saturating_sub(sh))
    };
    (history_h, streaming_h, prompt_clamped)
}

/// Render the fullscreen TUI.
///
/// Outer: main | input | status. Main fills the full width.
pub fn render(frame: &mut Frame, app: &mut App) {
    let total = frame.area();
    let width = total.width as usize;

    let input_height =
        input_display_lines(&app.input, width).clamp(1, MAX_INPUT_ROWS as usize) as u16;
    let status_h = 1u16;
    let after_status = total.height.saturating_sub(status_h);
    let input_h = input_height.min(after_status);
    let main_h = after_status.saturating_sub(input_h);

    let outer = Layout::vertical([
        Constraint::Length(main_h),
        Constraint::Length(input_h),
        Constraint::Length(status_h),
    ])
    .split(total);

    let main_area = outer[0];
    let input_area = outer[1];
    let status_area = outer[2];

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

    let (history_height, streaming_height, prompt_h) = compute_conversation_heights(
        main_area.height,
        prompt_height,
        !app.streaming_buffer.is_empty(),
    );

    let main_parts = Layout::vertical([
        Constraint::Length(history_height),
        Constraint::Length(streaming_height),
        Constraint::Length(prompt_h),
    ])
    .split(main_area);

    if history_height > 0 {
        app.history_area = main_parts[0];
        render_chat_list(frame, app, main_parts[0]);
    }
    if streaming_height > 0 {
        render_streaming(frame, app, main_parts[1]);
    }
    if prompt_height > 0 {
        render_prompt_display(frame, app, main_parts[2]);
    }

    render_input(frame, app, input_area);
    render_status(frame, app, status_area);
}

/// Render the message history using a scrollable Paragraph with line-based scrolling.
fn render_chat_list(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }

    let (text, total_lines, line_ranges) = build_chat_text(app, area.width);
    let scroll_offset = app
        .scroll_offset
        .min(total_lines.saturating_sub(area.height as usize));

    let scrollbar_width = 1u16;
    let text_area = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(scrollbar_width),
        area.height,
    );
    let scrollbar_area = Rect::new(
        area.x + area.width.saturating_sub(scrollbar_width),
        area.y,
        scrollbar_width,
        area.height,
    );

    let paragraph = Paragraph::new(text).scroll((scroll_offset as u16, 0));
    frame.render_widget(paragraph, text_area);

    if total_lines > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight).thumb_symbol("█");
        let viewport_lines = area.height as usize;
        let mut scrollbar_state = ScrollbarState::new(total_lines)
            .position(scroll_offset)
            .viewport_content_length(viewport_lines);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    app.item_line_ranges = line_ranges;
    app.total_chat_lines = total_lines;
}

/// Build the full chat text and per-message line ranges for click mapping.
fn build_chat_text(app: &App, width: u16) -> (Text<'static>, usize, Vec<(usize, usize)>) {
    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut line_ranges: Vec<(usize, usize)> = Vec::new();

    for msg in &app.messages {
        let start = all_lines.len();
        if msg.collapsed && msg.expandable {
            all_lines.push(collapsed_summary_line(msg, width));
        } else {
            let lines = message_lines(msg, width);
            all_lines.extend(lines);
        }
        all_lines.push(Line::raw(""));
        let end = all_lines.len();
        line_ranges.push((start, end));
    }

    let total = all_lines.len();
    (Text::from(all_lines), total, line_ranges)
}

/// Build a 1-line summary for a collapsed message.
fn collapsed_summary_line(msg: &ChatMessage, width: u16) -> Line<'static> {
    let (icon, color) = match &msg.role {
        Role::Tool => ("▶", Color::Rgb(100, 100, 100)),
        Role::Error => ("▶", Color::Red),
        Role::System => ("▶", Color::Rgb(180, 180, 100)),
        Role::AgentTask => {
            let info = msg.agent_task.as_ref();
            let c = match info.map(|i| &i.status) {
                Some(AgentTaskStatus::Completed) => Color::Green,
                Some(AgentTaskStatus::Failed) => Color::Red,
                Some(AgentTaskStatus::PermissionRequested) => Color::Yellow,
                _ => Color::Magenta,
            };
            ("▶", c)
        }
        _ => ("▶", Color::Rgb(100, 100, 100)),
    };

    let role_label = match &msg.role {
        Role::Tool => "tool",
        Role::Error => "error",
        Role::System => "system",
        Role::AgentTask => {
            if let Some(info) = &msg.agent_task {
                if !info.agent_name.is_empty() {
                    return Line::from(vec![Span::styled(
                        format!(
                            "▶ {} ({}) ",
                            truncate_str(
                                &msg.content,
                                (width as usize).saturating_sub(info.agent_name.len() + 10)
                            ),
                            info.agent_name
                        ),
                        Style::default().fg(color),
                    )]);
                }
            }
            "agent"
        }
        _ => "?",
    };

    let truncated = truncate_str(&msg.content, width as usize);
    Line::from(vec![Span::styled(
        format!("{icon} [{role_label}] {truncated}"),
        Style::default().fg(color),
    )])
}

fn truncate_str(s: &str, max_len: usize) -> String {
    let single_line = s.lines().next().unwrap_or("");
    if single_line.len() > max_len.saturating_sub(3) {
        format!("{}...", &single_line[..max_len.saturating_sub(6).max(0)])
    } else {
        single_line.to_string()
    }
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
#[allow(dead_code)]
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
        Role::AgentTask => {
            let info = match &msg.agent_task {
                Some(i) => i,
                None => return lines,
            };
            let time = msg.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

            // Status label and color
            let (label, color) = match info.status {
                AgentTaskStatus::Started => ("[Iniciando]", Color::Magenta),
                AgentTaskStatus::Running => ("[Procesando]", Color::Magenta),
                AgentTaskStatus::Delegated => ("[Proyecto en ejecución]", Color::Magenta),
                AgentTaskStatus::Finalizing => ("[Organizando resultados]", Color::Magenta),
                AgentTaskStatus::Completed => ("[Completado]", Color::Green),
                AgentTaskStatus::PermissionRequested => ("[Necesita confirmación]", Color::Yellow),
                AgentTaskStatus::Failed => ("[Error]", Color::Red),
            };
            let header = format!("{label} {}", info.agent_name);

            lines.push(Line::from(vec![
                Span::raw("┌ "),
                Span::styled(
                    header,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(time, Style::default().fg(Color::Rgb(100, 100, 100))),
            ]));

            // Content
            let content_color = if info.status == AgentTaskStatus::Completed {
                Color::Green
            } else if info.status == AgentTaskStatus::Failed {
                Color::Red
            } else if info.status == AgentTaskStatus::PermissionRequested {
                Color::Yellow
            } else {
                Color::Rgb(200, 160, 200)
            };
            for content_line in msg.content.lines() {
                let wrapped = word_wrap_plain(content_line, w.saturating_sub(2));
                for line in wrapped {
                    lines.push(Line::from(vec![Span::styled(
                        format!("│ {line}"),
                        Style::default().fg(content_color),
                    )]));
                }
            }

            // Options for permission requests
            if info.status == AgentTaskStatus::PermissionRequested && !info.options.is_empty() {
                let opts_text = format!("Opciones: {}", info.options.join(" / "));
                for row in word_wrap_plain(&opts_text, w.saturating_sub(2)) {
                    lines.push(Line::from(vec![Span::styled(
                        format!("│ {row}"),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
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
    let dest = app.input_destination_label();

    let text = if app.input.is_empty() {
        Text::from(Line::from(vec![
            Span::styled("┌ ", Style::default().fg(Color::Rgb(100, 100, 100))),
            Span::styled(format!("[{dest}] "), Style::default().fg(Color::Cyan)),
            Span::styled(
                "Type a message... (Enter to send)",
                Style::default().fg(Color::Rgb(100, 100, 100)),
            ),
        ]))
    } else {
        let prefix = format!("│ [{dest}] ");
        let content = app.input.clone();
        let chars: Vec<char> = content.chars().collect();
        let wrap_w = width.saturating_sub(prefix.chars().count()).max(1);
        let lines: Vec<Line> = if width == 0 {
            vec![Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Rgb(100, 100, 100))),
                Span::raw(app.input.as_str()),
            ])]
        } else {
            chars
                .chunks(wrap_w)
                .enumerate()
                .map(|(i, chunk)| {
                    if i == 0 {
                        Line::from(vec![
                            Span::styled(
                                prefix.clone(),
                                Style::default().fg(Color::Rgb(100, 100, 100)),
                            ),
                            Span::raw(chunk.iter().collect::<String>()),
                        ])
                    } else {
                        Line::from(vec![Span::raw(chunk.iter().collect::<String>())])
                    }
                })
                .collect()
        };
        Text::from(lines)
    };

    frame.render_widget(Paragraph::new(text), area);

    // Always show cursor in insert mode
    {
        let dest_prefix_w = format!("│ [{dest}] ").chars().count();
        let char_pos = app.input[..app.cursor].chars().count();
        let wrap_w = (area.width as usize).saturating_sub(dest_prefix_w).max(1);
        let (row, col) = if width == 0 {
            (0u16, dest_prefix_w as u16 + char_pos as u16)
        } else {
            let line_num = char_pos / wrap_w;
            let col_in_line = char_pos % wrap_w;
            let col = if line_num == 0 {
                dest_prefix_w + col_in_line
            } else {
                col_in_line
            };
            (line_num as u16, col as u16)
        };
        frame.set_cursor_position((area.x + col, area.y + row));
    }
}

/// Status-bar label for last/forced classifier intent.
///
/// - Force active → show forced intent with 🔒 (even before a turn runs).
/// - Last classification present → `SIMPLE` / `COMPLEX` (🔒 if that turn was forced).
/// - Otherwise → `—` (no classification yet, Auto mode).
fn intent_status_label(
    last_intent: Option<Intent>,
    force: ClassifierForceMode,
    last_forced: bool,
) -> (String, Color) {
    if let Some(intent) = force.as_intent() {
        return (format!("{}🔒", intent.as_str()), Color::Yellow);
    }
    match last_intent {
        Some(intent) => {
            let label = if last_forced {
                format!("{}🔒", intent.as_str())
            } else {
                intent.as_str().to_string()
            };
            let color = match intent {
                Intent::Simple => Color::Cyan,
                Intent::Complex => Color::Magenta,
            };
            (label, color)
        }
        None => ("—".to_string(), Color::Rgb(100, 100, 100)),
    }
}

/// Render the status bar at the bottom of the viewport.
fn render_status(frame: &mut Frame, app: &mut App, area: Rect) {
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

    let force = *app.classifier_force.lock().unwrap();
    let (intent_label, intent_color) =
        intent_status_label(app.last_intent, force, app.last_intent_forced);

    // Build segments with computed x-ranges for click detection
    let mut segments: Vec<(String, super::app::StatusBarAction, Rect)> = Vec::new();
    let mut x = area.x + 1; // after the leading space

    let brand = " seneschal ";
    x += brand.len() as u16 + 1; // +1 for space

    // State label (not clickable)
    x += state_label.len() as u16 + " │ ".len() as u16;

    // TTS label (clickable)
    let tts_start = x;
    let tts_w = tts_label.len() as u16;
    segments.push((
        tts_label.to_string(),
        super::app::StatusBarAction::ToggleTts,
        Rect::new(tts_start, area.y, tts_w, 1),
    ));
    x += tts_w + " │ ".len() as u16;

    // Conv mode (possibly clickable)
    x += conv_label.len() as u16 + " │ ".len() as u16;

    // Intent label (clickable for classifier force)
    let intent_start = x;
    let intent_w = intent_label.len() as u16;
    segments.push((
        intent_label.to_string(),
        super::app::StatusBarAction::CycleClassifierForce,
        Rect::new(intent_start, area.y, intent_w, 1),
    ));
    let _ = (x, intent_w);

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
        Span::styled(intent_label, Style::default().fg(intent_color)),
        Span::raw(" │ "),
        Span::styled(
            "Ctrl+T TTS  Ctrl+M force  Ctrl+C quit",
            Style::default().fg(Color::Rgb(100, 100, 100)),
        ),
    ])]);

    let block = Block::default().style(Style::default().bg(Color::Rgb(40, 40, 50)));

    frame.render_widget(Paragraph::new(text).block(block), area);

    app.status_bar_segments = segments
        .into_iter()
        .map(|(label, action, region)| super::app::StatusBarSegment {
            label,
            action,
            region,
        })
        .collect();
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
    fn intent_badge_none_auto() {
        let (label, _) = intent_status_label(None, ClassifierForceMode::Auto, false);
        assert_eq!(label, "—");
    }

    #[test]
    fn intent_badge_last_simple() {
        let (label, _) =
            intent_status_label(Some(Intent::Simple), ClassifierForceMode::Auto, false);
        assert_eq!(label, "SIMPLE");
    }

    #[test]
    fn intent_badge_force_complex() {
        let (label, _) = intent_status_label(None, ClassifierForceMode::ForceComplex, false);
        assert_eq!(label, "COMPLEX🔒");
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

    #[test]
    fn conversation_heights_sum_to_main_h() {
        for main_h in 0..=40u16 {
            for prompt_h in 0..=6u16 {
                for streaming in [false, true] {
                    let (h, s, p) = compute_conversation_heights(main_h, prompt_h, streaming);
                    assert_eq!(
                        h + s + p,
                        main_h,
                        "main_h={main_h} prompt={prompt_h} streaming={streaming}"
                    );
                }
            }
        }
    }
}
