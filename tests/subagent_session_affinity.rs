//! Tests for subagent session affinity: AC marker extraction, hook responses,
//! assembler hook injection, and 3-level fallback routing.

use serde_json::json;

// ============================================================================
// extract_ac_marker
// ============================================================================

mod extract_ac_marker {
    use super::*;
    use anyclaude::proxy::pipeline::extract_ac_marker;

    /// Helper: wrap marker text in the CC hook context format.
    fn hook_msg(marker: &str) -> serde_json::Value {
        json!({
            "role": "user",
            "content": format!(
                "<system-reminder>\nSubagentStart hook additional context: {}\n</system-reminder>",
                marker
            )
        })
    }

    /// Helper: same but as content block array.
    fn hook_msg_blocks(marker: &str) -> serde_json::Value {
        json!({
            "role": "user",
            "content": [
                {"type": "text", "text": format!(
                    "<system-reminder>\nSubagentStart hook additional context: {}\n</system-reminder>",
                    marker
                )}
            ]
        })
    }

    // === Positive cases ===

    #[test]
    fn valid_marker_in_hook_context() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), Some("a1b2c3d4e5f6a7b8".into()));
    }

    #[test]
    fn marker_with_hyphens_and_underscores() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:a1b2-c3d4_e5f6\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), Some("a1b2-c3d4_e5f6".into()));
    }

    #[test]
    fn marker_in_content_block_array() {
        let body = json!({
            "messages": [hook_msg_blocks("\u{27E8}AC:abcdef1234567890\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), Some("abcdef1234567890".into()));
    }

    #[test]
    fn marker_in_second_message() {
        let body = json!({
            "messages": [
                {"role": "user", "content": "some preamble"},
                hook_msg("\u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}")
            ]
        });
        assert_eq!(extract_ac_marker(&body), Some("a1b2c3d4e5f6a7b8".into()));
    }

    // === Negative cases: no false positives ===

    #[test]
    fn no_marker_returns_none() {
        let body = json!({
            "messages": [{"role": "user", "content": "hello"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn no_messages_field_returns_none() {
        let body = json!({"model": "test"});
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn marker_without_hook_prefix_rejected() {
        // Bare marker in user text — no hook context prefix
        let body = json!({
            "messages": [{"role": "user", "content": "\u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn marker_in_assistant_message_rejected() {
        let body = json!({
            "messages": [{
                "role": "assistant",
                "content": format!(
                    "SubagentStart hook additional context: \u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}"
                )
            }]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn marker_in_system_role_rejected() {
        let body = json!({
            "messages": [{
                "role": "system",
                "content": format!(
                    "SubagentStart hook additional context: \u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}"
                )
            }]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn marker_beyond_scan_limit_rejected() {
        // Marker in 4th message — beyond MAX_MESSAGES_TO_SCAN (3)
        let body = json!({
            "messages": [
                {"role": "user", "content": "msg 1"},
                {"role": "assistant", "content": "msg 2"},
                {"role": "user", "content": "msg 3"},
                hook_msg("\u{27E8}AC:a1b2c3d4e5f6a7b8\u{27E9}")
            ]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn empty_id_rejected() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn non_hex_chars_rejected() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:back@end\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn dots_rejected() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:back.end\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn spaces_rejected() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:back end\u{27E9}")]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn marker_without_closing_bracket() {
        let body = json!({
            "messages": [hook_msg("\u{27E8}AC:broken")]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn user_text_mimicking_hook_context_rejected() {
        // User writes something that looks like hook context but isn't
        // the exact prefix — should not match
        let body = json!({
            "messages": [{"role": "user", "content": "I saw SubagentStart hook additional context: \u{27E8}AC:fake\u{27E9} in the logs"}]
        });
        // This actually matches the prefix — but "fake" is valid hex? No, 'k' is not hex.
        // Wait — 'a'-'f' are hex digits. "fake" has 'k' which is not hex. So rejected by char validation.
        assert_eq!(extract_ac_marker(&body), None);
    }
}

// ============================================================================
// with_subagent_hooks (ArgAssembler)
// ============================================================================

mod assembler_hooks {
    use anyclaude::args::ArgAssembler;

    #[test]
    fn adds_settings_flag() {
        let args = ArgAssembler::new().with_subagent_hooks(4000).build();
        assert!(args.contains(&"--settings".to_string()));
    }

    #[test]
    fn settings_json_is_valid() {
        let args = ArgAssembler::new().with_subagent_hooks(4000).build();
        let idx = args.iter().position(|a| a == "--settings").unwrap();
        let json_str = &args[idx + 1];
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .expect("--settings value must be valid JSON");
        assert!(parsed.get("hooks").is_some());
    }

    #[test]
    fn json_contains_both_hooks() {
        let args = ArgAssembler::new().with_subagent_hooks(4000).build();
        let idx = args.iter().position(|a| a == "--settings").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&args[idx + 1]).unwrap();
        let hooks = parsed.get("hooks").unwrap();
        assert!(hooks.get("SubagentStart").is_some(), "missing SubagentStart");
        assert!(hooks.get("SubagentStop").is_some(), "missing SubagentStop");
    }

    #[test]
    fn curl_contains_correct_port() {
        let args = ArgAssembler::new().with_subagent_hooks(4321).build();
        let idx = args.iter().position(|a| a == "--settings").unwrap();
        let json_str = &args[idx + 1];
        assert!(
            json_str.contains("127.0.0.1:4321"),
            "port not found in curl command"
        );
    }

    #[test]
    fn curl_has_timeout() {
        let args = ArgAssembler::new().with_subagent_hooks(4000).build();
        let idx = args.iter().position(|a| a == "--settings").unwrap();
        let json_str = &args[idx + 1];
        assert!(json_str.contains("-m 5"), "curl must have -m 5 timeout");
    }

    #[test]
    fn hook_structure_has_matcher_and_command() {
        let args = ArgAssembler::new().with_subagent_hooks(5000).build();
        let idx = args.iter().position(|a| a == "--settings").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&args[idx + 1]).unwrap();

        let start_hooks = &parsed["hooks"]["SubagentStart"][0];
        assert_eq!(start_hooks["matcher"], "");
        assert_eq!(start_hooks["hooks"][0]["type"], "command");
        assert!(start_hooks["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("subagent-start"));

        let stop_hooks = &parsed["hooks"]["SubagentStop"][0];
        assert!(stop_hooks["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("subagent-stop"));
    }
}

// ============================================================================
// SubagentStartResponse serialization
// ============================================================================

mod hook_response {
    use anyclaude::proxy::hooks::{HookSpecificOutput, SubagentStartResponse};

    #[test]
    fn response_with_backend() {
        let resp = SubagentStartResponse {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "SubagentStart".into(),
                additional_context: Some("\u{27E8}AC:my-backend\u{27E9}".into()),
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json["hookSpecificOutput"]["hookEventName"],
            "SubagentStart"
        );
        assert_eq!(
            json["hookSpecificOutput"]["additionalContext"],
            "\u{27E8}AC:my-backend\u{27E9}"
        );
    }

    #[test]
    fn response_without_backend_omits_context() {
        let resp = SubagentStartResponse {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "SubagentStart".into(),
                additional_context: None,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json["hookSpecificOutput"]["hookEventName"],
            "SubagentStart"
        );
        // additionalContext should be absent (skip_serializing_if = "Option::is_none")
        assert!(json["hookSpecificOutput"].get("additionalContext").is_none());
    }

    #[test]
    fn hook_event_name_is_correct() {
        let resp = SubagentStartResponse {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "SubagentStart".into(),
                additional_context: None,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json["hookSpecificOutput"]["hookEventName"]
                .as_str()
                .unwrap(),
            "SubagentStart"
        );
    }
}

// ============================================================================
// SubagentHookInput deserialization
// ============================================================================

mod hook_input {
    use anyclaude::proxy::hooks::SubagentHookInput;

    #[test]
    fn deserializes_with_agent_id() {
        let json = r#"{
            "session_id": "abc-123",
            "hook_event_name": "SubagentStart",
            "agent_id": "a1b2c3d4e5f6a7b8",
            "agent_type": "general-purpose"
        }"#;
        let input: SubagentHookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.agent_id.as_deref(), Some("a1b2c3d4e5f6a7b8"));
        assert_eq!(input.session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn deserializes_empty_object() {
        let input: SubagentHookInput = serde_json::from_str("{}").unwrap();
        assert!(input.agent_id.is_none());
        assert!(input.session_id.is_none());
    }

    #[test]
    fn ignores_unknown_fields() {
        let json = r#"{"agent_id": "a1234", "unknown_field": 42}"#;
        let input: SubagentHookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.agent_id.as_deref(), Some("a1234"));
    }

    #[test]
    fn agent_id_none_when_missing() {
        let input: SubagentHookInput = serde_json::from_str(
            r#"{"session_id": "sess-42"}"#
        ).unwrap();
        assert!(input.agent_id.is_none());
        assert_eq!(input.session_id.as_deref(), Some("sess-42"));
    }
}

// ============================================================================
// SubagentRegistry
// ============================================================================

mod registry {
    use anyclaude::backend::SubagentRegistry;

    #[test]
    fn register_and_lookup() {
        let reg = SubagentRegistry::new();
        reg.register("sess-1", "openrouter");
        assert_eq!(reg.lookup("sess-1"), Some("openrouter".into()));
    }

    #[test]
    fn lookup_missing_returns_none() {
        let reg = SubagentRegistry::new();
        assert_eq!(reg.lookup("nonexistent"), None);
    }

    #[test]
    fn remove_cleans_up() {
        let reg = SubagentRegistry::new();
        reg.register("sess-1", "kimi");
        reg.remove("sess-1");
        assert_eq!(reg.lookup("sess-1"), None);
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let reg = SubagentRegistry::new();
        reg.remove("nonexistent"); // should not panic
    }

    #[test]
    fn multiple_entries() {
        let reg = SubagentRegistry::new();
        reg.register("a", "backend-1");
        reg.register("b", "backend-2");
        assert_eq!(reg.lookup("a"), Some("backend-1".into()));
        assert_eq!(reg.lookup("b"), Some("backend-2".into()));
    }

    #[test]
    fn overwrite_existing() {
        let reg = SubagentRegistry::new();
        reg.register("sess-1", "old-backend");
        reg.register("sess-1", "new-backend");
        assert_eq!(reg.lookup("sess-1"), Some("new-backend".into()));
    }
}
