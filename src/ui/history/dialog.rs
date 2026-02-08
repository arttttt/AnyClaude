use crate::ui::components::PopupDialog;
use crate::ui::history::reducer::MAX_VISIBLE_ROWS;
use crate::ui::history::state::HistoryDialogState;
use crate::ui::theme::{HEADER_TEXT, POPUP_BORDER};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use std::time::SystemTime;

const DIALOG_WIDTH: u16 = 50;

pub fn render_history_dialog(frame: &mut Frame, state: &HistoryDialogState) {
    let HistoryDialogState::Visible {
        entries,
        scroll_offset,
    } = state
    else {
        return;
    };

    if entries.is_empty() {
        return;
    }

    let can_scroll = entries.len() > MAX_VISIBLE_ROWS;
    // Subtract borders; when scrollbar present, reserve 1 column for it (0 gap cells —
    // the narrow centered ┃ char provides ~0.25-cell visual gaps on each side).
    let inner_width = DIALOG_WIDTH.saturating_sub(if can_scroll { 3 } else { 2 }) as usize;

    let lines: Vec<Line> = entries
        .iter()
        .skip(*scroll_offset)
        .take(MAX_VISIBLE_ROWS)
        .map(|entry| {
            let description = match &entry.from_backend {
                None => format!("Started on {}", entry.to_backend),
                Some(from) => format!("{} → {}", from, entry.to_backend),
            };
            let time = format_time(entry.timestamp);
            let margins = if can_scroll { 1 } else { 2 };
            let padding = inner_width
                .saturating_sub(description.chars().count())
                .saturating_sub(time.len())
                .saturating_sub(margins);
            let mut spans = vec![
                Span::styled(" ", Style::default()),
                Span::styled(description, Style::default().fg(HEADER_TEXT)),
                Span::styled(" ".repeat(padding.max(1)), Style::default()),
                Span::styled(time, Style::default().fg(POPUP_BORDER)),
            ];
            if !can_scroll {
                spans.push(Span::styled(" ", Style::default()));
            }
            Line::from(spans)
        })
        .collect();

    let mut dialog = PopupDialog::new("Backend History", lines)
        .fixed_width(DIALOG_WIDTH)
        .scrollbar(entries.len(), *scroll_offset);
    dialog = dialog.footer(if can_scroll {
        "Up/Down: Scroll  Esc/Ctrl+H: Close"
    } else {
        "Esc/Ctrl+H: Close"
    });
    dialog.render(frame, frame.area());
}

fn format_time(timestamp: SystemTime) -> String {
    let duration = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let local_offset = chrono_local_offset_secs();
    let local_secs = secs as i64 + local_offset;
    let local_secs = local_secs.rem_euclid(86400) as u64;
    let h = local_secs / 3600;
    let m = (local_secs / 60) % 60;
    let s = local_secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// Get the local timezone offset in seconds from UTC.
fn chrono_local_offset_secs() -> i64 {
    // Use libc to get timezone offset without adding chrono dependency
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm = MaybeUninit::<libc::tm>::uninit();
            libc::localtime_r(&now, tm.as_mut_ptr());
            (*tm.as_ptr()).tm_gmtoff
        }
    }
    #[cfg(not(unix))]
    {
        0 // Fallback to UTC on non-unix platforms
    }
}
