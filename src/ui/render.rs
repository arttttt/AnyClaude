use crate::config::SettingSection;
use crate::ui::app::{App, PopupKind};
use crate::ui::components::PopupDialog;
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::history::render_history_dialog;
use crate::ui::layout::layout_regions;
use crate::ui::settings::SettingsDialogState;
use crate::ui::terminal::TerminalBody;
use crate::ui::theme::{
    ACTIVE_HIGHLIGHT, CLAUDE_ORANGE, HEADER_SEPARATOR, HEADER_TEXT, STATUS_ERROR, STATUS_OK,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use ratatui::Frame;
use std::sync::Arc;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let (header, body, footer) = layout_regions(area);

    // Resolve sub/team backend display names (fallback to active backend name)
    let active_display_name = app
        .backends()
        .iter()
        .find(|b| b.is_active)
        .map(|b| b.display_name.as_str())
        .unwrap_or("unknown");

    let resolve_name = |id: Option<&str>| -> &str {
        id.and_then(|id| {
            app.backends()
                .iter()
                .find(|b| b.id == id)
                .map(|b| b.display_name.as_str())
        })
        .unwrap_or(active_display_name)
    };

    let sub_name = resolve_name(app.subagent_backend());
    let team_name = resolve_name(app.teammate_backend());

    let header_widget = Header::new();
    frame.render_widget(
        header_widget.widget(
            app.proxy_status(),
            app.session_id(),
            app.session_copied_flash(),
            sub_name,
            team_name,
        ),
        header,
    );
    frame.render_widget(Clear, body);
    if let Some(emu) = app.emulator() {
        frame.render_widget(TerminalBody::new(Arc::clone(&emu), app.selection()), body);
        // Show hardware cursor only when:
        // - the child process has started (is_pty_ready)
        // - terminal has focus and is at live view (scrollback == 0)
        // - the child wants the cursor visible (DECTCEM)
        // Apps like Claude Code hide the hardware cursor and render their own
        // visual cursor as an inverse-styled space.
        if app.is_pty_ready() && app.focus_is_terminal() && app.scrollback() == 0 && body.width > 0 && body.height > 0 {
            let cursor = emu.lock().cursor();
            if cursor.visible {
                let x = body.x + cursor.col.min(body.width.saturating_sub(1));
                let y = body.y + cursor.row.min(body.height.saturating_sub(1));
                frame.set_cursor_position((x, y));
            }
        }
    }
    let footer_widget = Footer::new();
    frame.render_widget(footer_widget.widget(footer), footer);

    if let Some(kind) = app.popup_kind() {
        // History and Settings dialogs render themselves independently
        if matches!(kind, PopupKind::History) {
            render_history_dialog(frame, app.history_dialog());
            return;
        }
        if matches!(kind, PopupKind::Settings) {
            render_settings_dialog(frame, app.settings_dialog(), body);
            return;
        }

        let (title, lines) = match kind {
            PopupKind::BackendSwitch => {
                use crate::ui::app::BackendPopupSection;
                let mut lines = Vec::new();
                let active_section = app.backend_popup_section();

                // Helper to format backend list
                let format_backend_list = |backends: &[crate::ipc::BackendInfo], selected_idx: usize, active_section: bool| -> Vec<Line<'static>> {
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

                        let mut spans = Vec::new();
                        let prefix = if is_selected {
                            format!("  → {}. ", idx + 1)
                        } else {
                            format!("    {}. ", idx + 1)
                        };
                        spans.push(Span::styled(prefix, base_style.fg(HEADER_TEXT)));
                        spans.push(Span::styled(
                            format!("{:<width$}", backend.display_name, width = max_name_width),
                            base_style.fg(HEADER_TEXT),
                        ));
                        spans.push(Span::styled("  [", base_style));
                        spans.push(Span::styled(status_text, base_style.fg(status_color)));
                        spans.push(Span::styled("]", base_style));

                        result.push(Line::from(spans));
                    }
                    result
                };

                // --- Active Backend Section ---
                let active_header_marker = if active_section == BackendPopupSection::ActiveBackend { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}Active Backend", active_header_marker), Style::default().bold()),
                ]));
                lines.push(Line::from("  ─────────────────"));
                lines.extend(format_backend_list(
                    app.backends(),
                    app.backend_selection(),
                    active_section == BackendPopupSection::ActiveBackend,
                ));

                // --- Subagent Backend Section ---
                lines.push(Line::from(""));
                let subagent_header_marker = if active_section == BackendPopupSection::SubagentBackend { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}Subagent Backend", subagent_header_marker), Style::default().bold()),
                ]));
                lines.push(Line::from("  ─────────────────"));

                // Always show backend list; first item is "Disabled" (inherit).
                // subagent_selection: 0 = Disabled, 1..N = backends
                let subagent_backend = app.subagent_backend();
                let in_section = active_section == BackendPopupSection::SubagentBackend;
                let sel = app.subagent_selection();

                // Item 0: Disabled (use active backend)
                {
                    let is_selected = in_section && sel == 0;
                    let is_current = subagent_backend.is_none();
                    let base_style = if is_selected {
                        Style::default().bg(ACTIVE_HIGHLIGHT)
                    } else {
                        Style::default()
                    };
                    let prefix = if is_selected { "  → " } else { "    " };
                    let mut spans = vec![
                        Span::styled(format!("{}Disabled (use active backend)", prefix), base_style.fg(HEADER_TEXT)),
                    ];
                    if is_current {
                        spans.push(Span::styled("  [", base_style));
                        spans.push(Span::styled("Active", base_style.fg(STATUS_OK)));
                        spans.push(Span::styled("]", base_style));
                    }
                    lines.push(Line::from(spans));
                }

                let max_name_width = app.backends()
                    .iter()
                    .map(|b| b.display_name.chars().count())
                    .max()
                    .unwrap_or(0);

                // Items 1..N: backends
                for (idx, backend) in app.backends().iter().enumerate() {
                    let item_index = idx + 1; // offset by 1 because 0 = Disabled
                    let is_selected = in_section && sel == item_index;
                    let is_current = subagent_backend == Some(backend.id.as_str());

                    let base_style = if is_selected {
                        Style::default().bg(ACTIVE_HIGHLIGHT)
                    } else {
                        Style::default()
                    };

                    let mut spans = Vec::new();
                    let prefix = if is_selected {
                        format!("  → {}. ", idx + 1)
                    } else {
                        format!("    {}. ", idx + 1)
                    };
                    spans.push(Span::styled(prefix, base_style.fg(HEADER_TEXT)));
                    spans.push(Span::styled(
                        format!("{:<width$}", backend.display_name, width = max_name_width),
                        base_style.fg(HEADER_TEXT),
                    ));
                    if is_current {
                        spans.push(Span::styled("  [", base_style));
                        spans.push(Span::styled("Selected", base_style.fg(STATUS_OK)));
                        spans.push(Span::styled("]", base_style));
                    }

                    lines.push(Line::from(spans));
                }

                if let Some(error) = app.last_ipc_error() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(format!("    IPC error: {error}")));
                }

                // --- Teammate Backend Section ---
                lines.push(Line::from(""));
                let teammate_header_marker = if active_section == BackendPopupSection::TeammateBackend { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}Teammate Backend", teammate_header_marker), Style::default().bold()),
                ]));
                lines.push(Line::from("  ─────────────────"));

                let teammate_backend = app.teammate_backend();
                let in_teammate_section = active_section == BackendPopupSection::TeammateBackend;
                let teammate_sel = app.teammate_selection();

                // Item 0: Disabled (use active backend)
                {
                    let is_selected = in_teammate_section && teammate_sel == 0;
                    let is_current = teammate_backend.is_none();
                    let base_style = if is_selected {
                        Style::default().bg(ACTIVE_HIGHLIGHT)
                    } else {
                        Style::default()
                    };
                    let prefix = if is_selected { "  → " } else { "    " };
                    let mut spans = vec![
                        Span::styled(format!("{}Disabled (use active backend)", prefix), base_style.fg(HEADER_TEXT)),
                    ];
                    if is_current {
                        spans.push(Span::styled("  [", base_style));
                        spans.push(Span::styled("Active", base_style.fg(STATUS_OK)));
                        spans.push(Span::styled("]", base_style));
                    }
                    lines.push(Line::from(spans));
                }

                // Items 1..N: backends
                for (idx, backend) in app.backends().iter().enumerate() {
                    let item_index = idx + 1;
                    let is_selected = in_teammate_section && teammate_sel == item_index;
                    let is_current = teammate_backend == Some(backend.id.as_str());

                    let base_style = if is_selected {
                        Style::default().bg(ACTIVE_HIGHLIGHT)
                    } else {
                        Style::default()
                    };

                    let mut spans = Vec::new();
                    let prefix = if is_selected {
                        format!("  → {}. ", idx + 1)
                    } else {
                        format!("    {}. ", idx + 1)
                    };
                    spans.push(Span::styled(prefix, base_style.fg(HEADER_TEXT)));
                    spans.push(Span::styled(
                        format!("{:<width$}", backend.display_name, width = max_name_width),
                        base_style.fg(HEADER_TEXT),
                    ));
                    if is_current {
                        spans.push(Span::styled("  [", base_style));
                        spans.push(Span::styled("Selected", base_style.fg(STATUS_OK)));
                        spans.push(Span::styled("]", base_style));
                    }

                    lines.push(Line::from(spans));
                }

                ("Select Backend", lines)
            }
            PopupKind::History | PopupKind::Settings => unreachable!("handled above"),
        };

        let dialog = PopupDialog::new(title, lines)
            .min_width(60)
            .footer("Tab: Section  Up/Down: Move  Enter: Select  Del: Clear  Esc: Close");
        dialog.render(frame, body);

    }
}

