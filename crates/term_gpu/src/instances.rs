//! GPU-uploaded instance and uniform data with manual `repr(C)` casts.
//!
//! The cast pattern (no `bytemuck` dependency) is intentional — see
//! `docs/gpu-terminal-spec.md` §5.3.

use std::mem::{size_of, size_of_val};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

impl RectInstance {
    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        // Safety: `RectInstance` is `#[repr(C)]` and `Copy`; the resulting
        // byte slice has the same lifetime as the input and is read-only.
        unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, size_of_val(slice)) }
    }

    pub const ATTRIBS: [wgpu::VertexAttribute; 3] = [
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 8,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 16,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32x4,
        },
    ];

    pub const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Per-glyph instance data for the text pipeline. `color` is the tint
/// applied to monochrome glyphs; colour glyphs (emoji) ignore it and use
/// their atlas RGBA verbatim.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GlyphInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub color: [f32; 4],
    /// Texture-array layer this glyph's atlas slot lives in.
    pub layer: u32,
}

impl GlyphInstance {
    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        // Safety: `GlyphInstance` is `#[repr(C)]` and `Copy`.
        unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, size_of_val(slice)) }
    }

    pub const ATTRIBS: [wgpu::VertexAttribute; 6] = [
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 8,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 16,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 24,
            shader_location: 3,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 32,
            shader_location: 4,
            format: wgpu::VertexFormat::Float32x4,
        },
        wgpu::VertexAttribute {
            offset: 48,
            shader_location: 5,
            format: wgpu::VertexFormat::Uint32,
        },
    ];

    pub const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Per-instance data for the drop-shadow pipeline. Drawn UNDER the
/// content rect that overlays it — typically a popup or command
/// palette. The fragment shader evaluates a rounded-rect SDF at
/// `pos + offset` with the given `corner_radius`, then smoothsteps
/// from saturated (inside content) to transparent at `blur_radius`
/// outwards. The content rect drawn afterward covers the saturated
/// centre, leaving only the soft halo visible.
///
/// Coordinates and dimensions are in **logical pixels**, matching
/// `RectInstance` / `GlyphInstance`. Colour is non-premultiplied
/// (the shader's blend state is `ALPHA_BLENDING`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ShadowInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub blur_radius: f32,
    pub corner_radius: f32,
    pub offset: [f32; 2],
    pub color: [f32; 4],
}

impl ShadowInstance {
    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        // Safety: `ShadowInstance` is `#[repr(C)]` and `Copy`.
        unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, size_of_val(slice)) }
    }

    pub const ATTRIBS: [wgpu::VertexAttribute; 6] = [
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 8,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 16,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32,
        },
        wgpu::VertexAttribute {
            offset: 20,
            shader_location: 3,
            format: wgpu::VertexFormat::Float32,
        },
        wgpu::VertexAttribute {
            offset: 24,
            shader_location: 4,
            format: wgpu::VertexFormat::Float32x2,
        },
        wgpu::VertexAttribute {
            offset: 32,
            shader_location: 5,
            format: wgpu::VertexFormat::Float32x4,
        },
    ];

    pub const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A rounded rectangle with an optional border — one instance does both the
/// fill and the border ring via a rounded-rect SDF in the fragment shader.
/// `corner_radius == 0` is a sharp rectangle; `border_width == 0` is a plain
/// fill. Drawn in its own pass BETWEEN the rect pass and the glyph pass, so a
/// UI box (chrome bar / popup / panel / highlight) sits over the terminal cell
/// rects and under the text. All dimensions are **logical pixels**; colours are
/// non-premultiplied (`ALPHA_BLENDING`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RoundRectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub fill_color: [f32; 4],
    pub border_color: [f32; 4],
    pub border_width: f32,
    pub corner_radius: f32,
}

