//! Bridge between `term_core`'s VT grid and the GPU rendering buffers.
//!
//! `populate_panel` walks a `RenderSnapshot` and emits the rectangles
//! and glyph instances that the renderer feeds to wgpu. `build_cursor_rect`
//! produces the cursor's rect for the same coordinate system.
//!
//! This is the ONE term_gpu module that knows about VT-grid concepts.
//! The rest of the crate (atlas, text shaping, instance encoding) is
//! VT-agnostic — keeping the coupling here makes it explicit and easy
//! to swap a different grid representation in the future.

use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, Style, SwashCache, Weight};
use term_core::{AnsiPalette, CellFlags, CursorState, CursorStyle, RenderSnapshot, TermColor};

use crate::{rasterize_glyph, GlyphAtlas, GlyphInstance, RectInstance, TextShapeCache};

/// Default foreground color for cells that have not specified one.
pub const DEFAULT_FG: [f32; 4] = [0.78, 0.78, 0.78, 1.0];

/// Cursor block fill / stroke color.
pub const CURSOR_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.55];

/// Thickness of the underline / beam cursor variants, in physical pixels.
pub const CURSOR_STROKE_PHYSICAL: f32 = 2.0;

/// A rectangle in *logical* (scale-factor-divided) pixels, used to bound
/// a single rendered panel inside a window. Top-left origin.
#[derive(Debug, Clone, Copy)]
pub struct PanelRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PanelRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

/// Physical pixel dimensions of one terminal cell at the active font
/// size and scale factor. Width comes from glyph advance for "M";
/// height from `font_size * line_height_ratio`.
#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub width_physical: f32,
    pub height_physical: f32,
}

/// Measure a cell's physical pixel dimensions by shaping a reference
/// "M" glyph. Cached at the call site — callers should invalidate when
/// font size or scale factor change.
pub fn measure_cell_metrics(
    font_system: &mut FontSystem,
    shape_cache: &mut TextShapeCache,
    font_size: f32,
    scale_factor: f32,
    line_height_ratio: f32,
) -> CellMetrics {
    let shaped = shape_cache.shape(
        font_system,
        "M",
        font_size,
        scale_factor,
        None,
        Weight::NORMAL,
        Style::Normal,
    );
    let width_physical = shaped
        .lines
        .first()
        .and_then(|line| line.glyphs.first())
        .map(|g| g.w)
        .unwrap_or(font_size * 0.6 * scale_factor)
        .round()
        .max(1.0);
    let height_physical = (font_size * line_height_ratio * scale_factor)
        .round()
        .max(1.0);
    CellMetrics {
        width_physical,
        height_physical,
    }
}

