//! Glyph atlas: an RGBA8 **texture array** + Shelf-Next-Fit packer per layer +
//! frame-counter eviction.
//!
//! Adapted from warpdotdev/warp (MIT). Sources:
//! - crates/warpui/src/rendering/atlas/allocator.rs (packer)
//! - crates/warpui/src/rendering/atlas/manager.rs (grow-on-full)
//! - crates/warpui_core/src/rendering/texture_cache.rs (frame-counter eviction)
//!
//! Design choices (see `docs/gpu-terminal-spec.md` §5.4):
//!
//! 1. **`RGBA8Unorm`, not `R8Unorm`** — one atlas holds both mono glyphs
//!    (data in the alpha channel) and colour glyphs (emoji).
//! 2. **Shelf-Next-Fit packer** — three state fields, ~50 lines.
//! 3. **Grow by LAYERS, like Warp's `Manager`.** When the current layer fills,
//!    packing advances to the next layer of the texture array; glyphs are never
//!    silently dropped while a layer is free. Warp uses a *series of textures*;
//!    we use array layers (one bind group, one sampler, a per-glyph layer
//!    index) — the same idea with simpler binding.
//! 4. **Frame-counter eviction + whole-layer reclaim.** `end_frame()` drops
//!    entries unused for `MAX_UNUSED_FRAMES`, then resets any layer whose glyphs
//!    are all gone (the array analogue of Warp's whole-texture eviction) so the
//!    space is reused. Only when *every* layer fills in a single frame do we
//!    compact wholesale (reset all + redraw) as a last resort.

use std::collections::HashMap;

use cosmic_text::CacheKey;

const ATLAS_SIZE: u32 = 1024;
const GLYPH_PAD: u32 = 1;
const MAX_UNUSED_FRAMES: u32 = 10;
/// Layers in the texture array. Four 1024² RGBA8 layers = 16 MiB; with
/// whole-layer reclaim this is far more than a terminal needs at steady state,
/// so the wholesale-compaction fallback effectively never fires.
const MAX_LAYERS: usize = 4;

/// Bytes in one atlas layer (1024×1024 RGBA8).
const fn layer_bytes() -> usize {
    (ATLAS_SIZE * ATLAS_SIZE * 4) as usize
}

/// Shelf-Next-Fit bin packer. Tracks three things: the Y of the current
/// shelf's top edge, the tallest item placed on it so far, and the right
/// edge of items already placed on it. When `pack` can't fit horizontally,
/// it advances to a new shelf at `row_baseline + row_tallest + padding`.
pub struct ShelfPacker {
    width: u32,
    height: u32,
    row_baseline: u32,
    row_tallest: u32,
    row_extent: u32,
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            row_baseline: 0,
            row_tallest: 0,
            row_extent: 0,
        }
    }

    /// Try to allocate a `(w + 2*pad) × (h + 2*pad)` rectangle. Returns the
    /// top-left of the *padded* region's interior, i.e. the pixel where the
    /// glyph's first column lands.
    pub fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let w = w + GLYPH_PAD * 2;
        let h = h + GLYPH_PAD * 2;
        if w > self.width {
            return None;
        }
        if self.row_extent + w > self.width {
            self.row_baseline += self.row_tallest + GLYPH_PAD;
            self.row_extent = 0;
            self.row_tallest = 0;
        }
        if self.row_baseline + h > self.height {
            return None;
        }
        let pos = (self.row_extent + GLYPH_PAD, self.row_baseline + GLYPH_PAD);
        self.row_extent += w;
        self.row_tallest = self.row_tallest.max(h);
        Some(pos)
    }

    pub fn reset(&mut self) {
        self.row_baseline = 0;
        self.row_tallest = 0;
        self.row_extent = 0;
    }

    /// True if nothing has been packed since construction / `reset`.
    pub fn is_empty(&self) -> bool {
        self.row_baseline == 0 && self.row_extent == 0 && self.row_tallest == 0
    }
}

/// Source format of a rasterized glyph. Mono glyphs come from outline
/// rasterization (one alpha byte per pixel); colour glyphs come from
/// CBDT/COLR/SVG fonts (premultiplied RGBA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphFormat {
    Alpha,
    Rgba,
}

