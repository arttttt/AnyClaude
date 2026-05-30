//! Chrome dimensions + palette shared by the GPU UI.
//!
//! The header / footer are now rendered as a term_ui view
//! (`ui::chrome_labels::chrome_view`, reconciled + painted in
//! `GpuApp::redraw`); this module is reduced to the shared constants the
//! coordinator and the popup overlay still reference. The old immediate-mode
//! `draw_header` / `draw_footer` were removed when the chrome moved to term_ui
//! (Phase E.6).

use std::time::Duration;

/// Top chrome reserved for the header. Terminal area starts immediately below.
pub(super) const HEADER_HEIGHT_LOGICAL: f32 = 30.0;

/// Bottom chrome reserved for the footer. Terminal area ends immediately above.
pub(super) const FOOTER_HEIGHT_LOGICAL: f32 = 28.0;

/// Horizontal inset (logical px) for chrome text + the terminal content. The
/// bar backgrounds + separators span the full width; only text/content is
/// padded in from the edges.
pub(super) const CHROME_H_PAD: f32 = 12.0;

/// Font size for chrome text (variable-width SansSerif), in logical pixels.
pub(super) const CHROME_FONT_SIZE: f32 = 14.0;

/// How long the "Session ID copied!" flash stays visible after a copy click.
pub(super) const SESSION_COPY_FLASH: Duration = Duration::from_millis(1500);