/// Walk every visible row in `snapshot` and append the corresponding
/// background rectangles, foreground glyphs, and SGR decoration lines
/// to `rects` / `glyphs`. The buffer is bottom-anchored: with
/// `scroll_offset_y_logical == 0` the live region sits at the bottom
/// of `panel_rect`; positive offsets reveal scrollback above.
///
/// Bound the cell grid to `panel_rect` so a panel resize mid-frame
/// (BSP divider drag) doesn't leak glyphs into a neighbour.
///
/// Fast vs slow path per cell: cells without combining marks go through
/// `TextShapeCache::shape_char` (direct cmap, no `String` alloc);
/// combining clusters fall through to `TextShapeCache::shape`.
#[allow(clippy::too_many_arguments)]
pub fn populate_panel(
    snapshot: &RenderSnapshot,
    panel_rect: PanelRect,
    palette: &AnsiPalette,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    shape_cache: &mut TextShapeCache,
    font_size: f32,
    scale_factor: f32,
    metrics: CellMetrics,
    scroll_offset_y_logical: f32,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
) {
    let sf = scale_factor;
    let cell_w_logical = metrics.width_physical / sf;
    let cell_h_logical = metrics.height_physical / sf;
    let panel_origin_x_physical = panel_rect.x * sf;
    let panel_origin_y_physical = panel_rect.y * sf;
    let scroll_offset_y_physical = scroll_offset_y_logical * sf;
    let total_rows = snapshot.rows.len();
    let visible_rows = snapshot.visible_rows;
    // The visible region of the buffer is anchored at the BOTTOM of
    // the panel — i.e. with `scroll_offset_y = 0` (no scrollback
    // shown) row `total - visible` should land at the panel's first
    // cell. We accomplish this by shifting every row up by
    // `(total - visible) * cell_h`, then applying the scroll offset.
    let baseline_offset_phys =
        (total_rows.saturating_sub(visible_rows)) as f32 * metrics.height_physical;
    let mut cell_text = String::with_capacity(8);

    let panel_max_x_phys = panel_rect.w * sf;
    let panel_max_y_phys = panel_rect.h * sf;
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        // Y of this row's top edge relative to panel top, in physical px.
        // `+ scroll_offset` because scrolling UP visually moves rows DOWN.
        let row_y_phys = row_idx as f32 * metrics.height_physical - baseline_offset_phys
            + scroll_offset_y_physical;
        if row_y_phys + metrics.height_physical <= 0.0 || row_y_phys >= panel_max_y_phys {
            continue;
        }
        for (col_idx, cell) in row.cells.iter().enumerate() {
            let col_x_phys = col_idx as f32 * metrics.width_physical;
            if col_x_phys >= panel_max_x_phys {
                break;
            }
            let inverse = cell.flags.contains(CellFlags::INVERSE);
            let (fg_eff, bg_eff) = if inverse {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };

            let pos_x_logical = (panel_origin_x_physical + col_x_phys) / sf;
            let pos_y_logical = (panel_origin_y_physical + row_y_phys) / sf;

            if bg_eff != TermColor::Default {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical],
                    size: [cell_w_logical, cell_h_logical],
                    color: bg_eff.to_rgba(palette),
                });
            }

            let is_blank = cell.c == ' ' || cell.c == '\0';
            let has_decoration = cell.flags.underline()
                || cell.flags.double_underline()
                || cell.flags.strike();
            if is_blank && fg_eff == TermColor::Default && !has_decoration {
                continue;
            }

            let mut color = if fg_eff == TermColor::Default {
                DEFAULT_FG
            } else {
                fg_eff.to_rgba(palette)
            };
            if cell.flags.faint() {
                color[3] *= 0.5;
            }

            // SGR HIDDEN suppresses the glyph but keeps bg and any
            // decoration lines (matches xterm/iTerm behavior).
            let push_glyph = !cell.flags.hidden() && !is_blank;
            if push_glyph {
                let cell_origin_x_phys = panel_origin_x_physical + col_x_phys;
                let cell_origin_y_phys = panel_origin_y_physical + row_y_phys;

                let weight = if cell.flags.bold() {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                };
                let style = if cell.flags.italic() {
                    Style::Italic
                } else {
                    Style::Normal
                };

                // Fast path: single-codepoint cell with no combining
                // marks resolves through direct cmap
                // (TextShapeCache::shape_char) — no String alloc, no
                // cosmic-text shaper. Mirrors Warp's
                // CellGlyphCache.glyph_cache hot path. Combining
                // clusters and missing-glyph fallbacks drop through to
                // the slow String-keyed path below.
                let zerowidth_count = cell.extra.as_ref().map_or(0, |e| e.zerowidth.len());
                let mut fast_path_handled = false;
                if zerowidth_count == 0 {
                    if let Some(cg) = shape_cache
                        .shape_char(font_system, cell.c, font_size, sf, weight, style)
                    {
                        let font_size_physical = font_size * sf;
                        let baseline_y_phys = cell_origin_y_phys + cg.baseline_y_physical;
                        let (cache_key, glyph_x_floor, glyph_y_floor) = CacheKey::new(
                            cg.font_id,
                            cg.glyph_id,
                            font_size_physical,
                            (cell_origin_x_phys, baseline_y_phys),
                            CacheKeyFlags::empty(),
                        );
                        if let Some(placed) = atlas.get_or_insert(cache_key, || {
                            rasterize_glyph(font_system, swash_cache, cache_key)
                        }) {
                            let pos_x = (glyph_x_floor as f32 + placed.offset_x) / sf;
                            let pos_y = (glyph_y_floor as f32 - placed.offset_y) / sf;
                            glyphs.push(GlyphInstance {
                                pos: [pos_x, pos_y],
                                size: [placed.width / sf, placed.height / sf],
                                uv_min: placed.uv_min,
                                uv_max: placed.uv_max,
                                color,
                            });
                        }
                        fast_path_handled = true;
                    }
                }

                if !fast_path_handled {
                    cell_text.clear();
                    cell_text.push(cell.c);
                    if let Some(extra) = &cell.extra {
                        for c in &extra.zerowidth {
                            cell_text.push(*c);
                        }
                    }
                    let shaped = shape_cache.shape(
                        font_system,
                        &cell_text,
                        font_size,
                        sf,
                        None,
                        weight,
                        style,
                    );
                    for line in &shaped.lines {
                        let baseline_y = (cell_origin_y_phys + line.line_y).round();
                        for glyph in &line.glyphs {
                            let physical = glyph.physical((cell_origin_x_phys, baseline_y), 1.0);
                            let Some(placed) = atlas.get_or_insert(physical.cache_key, || {
                                rasterize_glyph(font_system, swash_cache, physical.cache_key)
                            }) else {
                                continue;
                            };
                            let pos_x = (physical.x as f32 + placed.offset_x) / sf;
                            let pos_y = (physical.y as f32 - placed.offset_y) / sf;
                            glyphs.push(GlyphInstance {
                                pos: [pos_x, pos_y],
                                size: [placed.width / sf, placed.height / sf],
                                uv_min: placed.uv_min,
                                uv_max: placed.uv_max,
                                color,
                            });
                        }
                    }
                }
            }

            // SGR decoration lines. Positions are vertical fractions of
            // the cell height: underline sits just below the baseline
            // (~0.78), strike crosses the x-height midline (~0.42), the
            // double-underline pair brackets the regular underline
            // position.
            if cell.flags.underline() {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.78],
                    size: [cell_w_logical, 1.0],
                    color,
                });
            }
            if cell.flags.double_underline() {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.72],
                    size: [cell_w_logical, 0.8],
                    color,
                });
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.84],
                    size: [cell_w_logical, 0.8],
                    color,
                });
            }
            if cell.flags.strike() {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.42],
                    size: [cell_w_logical, 1.0],
                    color,
                });
            }
        }
    }
}

