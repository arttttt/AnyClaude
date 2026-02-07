//! Dialog rendering for the summarization overlay.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::ui::theme::{ACTIVE_HIGHLIGHT, HEADER_TEXT, POPUP_BORDER, STATUS_ERROR, STATUS_OK};

use super::state::{SummarizeDialogState, MAX_AUTO_RETRIES};

/// Spinner animation frames.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Width of the summarization dialog.
const DIALOG_WIDTH: u16 = 50;

/// Height of the summarization dialog (varies by state).
fn dialog_height(state: &SummarizeDialogState) -> u16 {
    match state {
        SummarizeDialogState::Hidden => 0,
        SummarizeDialogState::Summarizing { .. } => 5,
        SummarizeDialogState::Retrying { .. } => 6,
        SummarizeDialogState::Failed { .. } => 8,
    }
}

/// Render the summarization dialog overlay.
///
/// This should be rendered on top of the backend selection popup.
/// `countdown_secs` is the time remaining until next auto-retry (for display).
pub fn render_summarize_dialog(
    frame: &mut Frame,
    state: &SummarizeDialogState,
    selected_button: u8,
    countdown_secs: Option<u64>,
) {
    if !state.is_visible() {
        return;
    }

    let height = dialog_height(state);
    let area = centered_rect(DIALOG_WIDTH, height, frame.area());

    // Clear the area behind the dialog
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Summarizing Session ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(POPUP_BORDER));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match state {
        SummarizeDialogState::Hidden => {}

        SummarizeDialogState::Summarizing { animation_tick } => {
            render_progress(frame, inner, "Summarizing session...", *animation_tick);
        }

        SummarizeDialogState::Retrying {
            attempt,
            error,
            animation_tick,
        } => {
            render_retrying(frame, inner, error, *attempt, *animation_tick, countdown_secs);
        }

        SummarizeDialogState::Failed { error } => {
            render_failed(frame, inner, error, selected_button);
        }
    }
}

/// Render the progress spinner state.
fn render_progress(frame: &mut Frame, area: Rect, message: &str, animation_tick: u8) {
    let spinner = SPINNER_FRAMES[(animation_tick as usize) % SPINNER_FRAMES.len()];

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("  {} ", spinner), Style::default().fg(STATUS_OK)),
            Span::styled(message, Style::default().fg(HEADER_TEXT)),
        ]),
    ];

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render the retrying state.
fn render_retrying(
    frame: &mut Frame,
    area: Rect,
    error: &str,
    attempt: u8,
    animation_tick: u8,
    countdown_secs: Option<u64>,
) {
    let spinner = SPINNER_FRAMES[(animation_tick as usize) % SPINNER_FRAMES.len()];

    let countdown_text = match countdown_secs {
        Some(0) | None => "now".to_string(),
        Some(secs) => format!("in {}s", secs),
    };

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Error: ", Style::default().fg(STATUS_ERROR)),
            Span::styled(truncate_error(error, 35), Style::default().fg(HEADER_TEXT)),
        ]),
        Line::from(vec![
            Span::styled(format!("  {} ", spinner), Style::default().fg(STATUS_OK)),
            Span::styled(
                format!("Retrying ({}/{}) {}...", attempt, MAX_AUTO_RETRIES, countdown_text),
                Style::default().fg(HEADER_TEXT),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render the failed state with action buttons.
fn render_failed(frame: &mut Frame, area: Rect, error: &str, selected_button: u8) {
    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("  Summarization failed after {} attempts", MAX_AUTO_RETRIES),
            Style::default().fg(STATUS_ERROR),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Error: ", Style::default().fg(STATUS_ERROR)),
            Span::styled(truncate_error(error, 35), Style::default().fg(HEADER_TEXT)),
        ]),
        Line::from(""),
        render_buttons(selected_button),
    ];

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render the Retry/Cancel buttons.
fn render_buttons(selected: u8) -> Line<'static> {
    let retry_style = if selected == 0 {
        Style::default()
            .fg(HEADER_TEXT)
            .bg(ACTIVE_HIGHLIGHT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(HEADER_TEXT)
    };

    let cancel_style = if selected == 1 {
        Style::default()
            .fg(HEADER_TEXT)
            .bg(ACTIVE_HIGHLIGHT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(HEADER_TEXT)
    };

    Line::from(vec![
        Span::raw("          "),
        Span::styled(" Retry ", retry_style),
        Span::raw("    "),
        Span::styled(" Cancel ", cancel_style),
    ])
}

/// Truncate error message to fit in the dialog.
fn truncate_error(error: &str, max_len: usize) -> String {
    if error.len() <= max_len {
        error.to_string()
    } else {
        format!("{}...", &error[..max_len.saturating_sub(3)])
    }
}

/// Create a centered rect of given size.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1]);

    horizontal[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_height_varies_by_state() {
        assert_eq!(dialog_height(&SummarizeDialogState::Hidden), 0);
        assert_eq!(
            dialog_height(&SummarizeDialogState::Summarizing { animation_tick: 0 }),
            5
        );
        assert_eq!(
            dialog_height(&SummarizeDialogState::Failed {
                error: "test".into()
            }),
            8
        );
    }

    #[test]
    fn truncate_error_works() {
        assert_eq!(truncate_error("short", 10), "short");
        assert_eq!(truncate_error("this is a very long error message", 15), "this is a ve...");
    }
}
