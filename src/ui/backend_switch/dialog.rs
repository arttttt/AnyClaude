use crate::ipc::BackendInfo;
use crate::ui::backend_switch::state::{BackendPopupSection, BackendSwitchState};
use crate::ui::components::PopupDialog;
use crate::ui::theme::{ACTIVE_HIGHLIGHT, HEADER_TEXT, STATUS_ERROR, STATUS_OK};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

pub fn render_backend_switch_dialog(
    frame: &mut Frame,
    state: &BackendSwitchState,
    backends: &[BackendInfo],
    subagent_backend: Option<&str>,
    teammate_backend: Option<&str>,
    last_ipc_error: Option<&str>,
    body: ratatui::layout::Rect,
) {
    let BackendSwitchState::Visible {
        section: active_section,
        backend_selection,
        subagent_selection,
        teammate_selection,
        ..
    } = state
    else {
        return;
    };

    let mut lines = Vec::new();

    // --- Active Backend Section ---
    let active_header_marker = if *active_section == BackendPopupSection::ActiveBackend {
        "▸ "
    } else {
        "  "
    };
    lines.push(Line::from(vec![Span::styled(
        format!("{active_header_marker}Active Backend"),
        Style::default().bold(),
    )]));
    lines.push(Line::from("  ─────────────────"));
    lines.extend(format_backend_list(
        backends,
        *backend_selection,
        *active_section == BackendPopupSection::ActiveBackend,
    ));

    // --- Subagent Backend Section ---
    lines.push(Line::from(""));
    let subagent_header_marker = if *active_section == BackendPopupSection::SubagentBackend {
        "▸ "
    } else {
        "  "
    };
    lines.push(Line::from(vec![Span::styled(
        format!("{subagent_header_marker}Subagent Backend"),
        Style::default().bold(),
    )]));
    lines.push(Line::from("  ─────────────────"));

    let max_name_width = backends
        .iter()
        .map(|b| b.display_name.chars().count())
        .max()
        .unwrap_or(0);

    render_override_section(
        &mut lines,
        backends,
        *subagent_selection,
        subagent_backend,
        *active_section == BackendPopupSection::SubagentBackend,
        max_name_width,
    );

    if let Some(error) = last_ipc_error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("    IPC error: {error}")));
    }

    // --- Teammate Backend Section ---
    lines.push(Line::from(""));
    let teammate_header_marker = if *active_section == BackendPopupSection::TeammateBackend {
        "▸ "
    } else {
        "  "
    };
    lines.push(Line::from(vec![Span::styled(
        format!("{teammate_header_marker}Teammate Backend"),
        Style::default().bold(),
    )]));
    lines.push(Line::from("  ─────────────────"));

    render_override_section(
        &mut lines,
        backends,
        *teammate_selection,
        teammate_backend,
        *active_section == BackendPopupSection::TeammateBackend,
        max_name_width,
    );

    let dialog = PopupDialog::new("Select Backend", lines)
        .min_width(60)
        .footer("Tab: Section  Up/Down: Move  Enter: Select  Del: Clear  Esc: Close");
    dialog.render(frame, body);
}

/// Render the active backend list with status indicators.
fn format_backend_list<'a>(
    backends: &[BackendInfo],
    selected_idx: usize,
    active_section: bool,
) -> Vec<Line<'a>> {
    if backends.is_empty() {
        return vec![Line::from("    No backends available.")];
    }

    let max_name_width = backends
        .iter()
        .map(|b| b.display_name.chars().count())
        .max()
        .unwrap_or(0);

    let mut result = Vec::new();
    for (idx, backend) in backends.iter().enumerate() {
        let (status_text, status_color) = if backend.is_active {
            ("Active", STATUS_OK)
        } else if backend.is_configured {
            ("Ready", STATUS_OK)
        } else {
            ("Missing", STATUS_ERROR)
        };
        let is_selected = active_section && idx == selected_idx;

        let base_style = if is_selected {
            Style::default().bg(ACTIVE_HIGHLIGHT)
        } else {
            Style::default()
        };

        let prefix = if is_selected {
            format!("  → {}. ", idx + 1)
        } else {
            format!("    {}. ", idx + 1)
        };

        let spans = vec![
            Span::styled(prefix, base_style.fg(HEADER_TEXT)),
            Span::styled(
                format!("{:<width$}", backend.display_name, width = max_name_width),
                base_style.fg(HEADER_TEXT),
            ),
            Span::styled("  [", base_style),
            Span::styled(status_text, base_style.fg(status_color)),
            Span::styled("]", base_style),
        ];

        result.push(Line::from(spans));
    }
    result
}

/// Render a subagent/teammate override section (Disabled + backend list).
fn render_override_section(
    lines: &mut Vec<Line<'_>>,
    backends: &[BackendInfo],
    selection: usize,
    current_backend: Option<&str>,
    in_section: bool,
    max_name_width: usize,
) {
    // Item 0: Disabled (use active backend)
    {
        let is_selected = in_section && selection == 0;
        let is_current = current_backend.is_none();
        let base_style = if is_selected {
            Style::default().bg(ACTIVE_HIGHLIGHT)
        } else {
            Style::default()
        };
        let prefix = if is_selected { "  → " } else { "    " };
        let mut spans = vec![Span::styled(
            format!("{prefix}Disabled (use active backend)"),
            base_style.fg(HEADER_TEXT),
        )];
        if is_current {
            spans.push(Span::styled("  [", base_style));
            spans.push(Span::styled("Active", base_style.fg(STATUS_OK)));
            spans.push(Span::styled("]", base_style));
        }
        lines.push(Line::from(spans));
    }

    // Items 1..N: backends
    for (idx, backend) in backends.iter().enumerate() {
        let item_index = idx + 1; // offset by 1 because 0 = Disabled
        let is_selected = in_section && selection == item_index;
        let is_current = current_backend == Some(backend.id.as_str());

        let base_style = if is_selected {
            Style::default().bg(ACTIVE_HIGHLIGHT)
        } else {
            Style::default()
        };

        let prefix = if is_selected {
            format!("  → {}. ", idx + 1)
        } else {
            format!("    {}. ", idx + 1)
        };

        let mut spans = vec![
            Span::styled(prefix, base_style.fg(HEADER_TEXT)),
            Span::styled(
                format!("{:<width$}", backend.display_name, width = max_name_width),
                base_style.fg(HEADER_TEXT),
            ),
        ];
        if is_current {
            spans.push(Span::styled("  [", base_style));
            spans.push(Span::styled("Selected", base_style.fg(STATUS_OK)));
            spans.push(Span::styled("]", base_style));
        }

        lines.push(Line::from(spans));
    }
}
