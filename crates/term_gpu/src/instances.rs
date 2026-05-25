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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Uniforms {
    pub screen_size: [f32; 2],
    pub scroll_offset: [f32; 2],
}

impl Uniforms {
    pub fn as_bytes(&self) -> &[u8] {
        // Safety: `Uniforms` is `#[repr(C)]` and `Copy`.
        unsafe { std::slice::from_raw_parts(self as *const Self as *const u8, size_of::<Self>()) }
    }
}