/// Build a `RectInstance` for the cursor at the snapshot's reported
/// position. Returns `None` when the cursor is hidden or falls outside
/// `panel_rect` (typical during a divider drag while the emulator is
/// catching up to the new size, or while the user is scrolled into
/// scrollback so the live cursor is below the visible region).
pub fn build_cursor_rect(
    cursor: CursorState,
    visible_start: usize,
    panel_rect: PanelRect,
    scale_factor: f32,
    metrics: CellMetrics,
    scroll_offset_y_logical: f32,
) -> Option<RectInstance> {
    if !cursor.visible {
        return None;
    }
    let sf = scale_factor;
    let cell_offset_x_phys = cursor.col as f32 * metrics.width_physical;
    // Cursor's row is visible-relative; combine with `visible_start` to
    // get the absolute row, then subtract the visible-anchor offset so
    // that `scroll_offset_y == 0` puts the cursor at its expected place
    // inside the panel.
    let abs_row = visible_start + cursor.row as usize;
    let scroll_offset_y_phys = scroll_offset_y_logical * sf;
    let baseline_offset_phys = visible_start as f32 * metrics.height_physical;
    let cell_offset_y_phys = abs_row as f32 * metrics.height_physical - baseline_offset_phys
        + scroll_offset_y_phys;
    let panel_h_phys = panel_rect.h * sf;
    if cell_offset_x_phys >= panel_rect.w * sf
        || cell_offset_y_phys + metrics.height_physical <= 0.0
        || cell_offset_y_phys >= panel_h_phys
    {
        return None;
    }
    let cell_x_phys = panel_rect.x * sf + cell_offset_x_phys;
    let cell_y_phys = panel_rect.y * sf + cell_offset_y_phys;
    let cell_w_phys = metrics.width_physical;
    let cell_h_phys = metrics.height_physical;
    let (pos_phys, size_phys) = match cursor.style {
        CursorStyle::BlockSteady | CursorStyle::BlockBlink => {
            ([cell_x_phys, cell_y_phys], [cell_w_phys, cell_h_phys])
        }
        CursorStyle::UnderlineSteady | CursorStyle::UnderlineBlink => (
            [
                cell_x_phys,
                cell_y_phys + cell_h_phys - CURSOR_STROKE_PHYSICAL,
            ],
            [cell_w_phys, CURSOR_STROKE_PHYSICAL],
        ),
        CursorStyle::BeamSteady | CursorStyle::BeamBlink => (
            [cell_x_phys, cell_y_phys],
            [CURSOR_STROKE_PHYSICAL, cell_h_phys],
        ),
    };
    Some(RectInstance {
        pos: [pos_phys[0] / sf, pos_phys[1] / sf],
        size: [size_phys[0] / sf, size_phys[1] / sf],
        color: CURSOR_COLOR,
    })
}
