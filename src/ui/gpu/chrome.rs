//! Top header + bottom footer rendering for the GPU UI.
//!
//! Header layout: dim-grey labels for backend / sub / team / Reqs /
//! Uptime / Session, separated by " │ ". The Session label is the
//! click-to-copy hot-zone; `draw_header` returns its X range so the
//! mouse handler in `app::GpuApp::on_mouse_press` can hit-test against
//! it without recomputing the layout.
//!
//! Footer layout: hotkey hints flush left, version string flush right.
//!
//! Each function pushes a 1px separator rect (`CHROME_SEPARATOR_COLOR`)
//! at the boundary between chrome and the terminal panel so the user
//! sees a visible fence between regions.

use std::time::{Duration, Instant};

use term_gpu::{
    measure_label_width, push_label, FontSystem, GlyphAtlas, GlyphInstance, RectInstance, Style,
    SwashCache, TextShapeCache, Weight,
};

/// Top chrome reserved for the header — backend / Reqs / Uptime /
/// Session etc. live here. Terminal area starts immediately below.
pub(super) const HEADER_HEIGHT_LOGICAL: f32 = 30.0;

/// Bottom chrome reserved for the footer — hotkey hints + version.
/// Terminal area ends immediately above.
pub(super) const FOOTER_HEIGHT_LOGICAL: f32 = 28.0;

/// Horizontal inset (logical px) for chrome TEXT and the terminal content.
/// The bar background + separator span the full width; only text/content is
/// padded in from the edges.
pub(super) const CHROME_H_PAD: f32 = 12.0;

/// Footer hint text — all the app-level shortcuts the GPU UI honours.
const FOOTER_HINTS: &str =
    "Cmd+B: Switch │ Cmd+H: History │ Cmd+E: Settings │ Cmd+R: Restart │ Cmd+Q: Quit";

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Font size for header / footer chrome text (variable-width
/// SansSerif), in logical pixels.
const CHROME_FONT_SIZE: f32 = 14.0;

/// Opaque background fill for the chrome bars. Matches the window clear colour
/// so the bars read as part of the surface, while still covering any terminal
/// glyph that scrolls into the bar band (the bars render in the overlay layer,
/// on top of the terminal).
const CHROME_BG: [f32; 4] = [0.04, 0.04, 0.06, 1.0];

/// How long the "Session ID copied!" flash stays visible after a
/// successful copy click.
pub(super) const SESSION_COPY_FLASH: Duration = Duration::from_millis(1500);

/// Dim foreground for chrome labels. Re-exported to popup code so the
/// inactive section labels match the chrome palette.
pub(super) const CHROME_TEXT_COLOR: [f32; 4] = [0.55, 0.55, 0.55, 1.0];

/// Highlight color for the "Session ID copied!" flash. Same green
/// the legacy ratatui chrome used for STATUS_OK. Also used by popup
/// code for the `[Active]` / `[Selected]` status suffixes.
pub(super) const CHROME_FLASH_COLOR: [f32; 4] = [0.4, 0.85, 0.4, 1.0];

/// 1px separator line that visually fences the terminal panel off
/// from the header / footer chrome.
const CHROME_SEPARATOR_COLOR: [f32; 4] = [0.25, 0.25, 0.27, 1.0];

/// Draw the top header chrome. Returns the X range `(start_x, end_x)`
/// of the "Session: …" label so the caller can hit-test it on mouse
/// clicks (the click-to-copy hot-zone).
///
/// Free function (not method) so the caller can hold a `&mut renderer`
/// borrow across the call — a `&mut self` here would collide.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_header(
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    active_backend: &str,
    subagent: Option<&str>,
    teammate: Option<&str>,
    reqs: u64,
    session_id: &str,
    start_time: Instant,
    session_copied_active: bool,
    window_w_logical: f32,
    sf: f32,
) -> Option<(f32, f32)> {
    // Opaque full-width background so any terminal glyph that scrolls into the
    // header band is covered (the header renders in the overlay, on top).
    rects.push(RectInstance {
        pos: [0.0, 0.0],
        size: [window_w_logical, HEADER_HEIGHT_LOGICAL],
        color: CHROME_BG,
    });
    // 1px separator (full width) that visually delineates the header from the
    // terminal panel below.
    rects.push(RectInstance {
        pos: [0.0, HEADER_HEIGHT_LOGICAL - 1.0],
        size: [window_w_logical, 1.0],
        color: CHROME_SEPARATOR_COLOR,
    });

    let backend = active_backend;
    let sub = subagent.unwrap_or("—");
    let team = teammate.unwrap_or("—");
    let uptime_s = start_time.elapsed().as_secs();

    let sep = " │ ";
    let baseline_y = HEADER_HEIGHT_LOGICAL * 0.7;
    let mut x = CHROME_H_PAD;

    let segments: [String; 5] = [
        format!("backend: {backend}"),
        format!("sub: {sub}"),
        format!("team: {team}"),
        format!("Reqs: {reqs}"),
        format!("Uptime: {uptime_s}s"),
    ];
    for seg in &segments {
        x = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            seg,
            x,
            baseline_y,
            CHROME_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            CHROME_TEXT_COLOR,
        );
        x = push_label(
            font_system,
            swash_cache,
            atlas,
            ui_shape_cache,
            glyphs,
            sep,
            x,
            baseline_y,
            CHROME_FONT_SIZE,
            sf,
            Weight::NORMAL,
            Style::Normal,
            CHROME_TEXT_COLOR,
        );
    }

    let session_text = if session_copied_active {
        "Session ID copied!".to_string()
    } else {
        format!("Session: {session_id}")
    };
    let session_color = if session_copied_active {
        CHROME_FLASH_COLOR
    } else {
        CHROME_TEXT_COLOR
    };
    let session_start_x = x;
    let session_end_x = push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &session_text,
        x,
        baseline_y,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        session_color,
    );
    Some((session_start_x, session_end_x))
}

