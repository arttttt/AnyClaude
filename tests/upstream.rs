//! Tests for upstream client helpers: adaptive thinking conversion and beta header patching.

mod common;

use anyclaude::proxy::upstream::{convert_adaptive_thinking, patch_anthropic_beta_header};

// --- convert_adaptive_thinking ---

#[test]
fn test_convert_adaptive_to_enabled_with_config_budget() {
    let mut body = serde_json::json!({
        "model": "claude-opus-4-6",
        "max_tokens": 32000,
        "thinking": {"type": "adaptive"}
    });
    let result = convert_adaptive_thinking(&mut body, Some(16000));
    assert_eq!(result, Some(true));
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 16000);
}

#[test]
fn test_convert_adaptive_falls_back_to_max_tokens() {
    let mut body = serde_json::json!({
        "model": "claude-opus-4-6",
        "max_tokens": 32000,
        "thinking": {"type": "adaptive"}
    });
    let result = convert_adaptive_thinking(&mut body, None);
    assert_eq!(result, Some(true));
    assert_eq!(body["thinking"]["budget_tokens"], 31999);
}

#[test]
fn test_convert_adaptive_falls_back_to_default() {
    let mut body = serde_json::json!({
        "model": "claude-opus-4-6",
        "thinking": {"type": "adaptive"}
    });
    let result = convert_adaptive_thinking(&mut body, None);
    assert_eq!(result, Some(true));
    assert_eq!(body["thinking"]["budget_tokens"], 10000);
}

#[test]
fn test_convert_enabled_unchanged() {
    let mut body = serde_json::json!({
        "thinking": {"type": "enabled", "budget_tokens": 8000}
    });
    let result = convert_adaptive_thinking(&mut body, Some(16000));
    assert_eq!(result, Some(false));
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 8000);
}

#[test]
fn test_convert_no_thinking_field() {
    let mut body = serde_json::json!({"model": "claude-opus-4-6"});
    let result = convert_adaptive_thinking(&mut body, Some(16000));
    assert_eq!(result, None);
}

// --- patch_anthropic_beta_header ---

#[test]
fn test_patch_header_replaces_adaptive() {
    let header = "claude-code-20250219,adaptive-thinking-2026-01-28,prompt-caching-2024-07-31";
    let patched = patch_anthropic_beta_header(header);
    assert!(!patched.contains("adaptive-thinking"));
    assert!(patched.contains("interleaved-thinking-2025-05-14"));
    assert!(patched.contains("claude-code-20250219"));
    assert!(patched.contains("prompt-caching-2024-07-31"));
}

#[test]
fn test_patch_header_preserves_existing_interleaved() {
    let header = "interleaved-thinking-2025-05-14,prompt-caching-2024-07-31";
    let patched = patch_anthropic_beta_header(header);
    assert_eq!(
        patched.matches("interleaved-thinking").count(),
        1,
        "should not duplicate interleaved-thinking"
    );
}

#[test]
fn test_patch_header_no_adaptive_adds_interleaved() {
    let header = "claude-code-20250219,prompt-caching-2024-07-31";
    let patched = patch_anthropic_beta_header(header);
    assert!(patched.contains("interleaved-thinking-2025-05-14"));
}

#[test]
fn test_patch_header_only_adaptive() {
    let header = "adaptive-thinking-2026-01-28";
    let patched = patch_anthropic_beta_header(header);
    assert_eq!(patched, "interleaved-thinking-2025-05-14");
}
