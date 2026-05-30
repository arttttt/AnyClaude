// Drop-shadow shader for UI overlays (popups, command palette, menus).
//
// Single-pass approximation: the vertex stage expands a quad outward
// by `blur_radius + |offset|` around the content rect; the fragment
// stage evaluates an SDF for a rounded rectangle at the content
// position (with the shadow offset applied) and smoothsteps from 1.0
// inside the content area to 0.0 at the blur frontier.
//
// The content rect that sits on top (drawn AFTER this pass) covers
// the saturated interior, so the visible shadow is a soft halo around
// the popup. No offscreen render target; no Gaussian convolution; one
// instance, six vertices.

struct Uniforms {
    screen_size: vec2<f32>,
    scroll_offset: vec2<f32>,
    scale_factor: f32,
    _pad_a: f32,
    _pad_b: f32,
    _pad_c: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct ShadowInput {
    @location(0) pos: vec2<f32>,            // content top-left, logical px
    @location(1) size: vec2<f32>,           // content size, logical px
    @location(2) blur_radius: f32,          // logical px
    @location(3) corner_radius: f32,        // logical px
    @location(4) offset: vec2<f32>,         // logical px — shadow shift
    @location(5) color: vec4<f32>,          // straight (non-premultiplied)
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) frag_logical: vec2<f32>,   // current fragment position (logical px)
    @location(1) content_min: vec2<f32>,    // content top-left after offset (logical)
    @location(2) content_max: vec2<f32>,    // content bottom-right after offset (logical)
    @location(3) blur_radius: f32,
    @location(4) corner_radius: f32,
    @location(5) color: vec4<f32>,
};

const QUAD: array<vec2<f32>, 6> = array(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, s: ShadowInput) -> VsOut {
    // Expanded quad covers the content rect plus the shadow halo:
    //   - blur_radius outwards on every side
    //   - whatever direction the offset pushes the rect
    let q = QUAD[vi];
    let pad_lo = vec2<f32>(s.blur_radius) + max(-s.offset, vec2<f32>(0.0));
    let pad_hi = vec2<f32>(s.blur_radius) + max(s.offset, vec2<f32>(0.0));
    let quad_min = s.pos - pad_lo;
    let quad_max = s.pos + s.size + pad_hi;
    let frag_pos_logical = mix(quad_min, quad_max, q);

    let px_logical = frag_pos_logical - uniforms.scroll_offset;
    let px_physical = px_logical * uniforms.scale_factor;
    let ndc = (px_physical / uniforms.screen_size) * 2.0 - 1.0;

    var out: VsOut;
    out.clip_pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.frag_logical = frag_pos_logical;
    out.content_min = s.pos + s.offset;
    out.content_max = s.pos + s.size + s.offset;
    out.blur_radius = s.blur_radius;
    out.corner_radius = s.corner_radius;
    out.color = s.color;
    return out;
}

// Signed distance from `p` to a rounded rectangle defined by min/max
// corners and `r`. Negative inside, positive outside.
fn sdf_rounded_rect(p: vec2<f32>, c_min: vec2<f32>, c_max: vec2<f32>, r: f32) -> f32 {
    let center = (c_min + c_max) * 0.5;
    let half = (c_max - c_min) * 0.5;
    let q = abs(p - center) - (half - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = sdf_rounded_rect(in.frag_logical, in.content_min, in.content_max, in.corner_radius);
    // d <= 0 inside content (saturated; content rect overlays it).
    // d in (0, blur_radius) fades from 1.0 to 0.0.
    // d >= blur_radius outside the halo.
    let shadow_alpha = 1.0 - smoothstep(0.0, in.blur_radius, d);
    // Non-premultiplied output to match the rect / text pipelines'
    // ALPHA_BLENDING state.
    return vec4(in.color.rgb, in.color.a * shadow_alpha);
}