/// Draw the bottom footer chrome. Free function for the same reason
/// `draw_header` is — keeps the `&mut renderer` borrow viable in
/// the caller.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_footer(
    atlas: &mut GlyphAtlas,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ui_shape_cache: &mut TextShapeCache,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    window_w_logical: f32,
    window_h_logical: f32,
    sf: f32,
) {
    // Opaque full-width background so terminal glyphs that scroll into the
    // footer band are covered (the footer renders in the overlay, on top).
    rects.push(RectInstance {
        pos: [0.0, window_h_logical - FOOTER_HEIGHT_LOGICAL],
        size: [window_w_logical, FOOTER_HEIGHT_LOGICAL],
        color: CHROME_BG,
    });
    // 1px separator (full width) delineating the terminal panel from the footer.
    rects.push(RectInstance {
        pos: [0.0, window_h_logical - FOOTER_HEIGHT_LOGICAL],
        size: [window_w_logical, 1.0],
        color: CHROME_SEPARATOR_COLOR,
    });

    let baseline_y = window_h_logical - FOOTER_HEIGHT_LOGICAL * 0.3;

    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        FOOTER_HINTS,
        CHROME_H_PAD,
        baseline_y,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );

    let version_text = format!("v{APP_VERSION}");
    let version_w = measure_label_width(
        font_system,
        ui_shape_cache,
        &version_text,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
    );
    let version_x = (window_w_logical - version_w - CHROME_H_PAD).max(0.0);
    push_label(
        font_system,
        swash_cache,
        atlas,
        ui_shape_cache,
        glyphs,
        &version_text,
        version_x,
        baseline_y,
        CHROME_FONT_SIZE,
        sf,
        Weight::NORMAL,
        Style::Normal,
        CHROME_TEXT_COLOR,
    );
}
