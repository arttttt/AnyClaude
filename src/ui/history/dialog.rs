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

    let inner_width = DIALOG_WIDTH.saturating_sub(2) as usize; // subtract borders
    let can_scroll = entries.len() > MAX_VISIBLE_ROWS;

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
            let padding = inner_width
                .saturating_sub(description.chars().count())
                .saturating_sub(time.len())
                .saturating_sub(2); // 1 char margin each side
            Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(description, Style::default().fg(HEADER_TEXT)),
                Span::styled(" ".repeat(padding.max(1)), Style::default()),
                Span::styled(time, Style::default().fg(POPUP_BORDER)),
                Span::styled(" ", Style::default()),
            ])
        })
        .collect();

    let content_rows = lines.len();

    let mut dialog = PopupDialog::new("Backend History", lines)
        .fixed_width(DIALOG_WIDTH);
    dialog = dialog.footer(if can_scroll {
        "Up/Down: Scroll  Esc/Ctrl+H: Close"
    } else {
        "Esc/Ctrl+H: Close"
    });
    let rect = dialog.render(frame, frame.area());

    // Custom scrollbar — ratatui's Scrollbar rounds start/end independently,
    // causing ±1 thumb size jitter. Draw manually for constant thumb size.
    if can_scroll {
        let max_offset = entries.len().saturating_sub(MAX_VISIBLE_ROWS);
        let track = content_rows;
        let thumb_size = (track * MAX_VISIBLE_ROWS / entries.len()).max(1);
        let thumb_start = if max_offset > 0 {
            *scroll_offset * (track - thumb_size) / max_offset
        } else {
            0
        };

        let x = rect.x + rect.width - 2; // inside border
        let y_base = rect.y + 1; // skip top border
        let buf = frame.buffer_mut();
        for i in 0..track {
            let cell = &mut buf[(x, y_base + i as u16)];
            if i >= thumb_start && i < thumb_start + thumb_size {
                cell.set_char('█');
                cell.set_style(Style::default().fg(HEADER_TEXT));
            }
        }
    }
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
