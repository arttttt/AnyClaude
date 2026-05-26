//! Tests for the in-process clipboard fallback and the
//! `should_insert_text_on_paste` heuristic.

use term_clipboard::{
    should_insert_text_on_paste, Clipboard, ClipboardContent, ImageData, InMemoryClipboard,
};

fn img(mime: &str, filename: Option<&str>) -> ImageData {
    ImageData {
        data: vec![0x89, 0x50, 0x4E, 0x47], // not real, just non-empty
        mime_type: mime.to_string(),
        filename: filename.map(str::to_string),
    }
}

// ──── InMemoryClipboard ──────────────────────────────────────────────────

#[test]
fn round_trips_plain_text() {
    let mut cb = InMemoryClipboard::default();
    cb.write(ClipboardContent::plain_text("hello".into()));
    let out = cb.read();
    assert_eq!(out.plain_text, "hello");
    assert!(out.paths.is_none() && out.html.is_none() && out.images.is_none());
}

#[test]
fn round_trips_all_fields() {
    let mut cb = InMemoryClipboard::default();
    cb.write(ClipboardContent {
        plain_text: "<b>hi</b>".into(),
        paths: Some(vec!["/tmp/a.png".into(), "/tmp/b.txt".into()]),
        html: Some("<b>hi</b>".into()),
        images: Some(vec![img("image/png", Some("a.png"))]),
    });
    let out = cb.read();
    assert_eq!(out.plain_text, "<b>hi</b>");
    assert_eq!(out.paths.as_deref().map(|p| p.len()), Some(2));
    assert_eq!(out.html.as_deref(), Some("<b>hi</b>"));
    assert_eq!(out.images.as_deref().map(|i| i.len()), Some(1));
}

#[test]
fn primary_and_default_are_independent() {
    let mut cb = InMemoryClipboard::default();
    cb.write(ClipboardContent::plain_text("default".into()));
    cb.write_to_primary_clipboard(ClipboardContent::plain_text("primary".into()));
    assert_eq!(cb.read().plain_text, "default");
    assert_eq!(cb.read_from_primary_clipboard().plain_text, "primary");
}

// ──── ClipboardContent helpers ──────────────────────────────────────────

#[test]
fn is_empty_recognises_empty_content() {
    assert!(ClipboardContent::default().is_empty());
    assert!(ClipboardContent::plain_text(String::new()).is_empty());
    assert!(!ClipboardContent::plain_text("x".into()).is_empty());

    let with_image = ClipboardContent {
        plain_text: String::new(),
        paths: None,
        html: None,
        images: Some(vec![img("image/png", None)]),
    };
    assert!(!with_image.is_empty());
}

#[test]
fn has_image_data_distinguishes_some_empty_vec_from_none() {
    let none = ClipboardContent::default();
    assert!(!none.has_image_data());

    let empty_vec = ClipboardContent {
        images: Some(vec![]),
        ..Default::default()
    };
    assert!(!empty_vec.has_image_data());

    let one = ClipboardContent {
        images: Some(vec![img("image/jpeg", None)]),
        ..Default::default()
    };
    assert!(one.has_image_data());
}

#[test]
fn num_paths_counts_paths_field() {
    assert_eq!(ClipboardContent::default().num_paths(), 0);
    let c = ClipboardContent {
        paths: Some(vec!["a".into(), "b".into(), "c".into()]),
        ..Default::default()
    };
    assert_eq!(c.num_paths(), 3);
}

#[test]
fn has_non_image_filepaths_detects_mixed() {
    let only_images = ClipboardContent {
        paths: Some(vec!["a.png".into(), "b.JPG".into(), "c.gif".into()]),
        ..Default::default()
    };
    assert!(!only_images.has_non_image_filepaths());

    let mixed = ClipboardContent {
        paths: Some(vec!["a.png".into(), "doc.pdf".into()]),
        ..Default::default()
    };
    assert!(mixed.has_non_image_filepaths());

    let no_paths = ClipboardContent::default();
    assert!(!no_paths.has_non_image_filepaths());
}

// ──── should_insert_text_on_paste heuristic ─────────────────────────────

#[test]
fn insert_text_for_plain_only() {
    let c = ClipboardContent::plain_text("hello".into());
    assert!(should_insert_text_on_paste(&c));
}

#[test]
fn insert_text_for_mixed_paths() {
    let c = ClipboardContent {
        plain_text: "label".into(),
        paths: Some(vec!["a.png".into(), "report.pdf".into()]),
        ..Default::default()
    };
    assert!(should_insert_text_on_paste(&c));
}

#[test]
fn insert_text_for_image_data_with_caption() {
    // Image data on clipboard but no file path — user might have
    // attached a caption.
    let c = ClipboardContent {
        plain_text: "caption".into(),
        images: Some(vec![img("image/png", None)]),
        ..Default::default()
    };
    assert!(should_insert_text_on_paste(&c));
}

#[test]
fn skip_text_for_image_path_only() {
    // Paths-only, all images, no plain_text — terminals would just
    // paste the path text, but the heuristic says "skip" because the
    // user probably copied the file itself.
    let c = ClipboardContent {
        plain_text: String::new(),
        paths: Some(vec!["screenshot.png".into()]),
        ..Default::default()
    };
    assert!(!should_insert_text_on_paste(&c));
}