/// CPU-side rasterized glyph, ready to be inserted into the atlas.
/// `left`/`top` are the glyph's bearing relative to the pen position.
pub struct RasterizedGlyph {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub format: GlyphFormat,
}

impl RasterizedGlyph {
    fn bytes_per_pixel(&self) -> usize {
        match self.format {
            GlyphFormat::Alpha => 1,
            GlyphFormat::Rgba => 4,
        }
    }
}

/// A glyph placed in the atlas. UVs are in `[0, 1]` within `layer`; size and
/// bearing are in pixels (caller multiplies by scale factor at draw time).
#[derive(Debug, Clone, Copy)]
pub struct PlacedGlyph {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: f32,
    pub height: f32,
    /// Index of the texture-array layer this glyph lives in.
    pub layer: u32,
}

/// Atlas entry: where the glyph sits plus the frame it was last sampled on
/// (used by `end_frame()` to evict stale entries and reclaim empty layers).
#[derive(Debug, Clone, Copy)]
struct AtlasEntry {
    placed: PlacedGlyph,
    last_used_frame: u32,
}

/// RGBA8 glyph atlas backed by a `wgpu` texture **array** plus its CPU mirror
/// and a lookup map from cosmic-text `CacheKey` to placement.
pub struct GlyphAtlas {
    /// One packer per array layer.
    packers: Vec<ShelfPacker>,
    /// Layer currently being packed into.
    current_layer: usize,
    entries: HashMap<CacheKey, AtlasEntry>,
    /// CPU mirror of all layers, laid out layer-major (layer 0 first).
    cpu_data: Vec<u8>,
    /// Which layers changed since the last `upload` (so we re-upload only those).
    layer_dirty: [bool; MAX_LAYERS],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    frame: u32,
    /// Set when a glyph could not pack into ANY layer this frame.
    out_of_space: bool,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("term_gpu/glyph_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_LAYERS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("term_gpu/glyph_atlas_view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            packers: (0..MAX_LAYERS)
                .map(|_| ShelfPacker::new(ATLAS_SIZE, ATLAS_SIZE))
                .collect(),
            current_layer: 0,
            entries: HashMap::with_capacity(2048),
            cpu_data: vec![0u8; layer_bytes() * MAX_LAYERS],
            layer_dirty: [false; MAX_LAYERS],
            texture,
            view,
            frame: 0,
            out_of_space: false,
        }
    }

    pub const fn size() -> u32 {
        ATLAS_SIZE
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Look up `key` in the cache, or rasterize and insert it. The
    /// `rasterize` closure is only invoked on a miss. Returns `None` if the
    /// glyph has no visual representation or every layer is full (in which case
    /// `end_frame()` will compact). Sets `last_used_frame` on a hit.
    pub fn get_or_insert<F>(&mut self, key: CacheKey, rasterize: F) -> Option<PlacedGlyph>
    where
        F: FnOnce() -> Option<RasterizedGlyph>,
    {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used_frame = self.frame;
            return Some(entry.placed);
        }
        let raster = rasterize()?;
        let Some(placed) = self.insert_raw(&raster) else {
            // Every layer is full. Flag it so `end_frame()` compacts; drop this
            // glyph for the current frame (rendered blank, then the
            // post-compaction redraw fills it in).
            self.out_of_space = true;
            return None;
        };
        self.entries.insert(
            key,
            AtlasEntry {
                placed,
                last_used_frame: self.frame,
            },
        );
        Some(placed)
    }

    /// Advance the frame counter, then evict stale entries and reclaim empty
    /// layers — or, if the atlas filled this frame, compact wholesale. Call
    /// once per rendered frame. Returns `true` if a wholesale compaction
    /// happened, in which case the caller should request one more redraw so the
    /// dropped-glyph frame is replaced by a clean one.
    pub fn end_frame(&mut self) -> bool {
        self.frame = self.frame.wrapping_add(1);

        if self.out_of_space {
            // Every layer filled in a single frame (pathological). Reset
            // everything; the next frame re-rasterizes only what it draws.
            for p in &mut self.packers {
                p.reset();
            }
            self.cpu_data.fill(0);
            self.layer_dirty = [true; MAX_LAYERS];
            self.entries.clear();
            self.current_layer = 0;
            self.out_of_space = false;
            return true;
        }

        // Drop entries unused for too long.
        let now = self.frame;
        self.entries
            .retain(|_, e| now.wrapping_sub(e.last_used_frame) <= MAX_UNUSED_FRAMES);

        // Reclaim any layer whose glyphs are all gone — reset its packer and
        // clear its pixels (keeping the 1px-padding-is-zero invariant) so the
        // space is reused. This is the array analogue of Warp's whole-texture
        // eviction; it keeps heavy scrolling from marching to the last layer.
        let mut layer_live = [false; MAX_LAYERS];
        for e in self.entries.values() {
            layer_live[e.placed.layer as usize] = true;
        }
        for layer in 0..MAX_LAYERS {
            if !layer_live[layer] && !self.packers[layer].is_empty() {
                self.packers[layer].reset();
                let base = layer * layer_bytes();
                self.cpu_data[base..base + layer_bytes()].fill(0);
                self.layer_dirty[layer] = true;
            }
        }
        // Start next frame's packing from layer 0 so partially-filled / freshly
        // reclaimed low layers get reused before climbing to higher ones.
        self.current_layer = 0;
        false
    }

    /// Pack a rasterized glyph into the first layer (from `current_layer` up)
    /// with room, copying its pixels into the CPU mirror. Returns the placement,
    /// or `None` if no layer can fit it. Bypasses the cache — usually you want
    /// `get_or_insert`.
    pub fn insert_raw(&mut self, raster: &RasterizedGlyph) -> Option<PlacedGlyph> {
        let (layer, x, y) = loop {
            if let Some((x, y)) = self.packers[self.current_layer].pack(raster.width, raster.height) {
                break (self.current_layer, x, y);
            }
            if self.current_layer + 1 >= MAX_LAYERS {
                return None;
            }
            self.current_layer += 1;
        };

        let bpp = raster.bytes_per_pixel();
        let atlas_w = ATLAS_SIZE as usize;
        let layer_base = layer * layer_bytes();
        for row in 0..raster.height {
            for col in 0..raster.width {
                let src = (row * raster.width + col) as usize * bpp;
                let dst = layer_base
                    + ((y as usize + row as usize) * atlas_w + x as usize + col as usize) * 4;
                match raster.format {
                    GlyphFormat::Alpha => {
                        // Mono glyph: zero RGB, alpha holds coverage. The text
                        // fragment shader multiplies by the text colour when it
                        // sees zero RGB.
                        self.cpu_data[dst] = 0;
                        self.cpu_data[dst + 1] = 0;
                        self.cpu_data[dst + 2] = 0;
                        self.cpu_data[dst + 3] = raster.data[src];
                    }
                    GlyphFormat::Rgba => {
                        self.cpu_data[dst..dst + 4].copy_from_slice(&raster.data[src..src + 4]);
                    }
                }
            }
        }
        self.layer_dirty[layer] = true;

        let scale = 1.0 / ATLAS_SIZE as f32;
        Some(PlacedGlyph {
            uv_min: [x as f32 * scale, y as f32 * scale],
            uv_max: [
                (x + raster.width) as f32 * scale,
                (y + raster.height) as f32 * scale,
            ],
            offset_x: raster.left as f32,
            offset_y: raster.top as f32,
            width: raster.width as f32,
            height: raster.height as f32,
            layer: layer as u32,
        })
    }

    /// Upload the dirty layers of the CPU mirror to the GPU texture. Cheap when
    /// nothing changed; uploads only the layers touched since the last call.
    pub fn upload(&mut self, queue: &wgpu::Queue) {
        for layer in 0..MAX_LAYERS {
            if !self.layer_dirty[layer] {
                continue;
            }
            let base = layer * layer_bytes();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &self.cpu_data[base..base + layer_bytes()],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ATLAS_SIZE * 4),
                    rows_per_image: Some(ATLAS_SIZE),
                },
                wgpu::Extent3d {
                    width: ATLAS_SIZE,
                    height: ATLAS_SIZE,
                    depth_or_array_layers: 1,
                },
            );
            self.layer_dirty[layer] = false;
        }
    }
}
