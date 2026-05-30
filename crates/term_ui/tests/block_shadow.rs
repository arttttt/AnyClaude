//! `term_ui::block_shadow` — the Block → drop-shadow emit mapping (E.7.0).
//! The live `paint` path needs a GPU atlas, so the shadow-emit decision is
//! split into this pure helper and asserted headlessly: a styled shadow maps
//! its fields onto a `ShadowInstance` at the Block's bounds; `None` / a fully
//! transparent colour emits nothing (the regression guard for every plain
//! chrome Block, whose `shadow` is `None`).

use glam::Vec2;
use term_ui::{block_shadow, BlockShadow, BlockStyle, Bounds, Insets};

fn bounds() -> Bounds {
    Bounds::new(Vec2::new(10.0, 20.0), Vec2::new(300.0, 180.0))
}

fn style(shadow: Option<BlockShadow>) -> BlockStyle {
    BlockStyle {
        background: [0.1, 0.1, 0.12, 1.0],
        border_color: [0.0; 4],
        border_width: 0.0,
        padding: Insets::default(),
        shadow,
    }
}

#[test]
fn some_shadow_maps_fields_onto_the_block_bounds() {
    let sh = BlockShadow {
        blur_radius: 24.0,
        corner_radius: 6.0,
        offset: [0.0, 8.0],
        color: [0.0, 0.0, 0.0, 0.45],
    };
    let s = block_shadow(bounds(), &style(Some(sh))).expect("a visible shadow emits");
    assert_eq!(s.pos, [10.0, 20.0], "shadow sits at the block origin");
    assert_eq!(s.size, [300.0, 180.0], "shadow covers the block bounds");
    assert_eq!(s.blur_radius, 24.0);
    assert_eq!(s.corner_radius, 6.0);
    assert_eq!(s.offset, [0.0, 8.0]);
    assert_eq!(s.color, [0.0, 0.0, 0.0, 0.45]);
}

#[test]
fn none_shadow_emits_nothing() {
    assert!(
        block_shadow(bounds(), &style(None)).is_none(),
        "shadow:None is the plain-Block case — no instance"
    );
}

#[test]
fn fully_transparent_shadow_emits_nothing() {
    let sh = BlockShadow {
        blur_radius: 24.0,
        corner_radius: 6.0,
        offset: [0.0, 8.0],
        color: [0.0, 0.0, 0.0, 0.0],
    };
    assert!(
        block_shadow(bounds(), &style(Some(sh))).is_none(),
        "a fully transparent shadow is skipped (no wasted instance)"
    );
}
