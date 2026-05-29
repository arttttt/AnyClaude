//! ShelfPacker — the per-layer bin packer behind the glyph atlas. The
//! multi-layer growth + reclaim logic lives on `GlyphAtlas` (needs a wgpu
//! device, so it's verified by running the app), but the packer's fill / reset
//! / is_empty contract — which the layer reclaim relies on — is GPU-free.

use term_gpu::ShelfPacker;

#[test]
fn fresh_packer_is_empty() {
    assert!(ShelfPacker::new(64, 64).is_empty());
}

#[test]
fn pack_places_then_marks_nonempty() {
    let mut p = ShelfPacker::new(64, 64);
    // +1px padding on each side, so the interior starts at (1, 1).
    assert_eq!(p.pack(10, 10), Some((1, 1)));
    assert!(!p.is_empty());
}

#[test]
fn glyph_wider_than_atlas_never_fits() {
    let mut p = ShelfPacker::new(64, 64);
    assert_eq!(p.pack(100, 10), None);
}

#[test]
fn packer_fills_and_then_returns_none() {
    let mut p = ShelfPacker::new(64, 64);
    let mut packed = 0;
    while p.pack(10, 10).is_some() {
        packed += 1;
        assert!(packed <= 1000, "packer should fill before 1000 glyphs");
    }
    assert!(packed > 0);
    assert_eq!(p.pack(10, 10), None, "full packer rejects further glyphs");
}

#[test]
fn reset_reclaims_the_whole_packer() {
    let mut p = ShelfPacker::new(64, 64);
    while p.pack(10, 10).is_some() {}
    assert_eq!(p.pack(10, 10), None);
    p.reset();
    assert!(p.is_empty());
    assert!(p.pack(10, 10).is_some(), "reset re-enables packing");
}

#[test]
fn row_overflow_advances_to_a_new_shelf() {
    // Width 30 fits two 10px (+2px pad = 12px) glyphs per row; the third wraps.
    let mut p = ShelfPacker::new(30, 64);
    let a = p.pack(10, 10).unwrap();
    let b = p.pack(10, 10).unwrap();
    let c = p.pack(10, 10).unwrap();
    assert_eq!(a.1, b.1, "first two share a shelf (same y)");
    assert!(c.1 > a.1, "third wraps onto a new shelf (greater y)");
}
