// Three scalar pads instead of vec3<f32> — vec3 in WGSL has align 16, which
// would push the struct size to 48 bytes and mismatch our 32-byte Rust
// Uniforms struct. Scalar f32 has align 4, so total stays 32.
struct Uniforms {
    screen_size: vec2<f32>,    // physical
    scroll_offset: vec2<f32>,  // logical
    scale_factor: f32,
    _pad_a: f32,
    _pad_b: f32,
    _pad_c: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct RectInput {
    @location(0) pos: vec2<f32>,   // logical
    @location(1) size: vec2<f32>,  // logical
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

const QUAD: array<vec2<f32>, 6> = array(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, r: RectInput) -> VsOut {
    let q = QUAD[vi];
    let px_logical = r.pos + q * r.size - uniforms.scroll_offset;
    let px_physical = px_logical * uniforms.scale_factor;
    let ndc = (px_physical / uniforms.screen_size) * 2.0 - 1.0;
    var out: VsOut;
    out.pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.color = r.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
