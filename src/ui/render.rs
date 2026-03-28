use crate::config::SettingSection;
use crate::ui::app::{App, PopupKind};
use crate::ui::backend_switch::render_backend_switch_dialog;
use crate::ui::components::PopupDialog;
use crate::ui::footer::Footer;
use crate::ui::header::Header;
use crate::ui::history::render_history_dialog;
use crate::ui::layout::layout_regions;
use crate::ui::settings::SettingsDialogState;
use crate::ui::terminal::TerminalBody;
use crate::ui::theme::{
    ACTIVE_HIGHLIGHT, CLAUDE_ORANGE, HEADER_SEPARATOR, HEADER_TEXT, STATUS_OK,
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
        match kind {
            PopupKind::History => {
                render_history_dialog(frame, app.history_dialog());
            }
            PopupKind::Settings => {
                render_settings_dialog(frame, app.settings_dialog(), body);
            }
            PopupKind::BackendSwitch => {
                render_backend_switch_dialog(
                    frame,
                    app.backend_switch(),
                    app.backends(),
                    app.subagent_backend(),
                    app.teammate_backend(),
                    app.last_ipc_error(),
                    body,
                );
            }
        }
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
