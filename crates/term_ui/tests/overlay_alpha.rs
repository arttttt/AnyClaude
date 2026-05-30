//! `term_ui::anim` — the overlay-alpha bake + easing curves (E.7.0). The bake
//! multiplies a global opacity into every overlay instance's alpha channel
//! (rects, glyphs, shadows) while leaving RGB untouched; the curves hit their
//! endpoints and stay monotonic. All headless — these are pure functions over a
//! hand-built `PaintOutput`, no GPU.

use term_gpu::{GlyphInstance, RectInstance, ShadowInstance};
use term_ui::{apply_overlay_alpha, ease_in_out, ease_out, linear, PaintOutput};

fn approx(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

fn sample() -> PaintOutput {
    let mut out = PaintOutput::default();
    out.rects.push(RectInstance {
        pos: [0.0, 0.0],
        size: [10.0, 10.0],
        color: [0.2, 0.4, 0.6, 0.8],
    });
    out.glyphs.push(GlyphInstance {
        pos: [1.0, 2.0],
        size: [3.0, 4.0],
        uv_min: [0.0, 0.0],
        uv_max: [1.0, 1.0],
        color: [0.9, 0.8, 0.7, 1.0],
        layer: 0,
    });
    out.shadows.push(ShadowInstance {
        pos: [0.0, 0.0],
        size: [10.0, 10.0],
        blur_radius: 24.0,
        corner_radius: 6.0,
        offset: [0.0, 8.0],
        color: [0.1, 0.0, 0.0, 0.45],
    });
    out
}

#[test]
fn alpha_scales_every_layers_alpha_and_leaves_rgb() {
    let mut out = sample();
    apply_overlay_alpha(&mut out, 0.5);
    assert!(approx(out.rects[0].color, [0.2, 0.4, 0.6, 0.4]), "rect: {:?}", out.rects[0].color);
    assert!(approx(out.glyphs[0].color, [0.9, 0.8, 0.7, 0.5]), "glyph: {:?}", out.glyphs[0].color);
    assert!(
        approx(out.shadows[0].color, [0.1, 0.0, 0.0, 0.225]),
        "shadow rgb untouched, alpha halved: {:?}",
        out.shadows[0].color
    );
}

#[test]
fn alpha_one_is_identity() {
    let before = sample();
    let mut out = sample();
    apply_overlay_alpha(&mut out, 1.0);
    assert!(approx(out.rects[0].color, before.rects[0].color));
    assert!(approx(out.glyphs[0].color, before.glyphs[0].color));
    assert!(approx(out.shadows[0].color, before.shadows[0].color));
}

#[test]
fn alpha_is_clamped_both_ends() {
    let mut hi = sample();
    apply_overlay_alpha(&mut hi, 2.0); // clamps to 1.0 → alpha unchanged
    assert!((hi.rects[0].color[3] - 0.8).abs() < 1e-6);

    let mut lo = sample();
    apply_overlay_alpha(&mut lo, -1.0); // clamps to 0.0 → fully transparent
    assert!(lo.rects[0].color[3].abs() < 1e-6);
    assert!(lo.glyphs[0].color[3].abs() < 1e-6);
    assert!(lo.shadows[0].color[3].abs() < 1e-6);
}

#[test]
fn easing_hits_endpoints_and_is_monotonic() {
    assert!(linear(0.0).abs() < 1e-6 && (linear(1.0) - 1.0).abs() < 1e-6);
    assert!(ease_out(0.0).abs() < 1e-6 && (ease_out(1.0) - 1.0).abs() < 1e-6);
    assert!(ease_in_out(0.0).abs() < 1e-6, "ease_in_out(0) = {}", ease_in_out(0.0));
    assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6, "ease_in_out(0.5) = {}", ease_in_out(0.5));
    assert!((ease_in_out(1.0) - 1.0).abs() < 1e-6, "ease_in_out(1) = {}", ease_in_out(1.0));

    // Non-decreasing across the unit interval (both curves).
    let mut prev_out = -1.0;
    let mut prev_io = -1.0;
    for i in 0..=20 {
        let t = i as f32 / 20.0;
        let o = ease_out(t);
        let io = ease_in_out(t);
        assert!(o >= prev_out - 1e-6, "ease_out not monotonic at t={t}: {o} < {prev_out}");
        assert!(io >= prev_io - 1e-6, "ease_in_out not monotonic at t={t}: {io} < {prev_io}");
        prev_out = o;
        prev_io = io;
    }
}

#[test]
fn easing_clamps_out_of_range_input() {
    assert!(ease_out(-0.5).abs() < 1e-6);
    assert!((ease_out(1.5) - 1.0).abs() < 1e-6);
    assert!(ease_in_out(-0.5).abs() < 1e-6);
    assert!((ease_in_out(1.5) - 1.0).abs() < 1e-6);
}
