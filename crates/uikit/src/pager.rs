//! A horizontal **pager**: shows one full-size page at a time and slides
//! horizontally between them, with a bottom indicator strip (‹ dots ›). Built
//! entirely from term_ui primitives — no new engine machinery:
//!
//! - the windowed pages (`current ± 1`) sit side by side in an `hstack`, each
//!   one viewport wide (`Sizing::Fixed(page_w)`, `Stretch` for full height);
//! - the whole track is shifted by `Mod::Offset` so the animated `scroll`
//!   position lands at the viewport — page `i` ends up at `(i - scroll)·page_w`;
//! - the track is wrapped in `Mod::Clip`, so the off-edge neighbours are clipped
//!   to the viewport instead of bleeding over the surrounding content.
//!
//! This is the web-carousel "translate the track" model (Swiper / embla): one
//! continuous `scroll` float is the source of truth and the page index is
//! derived. The animated `scroll` lives in the host (a `term_ui::Animation<f32>`
//! that chases the focused index, like the panel-width tween); the pager view is
//! a pure function of it. Only a `current ± 1` window is built into the tree —
//! enough for the slide, cheap for expensive pages.
//!
//! Domain-agnostic: the host supplies the page elements, the palette, and a base
//! `WidgetId` from which the strip's hit-test ids are derived.

use term_ui::{BoxView, CrossAxis, MainAxis, Modifier, Modify, Sizing, Stack, Text, WidgetId};

/// Id-path segments under the pager's `base_id` for the strip's hit targets.
const SEG_PREV: u64 = 1;
const SEG_NEXT: u64 = 2;
const SEG_DOT: u64 = 3;

/// Stable id of the "previous page" arrow (`‹`), for the host to hit-test.
pub fn pager_prev_id(base: WidgetId) -> WidgetId {
    base.child(SEG_PREV)
}

/// Stable id of the "next page" arrow (`›`), for the host to hit-test.
pub fn pager_next_id(base: WidgetId) -> WidgetId {
    base.child(SEG_NEXT)
}

/// Stable id of the `i`-th page dot, for the host to hit-test (jump to page).
pub fn pager_dot_id(base: WidgetId, i: usize) -> WidgetId {
    base.child(SEG_DOT).child(i as u64)
}

/// Colours for the [`pager`] indicator strip.
#[derive(Clone, Copy)]
pub struct PagerPalette {
    /// The filled dot for the current page.
    pub dot_current: [f32; 4],
    /// The hollow dots for the other pages.
    pub dot_idle: [f32; 4],
    /// The `‹` / `›` navigation arrows.
    pub arrow: [f32; 4],
}

/// Build a horizontal pager view.
///
/// - `pages`: the page elements in order; the pager builds only the `current ± 1`
///   window into the tree (out-of-window pages are dropped).
/// - `current`: the crisp focused index — drives the filled dot.
/// - `scroll`: the animated continuous position in page units (`current` when
///   settled); page `i` is drawn at `(i - scroll)·page_w`.
/// - `page_w`: the viewport width the host allots (logical px) — each page is
///   this wide and the slide offset is in these units.
/// - `strip_height`: height of the bottom indicator strip (logical px).
/// - `font_size`: size of the arrows / dots glyphs.
/// - `base_id`: hit-test ids for the arrows + dots are derived from this.
///
/// Returns the pager root (a vstack of `[clipped page viewport, indicator
/// strip]`); the host sizes it to the overlay rect (`tight`) and places it.
#[allow(clippy::too_many_arguments)]
pub fn pager(
    pages: Vec<BoxView>,
    current: usize,
    scroll: f32,
    page_w: f32,
    strip_height: f32,
    font_size: f32,
    palette: PagerPalette,
    base_id: WidgetId,
) -> Stack {
    let n = pages.len();
    let current = current.min(n.saturating_sub(1));
    // The built window: current ± 1, clamped to valid indices.
    let first = current.saturating_sub(1);
    let last = (current + 1).min(n.saturating_sub(1));

    // The track: the windowed pages side by side, each one viewport wide;
    // `Stretch` gives each the full viewport height. Out-of-window pages are
    // consumed and dropped (never built into the tree).
    let mut track = Stack::hstack().cross(CrossAxis::Stretch);
    for (i, page) in pages.into_iter().enumerate() {
        if (first..=last).contains(&i) {
            track = track.child_boxed(page, Sizing::Fixed(page_w));
        }
    }

    // Translate the track so page `current` sits at the viewport: a windowed
    // page at global index `g` is at local `(g - first)·page_w`; shifting the
    // track by `(first - scroll)·page_w` lands it at `(g - scroll)·page_w`.
    let track_x = (first as f32 - scroll) * page_w;
    let viewport = track
        .modify(Modifier::new().offset(track_x, 0.0))
        .modify(Modifier::new().clip());

    Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(viewport, Sizing::Fill)
        .child_sized(
            indicator_strip(n, current, font_size, palette, base_id),
            Sizing::Fixed(strip_height),
        )
}

/// The bottom strip: `‹  ● ○ ○  ›` — a centred row of the prev arrow, one dot
/// per page (filled for `current`, hollow otherwise), and the next arrow. Each
/// is tagged so the host can hit-test clicks (page back / forward / jump).
fn indicator_strip(
    n: usize,
    current: usize,
    font_size: f32,
    palette: PagerPalette,
    base_id: WidgetId,
) -> Stack {
    let mut row = Stack::hstack()
        .main(MainAxis::Center)
        .cross(CrossAxis::Center)
        .gap(6.0)
        .child(Text::new("‹", font_size, palette.arrow).id(pager_prev_id(base_id)));
    for i in 0..n {
        let (glyph, color) = if i == current {
            ("●", palette.dot_current)
        } else {
            ("○", palette.dot_idle)
        };
        row = row.child(Text::new(glyph, font_size, color).id(pager_dot_id(base_id, i)));
    }
    row.child(Text::new("›", font_size, palette.arrow).id(pager_next_id(base_id)))
}
