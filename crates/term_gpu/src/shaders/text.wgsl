// Three scalar pads instead of vec3<f32> — see prim.wgsl for the alignment
// rationale.
struct Uniforms {
    screen_size: vec2<f32>,    // physical
    scroll_offset: vec2<f32>,  // logical
    scale_factor: f32,
    _pad_a: f32,
    _pad_b: f32,
    _pad_c: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct GlyphInput {
    @location(0) pos: vec2<f32>,    // logical
    @location(1) size: vec2<f32>,   // logical
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

const QUAD: array<vec2<f32>, 6> = array(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, g: GlyphInput) -> VsOut {
    let q = QUAD[vi];
    // Subpixel-correct images come from cosmic-text's SubpixelBin (4x4 per
    // glyph). No shader-side snap. Scale logical pixels to physical before NDC.
    let px_logical = g.pos + q * g.size - uniforms.scroll_offset;
    let px_physical = px_logical * uniforms.scale_factor;
    let ndc = (px_physical / uniforms.screen_size) * 2.0 - 1.0;
    var out: VsOut;
    out.pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(g.uv_min, g.uv_max, q);
    out.color = g.color;
    return out;
}

// Luma-dependent contrast enhancement. Lifts perceived weight of
// AA-fringe pixels so light-on-dark text doesn't look anemic on a
// gamma-space (non-sRGB) swap chain. Copied from Warp's
// glyph_shader.wgsl which sources it from Windows Terminal's
// DirectWrite light-text fix. k = REC.601 luma of the foreground
// color; brighter glyphs get a fatter alpha boost.
fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    return alpha * (k + 1.0) / (alpha * k + 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sample = textureSample(atlas_tex, atlas_samp, in.uv);
    // Mono glyphs store coverage in the alpha channel and zero RGB; colour
    // glyphs (emoji) store premultiplied RGBA. Branch on RGB sum.
    let is_color = sample.r + sample.g + sample.b > 0.0;
    if is_color {
        return sample;
    }
    let luma = dot(in.color.rgb, vec3<f32>(0.30, 0.59, 0.11));
    let alpha = enhance_contrast(sample.a, luma);
    return vec4(in.color.rgb, in.color.a * alpha);
}