fn render_settings_dialog(
    frame: &mut Frame<'_>,
    state: &SettingsDialogState,
    body: ratatui::layout::Rect,
) {
    let SettingsDialogState::Visible {
        fields,
        focused,
        dirty,
        confirm_discard,
    } = state
    else {
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut current_section: Option<SettingSection> = None;

    for (idx, field) in fields.iter().enumerate() {
        // Section header
        if current_section != Some(field.section) {
            if current_section.is_some() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                format!("  ── {} ──", field.section.label()),
                Style::default().fg(CLAUDE_ORANGE),
            )));
            current_section = Some(field.section);
        }

        let is_focused = idx == *focused;
        let checkbox = if field.value { "[x]" } else { "[ ]" };
        let prefix = if is_focused { "  → " } else { "    " };

        let base_style = if is_focused {
            Style::default().bg(ACTIVE_HIGHLIGHT)
        } else {
            Style::default()
        };

        let check_color = if field.value { STATUS_OK } else { HEADER_TEXT };

        lines.push(Line::from(vec![
            Span::styled(prefix, base_style.fg(HEADER_TEXT)),
            Span::styled(checkbox, base_style.fg(check_color)),
            Span::styled(format!(" {}", field.label), base_style.fg(HEADER_TEXT)),
        ]));

        // Description as a dim line below the setting
        lines.push(Line::from(Span::styled(
            format!("      {}", field.description),
            Style::default().fg(HEADER_SEPARATOR),
        )));
    }

    let title = if *dirty { "Settings *" } else { "Settings" };

    let footer = if *confirm_discard {
        "Unsaved changes! Esc: Discard  Enter: Apply"
    } else {
        "Space: Toggle  Enter: Apply  Esc: Cancel"
    };

    PopupDialog::new(title, lines)
        .min_width(50)
        .footer(footer)
        .render(frame, body);
}
