// Rounded-rectangle shader: one instance paints a rounded fill plus an optional
// border ring via a signed-distance field. `corner_radius == 0` is a sharp rect;
// `border_width == 0` is a plain fill. Anti-aliasing uses screen-space
// derivatives (`fwidth`) so the edge stays ~1px crisp at any DPI.

struct Uniforms {
    screen_size: vec2<f32>,    // physical
    scroll_offset: vec2<f32>,  // logical
    scale_factor: f32,
    _pad_a: f32,
    _pad_b: f32,
    _pad_c: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct RoundRectInput {
    @location(0) pos: vec2<f32>,           // top-left, logical
    @location(1) size: vec2<f32>,          // logical
    @location(2) fill_color: vec4<f32>,    // straight (non-premultiplied)
    @location(3) border_color: vec4<f32>,
    @location(4) border_width: f32,        // logical px (0 = no border)
    @location(5) corner_radius: f32,       // logical px (0 = sharp)
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) frag_logical: vec2<f32>,  // fragment position, logical (pre-scroll)
    @location(1) center: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) fill_color: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) border_width: f32,
    @location(6) corner_radius: f32,
};

const QUAD: array<vec2<f32>, 6> = array(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, r: RoundRectInput) -> VsOut {
    let q = QUAD[vi];
    let frag_logical = r.pos + q * r.size;
    let px_logical = frag_logical - uniforms.scroll_offset;
    let px_physical = px_logical * uniforms.scale_factor;
    let ndc = (px_physical / uniforms.screen_size) * 2.0 - 1.0;

    var out: VsOut;
    out.clip_pos = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.frag_logical = frag_logical;
    out.center = r.pos + r.size * 0.5;
    out.half_size = r.size * 0.5;
    out.fill_color = r.fill_color;
    out.border_color = r.border_color;
    out.border_width = r.border_width;
    out.corner_radius = r.corner_radius;
    return out;
}

// Signed distance to a rounded rectangle (negative inside).
fn sdf_round_rect(p: vec2<f32>, center: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p - center) - (half - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = sdf_round_rect(in.frag_logical, in.center, in.half_size, max(in.corner_radius, 0.0));
    let aa = max(fwidth(d), 1e-4);
    // Outer coverage: 1 inside the shape, fading to 0 across ~1px at the edge.
    let outer = clamp(0.5 - d / aa, 0.0, 1.0);

    if (in.border_width <= 0.0) {
        return vec4(in.fill_color.rgb, in.fill_color.a * outer);
    }
    // Inner coverage: 1 inside the fill region (shape shrunk by border_width).
    let inner = clamp(0.5 - (d + in.border_width) / aa, 0.0, 1.0);
    let rgb = mix(in.border_color.rgb, in.fill_color.rgb, inner);
    let a = mix(in.border_color.a, in.fill_color.a, inner);
    return vec4(rgb, a * outer);
}
