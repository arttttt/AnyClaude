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

/// Default background color for cells that have not specified one.
/// Matches the renderer's surface clear color so an inverse-video
/// cell with no explicit background ends up using this as its
/// post-swap foreground — i.e. the glyph stays invisible on the
/// window's clear-color backdrop. Conventionally cell bg "default"
/// IS the window background.
pub const DEFAULT_BG: [f32; 4] = [0.04, 0.04, 0.06, 1.0];

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

/// Measure a cell's physical pixel dimensions from the primary
/// font face's real ascent + descent + line_gap, mirroring Warp's
/// `grid_size_util` formula. Result: cell rows tile seamlessly
/// without vertical gaps (so Unicode block art lines up) and text
/// rows don't overlap (descenders fit within the cell's bottom edge).
///
/// Falls back to `font_size * 1.2` when face resolution fails — the
/// degenerate path picks a sane default rather than panicking.
pub fn measure_cell_metrics(
    font_system: &mut FontSystem,
    shape_cache: &mut TextShapeCache,
    font_size: f32,
    scale_factor: f32,
) -> CellMetrics {
    if let Some(metrics) = shape_cache.face_metrics(
        font_system,
        font_size,
        scale_factor,
        Weight::NORMAL,
        Style::Normal,
    ) {
        return CellMetrics {
            width_physical: metrics.cell_width(),
            height_physical: metrics.cell_height(),
        };
    }
    let fallback_h = (font_size * scale_factor * 1.2).round().max(1.0);
    let fallback_w = (font_size * scale_factor * 0.6).round().max(1.0);
    CellMetrics {
        width_physical: fallback_w,
        height_physical: fallback_h,
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
            // Resolve TermColor::Default to concrete RGBA before the
            // inverse swap so swapping two Defaults doesn't end up as
            // a degenerate "swap nothing for nothing". The bg side
            // stays Option-typed so the non-inverse / no-explicit-bg
            // path can still skip the bg rect push and let the window
            // clear-color show through.
            let fg_concrete: [f32; 4] = if cell.fg == TermColor::Default {
                DEFAULT_FG
            } else {
                cell.fg.to_rgba(palette)
            };
            let bg_explicit: Option<[f32; 4]> = if cell.bg == TermColor::Default {
                None
            } else {
                Some(cell.bg.to_rgba(palette))
            };
            let (fg_eff_rgba, bg_eff_rgba): ([f32; 4], Option<[f32; 4]>) = if inverse {
                // After swap: the new fg is whatever was bg, falling
                // back to DEFAULT_BG so an inverse cell with no
                // explicit colors still has a visible "block" — ink-
                // based TUIs (Claude Code, htop, vim's visual mode)
                // render their faux-cursor as `CSI 7 m SP CSI 27 m`,
                // which without this fallback would collapse to a
                // blank cell on the clear-color backdrop.
                (bg_explicit.unwrap_or(DEFAULT_BG), Some(fg_concrete))
            } else {
                (fg_concrete, bg_explicit)
            };

            let pos_x_logical = (panel_origin_x_physical + col_x_phys) / sf;
            let pos_y_logical = (panel_origin_y_physical + row_y_phys) / sf;

            if let Some(bg) = bg_eff_rgba {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical],
                    size: [cell_w_logical, cell_h_logical],
                    color: bg,
                });
            }

            let is_blank = cell.c == ' ' || cell.c == '\0';
            let has_decoration = cell.flags.underline()
                || cell.flags.double_underline()
                || cell.flags.strike();
            // An INVERSE cell's bg rect is the visible content, so a
            // blank glyph is fine — we already pushed the rect above.
            // For non-inverse cells, a blank with default fg and no
            // decoration produces nothing visible, skip it.
            if is_blank && !inverse && bg_eff_rgba.is_none() && !has_decoration {
                continue;
            }

            let mut color = fg_eff_rgba;
            if cell.flags.faint() {
                color[3] *= 0.5;
            }

            // SGR HIDDEN suppresses the glyph but keeps bg and any
            // decoration lines (matches xterm/iTerm behavior).
            let push_glyph = !cell.flags.hidden() && !is_blank;
            if push_glyph {
                // Native block-char painter: U+2580-259F rasterise via
                // colored rects spanning specific cell fractions, not
                // through the font shaper. Guarantees seamless tiling
                // (no anti-alias fringe, no font-metrics gap) — block
                // art and Unicode borders line up pixel-perfectly.
                // Mirrors Warp's `render_native_glyph`.
                if paint_block_char(
                    cell.c,
                    pos_x_logical,
                    pos_y_logical,
                    cell_w_logical,
                    cell_h_logical,
                    color,
                    rects,
                ) {
                    // Decoration lines (underline / strike) still apply
                    // — drop through to that block at the end of the
                    // outer loop iteration without going through the
                    // font-shaped path.
                    if cell.flags.underline() {
                        rects.push(RectInstance {
                            pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.78],
                            size: [cell_w_logical, 1.0],
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
                    continue;
                }

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


/// Paint a Unicode block / shade character (U+2580–U+259F) as
/// one or more solid rects filling specific fractions of the
/// cell. Returns `true` when `ch` was handled (caller must skip
/// the shaped-glyph path); `false` otherwise.
///
/// The block char glyphs in monospace fonts are designed to span
/// `[0, cell_size]` in their respective dimensions, but cosmic-text's
/// rasterised glyph image is clipped to the visible coverage and
/// anti-aliased, so adjacent cells leave a faint 1-2px seam between
/// rows / columns. Painting them as colored rectangles aligned to
/// integer cell pixels eliminates the seam entirely. Mirrors Warp's
/// `render_native_glyph` (app/src/terminal/grid_renderer.rs:2008+).
pub fn paint_block_char(
    ch: char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    rects: &mut Vec<RectInstance>,
) -> bool {
    // The eighth fractions are precomputed once instead of inside
    // each match arm so the codegen stays compact.
    let h1 = h / 8.0;
    let h2 = h * 2.0 / 8.0;
    let h3 = h * 3.0 / 8.0;
    let h4 = h / 2.0;
    let h5 = h * 5.0 / 8.0;
    let h6 = h * 6.0 / 8.0;
    let h7 = h * 7.0 / 8.0;
    let w1 = w / 8.0;
    let w2 = w * 2.0 / 8.0;
    let w3 = w * 3.0 / 8.0;
    let w4 = w / 2.0;
    let w5 = w * 5.0 / 8.0;
    let w6 = w * 6.0 / 8.0;
    let w7 = w * 7.0 / 8.0;

    // Shade chars (░ ▒ ▓) are full-cell rects with reduced alpha.
    let shade = match ch {
        '\u{2591}' => Some(64.0 / 255.0),  // ░
        '\u{2592}' => Some(128.0 / 255.0), // ▒
        '\u{2593}' => Some(191.0 / 255.0), // ▓
        _ => None,
    };
    if let Some(alpha) = shade {
        rects.push(RectInstance {
            pos: [x, y],
            size: [w, h],
            color: [color[0], color[1], color[2], color[3] * alpha],
        });
        return true;
    }

    match ch {
        // ▀ Upper half (U+2580)
        '\u{2580}' => rects.push(RectInstance { pos: [x, y], size: [w, h4], color }),
        // ▁ Lower 1/8 (U+2581)
        '\u{2581}' => rects.push(RectInstance { pos: [x, y + h7], size: [w, h1], color }),
        '\u{2582}' => rects.push(RectInstance { pos: [x, y + h6], size: [w, h2], color }),
        '\u{2583}' => rects.push(RectInstance { pos: [x, y + h5], size: [w, h3], color }),
        // ▄ Lower half (U+2584)
        '\u{2584}' => rects.push(RectInstance { pos: [x, y + h4], size: [w, h4], color }),
        '\u{2585}' => rects.push(RectInstance { pos: [x, y + h3], size: [w, h5], color }),
        '\u{2586}' => rects.push(RectInstance { pos: [x, y + h2], size: [w, h6], color }),
        '\u{2587}' => rects.push(RectInstance { pos: [x, y + h1], size: [w, h7], color }),
        // █ Full block (U+2588)
        '\u{2588}' => rects.push(RectInstance { pos: [x, y], size: [w, h], color }),
        '\u{2589}' => rects.push(RectInstance { pos: [x, y], size: [w7, h], color }),
        '\u{258A}' => rects.push(RectInstance { pos: [x, y], size: [w6, h], color }),
        '\u{258B}' => rects.push(RectInstance { pos: [x, y], size: [w5, h], color }),
        // ▌ Left half (U+258C)
        '\u{258C}' => rects.push(RectInstance { pos: [x, y], size: [w4, h], color }),
        '\u{258D}' => rects.push(RectInstance { pos: [x, y], size: [w3, h], color }),
        '\u{258E}' => rects.push(RectInstance { pos: [x, y], size: [w2, h], color }),
        '\u{258F}' => rects.push(RectInstance { pos: [x, y], size: [w1, h], color }),
        // ▐ Right half (U+2590)
        '\u{2590}' => rects.push(RectInstance { pos: [x + w4, y], size: [w4, h], color }),
        // ▔ Upper 1/8 (U+2594)
        '\u{2594}' => rects.push(RectInstance { pos: [x, y], size: [w, h1], color }),
        // ▕ Right 1/8 (U+2595)
        '\u{2595}' => rects.push(RectInstance { pos: [x + w7, y], size: [w1, h], color }),
        // Quadrant blocks (U+2596–U+259F)
        '\u{2596}' => rects.push(RectInstance { pos: [x, y + h4], size: [w4, h4], color }), // ▖
        '\u{2597}' => rects.push(RectInstance { pos: [x + w4, y + h4], size: [w4, h4], color }), // ▗
        '\u{2598}' => rects.push(RectInstance { pos: [x, y], size: [w4, h4], color }), // ▘
        '\u{2599}' => {
            // ▙ Left half + lower-right quadrant
            rects.push(RectInstance { pos: [x, y], size: [w4, h], color });
            rects.push(RectInstance { pos: [x + w4, y + h4], size: [w4, h4], color });
        }
        '\u{259A}' => {
            // ▚ Upper-left + lower-right (anti-diagonal pair)
            rects.push(RectInstance { pos: [x, y], size: [w4, h4], color });
            rects.push(RectInstance { pos: [x + w4, y + h4], size: [w4, h4], color });
        }
        '\u{259B}' => {
            // ▛ Upper half + lower-left quadrant
            rects.push(RectInstance { pos: [x, y], size: [w, h4], color });
            rects.push(RectInstance { pos: [x, y + h4], size: [w4, h4], color });
        }
        '\u{259C}' => {
            // ▜ Upper half + lower-right quadrant
            rects.push(RectInstance { pos: [x, y], size: [w, h4], color });
            rects.push(RectInstance { pos: [x + w4, y + h4], size: [w4, h4], color });
        }
        '\u{259D}' => rects.push(RectInstance { pos: [x + w4, y], size: [w4, h4], color }), // ▝
        '\u{259E}' => {
            // ▞ Upper-right + lower-left
            rects.push(RectInstance { pos: [x + w4, y], size: [w4, h4], color });
            rects.push(RectInstance { pos: [x, y + h4], size: [w4, h4], color });
        }
        '\u{259F}' => {
            // ▟ Right half + lower-left quadrant
            rects.push(RectInstance { pos: [x + w4, y], size: [w4, h], color });
            rects.push(RectInstance { pos: [x, y + h4], size: [w4, h4], color });
        }
        _ => return false,
    }
    true
}
