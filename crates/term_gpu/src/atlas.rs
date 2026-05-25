//! Glyph atlas: RGBA8 texture + Shelf-Next-Fit packer.
//!
//! Adapted from warpdotdev/warp (MIT). Source:
//! crates/warpui/src/rendering/atlas/allocator.rs
//!
//! Two intentional choices documented in
//! `docs/gpu-terminal-spec.md` §5.4:
//!
//! 1. **`RGBA8Unorm`, not `R8Unorm`** — one atlas holds both mono glyphs
//!    (data in the alpha channel) and colour glyphs (emoji), avoiding a
//!    second texture and the branch logic that would entail.
//! 2. **Shelf-Next-Fit packer** — three state fields, ~50 lines. The
//!    algorithm is "fill the current shelf left-to-right; when an item
//!    doesn't fit, start a new shelf below as tall as the previous tallest
//!    item; when no more shelves fit, the atlas is full".
//!
//! Cache lookup, rasterization, and eviction land in subsequent commits
//! alongside the cosmic-text integration.

const ATLAS_SIZE: u32 = 1024;
const GLYPH_PAD: u32 = 1;

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

/// A glyph placed in the atlas. UVs are in `[0, 1]`; size and bearing are
/// in pixels (caller multiplies by scale factor at draw time).
#[derive(Debug, Clone, Copy)]
pub struct PlacedGlyph {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: f32,
    pub height: f32,
}

/// RGBA8 glyph atlas. Owns a single `wgpu::Texture` plus its CPU mirror.
/// `dirty` tracks whether the GPU copy is stale.
pub struct GlyphAtlas {
    packer: ShelfPacker,
    cpu_data: Vec<u8>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    dirty: bool,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("term_gpu/glyph_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            packer: ShelfPacker::new(ATLAS_SIZE, ATLAS_SIZE),
            cpu_data: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
            texture,
            view,
            dirty: false,
        }
    }

    pub const fn size() -> u32 {
        ATLAS_SIZE
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Pack a rasterized glyph into the atlas and copy its pixels into the
    /// CPU mirror. Returns the placement, or `None` if the atlas is full.
    ///
    /// The texture is not uploaded here — call `upload()` once per frame
    /// after all inserts.
    pub fn insert(&mut self, raster: &RasterizedGlyph) -> Option<PlacedGlyph> {
        let (x, y) = self.packer.pack(raster.width, raster.height)?;
        let bpp = raster.bytes_per_pixel();
        let atlas_w = ATLAS_SIZE as usize;
        for row in 0..raster.height {
            for col in 0..raster.width {
                let src = (row * raster.width + col) as usize * bpp;
                let dst = ((y as usize + row as usize) * atlas_w + x as usize + col as usize) * 4;
                match raster.format {
                    GlyphFormat::Alpha => {
                        // Mono glyph: zero RGB, alpha holds coverage. The
                        // text fragment shader knows to multiply by the text
                        // colour when it sees zero RGB.
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
        self.dirty = true;

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
        })
    }

    /// Upload the CPU mirror to the GPU texture if anything changed since
    /// the last upload. Cheap when called every frame with no changes.
    pub fn upload(&mut self, queue: &wgpu::Queue) {
        if !self.dirty {
            return;
        }
        queue.write_texture(
            self.texture.as_image_copy(),
            &self.cpu_data,
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
        self.dirty = false;
    }
}