impl RoundRectInstance {
    /// A plain rounded fill (no border).
    pub fn fill(pos: [f32; 2], size: [f32; 2], color: [f32; 4], corner_radius: f32) -> Self {
        Self {
            pos,
            size,
            fill_color: color,
            border_color: [0.0; 4],
            border_width: 0.0,
            corner_radius,
        }
    }

    /// A rounded box with a `border_width`-thick `border_color` ring around a
    /// `fill_color` interior.
    pub fn new(
        pos: [f32; 2],
        size: [f32; 2],
        fill_color: [f32; 4],
        border_color: [f32; 4],
        border_width: f32,
        corner_radius: f32,
    ) -> Self {
        Self { pos, size, fill_color, border_color, border_width, corner_radius }
    }

    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        // Safety: `RoundRectInstance` is `#[repr(C)]` and `Copy`.
        unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, size_of_val(slice)) }
    }

    pub const ATTRIBS: [wgpu::VertexAttribute; 6] = [
        wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x2 },
        wgpu::VertexAttribute { offset: 8, shader_location: 1, format: wgpu::VertexFormat::Float32x2 },
        wgpu::VertexAttribute { offset: 16, shader_location: 2, format: wgpu::VertexFormat::Float32x4 },
        wgpu::VertexAttribute { offset: 32, shader_location: 3, format: wgpu::VertexFormat::Float32x4 },
        wgpu::VertexAttribute { offset: 48, shader_location: 4, format: wgpu::VertexFormat::Float32 },
        wgpu::VertexAttribute { offset: 52, shader_location: 5, format: wgpu::VertexFormat::Float32 },
    ];

    pub const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A render layer groups shadow / rect / round-rect / glyph instances drawn as
/// one stratum in a layered render call. Within a layer, draw order is
/// fixed: shadows first, then rects, then round-rects, then glyphs. Between layers,
/// `GpuRenderer::render` draws `base` before the optional `overlay`,
/// which is how popups (with their drop shadow) sit on top of the
/// terminal grid.
///
/// Borrowed slices — the renderer doesn't take ownership. Callers
/// typically build the underlying `Vec`s once per frame, slice into
/// this struct, and pass it to `render`.
#[derive(Debug, Clone, Copy)]
pub struct RenderLayer<'a> {
    pub shadows: &'a [ShadowInstance],
    pub rects: &'a [RectInstance],
    pub round_rects: &'a [RoundRectInstance],
    pub glyphs: &'a [GlyphInstance],
}

impl<'a> RenderLayer<'a> {
    pub const EMPTY: RenderLayer<'static> = RenderLayer {
        shadows: &[],
        rects: &[],
        round_rects: &[],
        glyphs: &[],
    };

    pub fn rects(rects: &'a [RectInstance]) -> Self {
        Self {
            shadows: &[],
            rects,
            round_rects: &[],
            glyphs: &[],
        }
    }

    pub fn rects_and_glyphs(rects: &'a [RectInstance], glyphs: &'a [GlyphInstance]) -> Self {
        Self {
            shadows: &[],
            rects,
            round_rects: &[],
            glyphs,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shadows.is_empty()
            && self.rects.is_empty()
            && self.round_rects.is_empty()
            && self.glyphs.is_empty()
    }
}

/// Per-frame uniforms shared by both pipelines. All instance positions and
/// sizes are in **logical pixels**; the shader multiplies by `scale_factor`
/// to get physical pixels before the NDC transform. This keeps `RectInstance`
/// and `GlyphInstance` DPI-independent.
///
/// Padded to 32 bytes for std140-style 16-byte alignment.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Uniforms {
    pub screen_size: [f32; 2],
    pub scroll_offset: [f32; 2],
    pub scale_factor: f32,
    pub _pad: [f32; 3],
}

impl Uniforms {
    pub fn as_bytes(&self) -> &[u8] {
        // Safety: `Uniforms` is `#[repr(C)]` and `Copy`.
        unsafe { std::slice::from_raw_parts(self as *const Self as *const u8, size_of::<Self>()) }
    }
}
