//! Tests for subagent session affinity: AC marker extraction, hook responses,
//! assembler hook injection, and 3-level fallback routing.

use serde_json::json;

// ============================================================================
// extract_ac_marker
// ============================================================================

mod extract_ac_marker {
    use super::*;
    use anyclaude::proxy::pipeline::extract_ac_marker;

    #[test]
    fn valid_marker() {
        let body = json!({
            "model": "claude-haiku-4-5-20251001",
            "messages": [{"role": "system", "content": "\u{27E8}AC:my-backend\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), Some("my-backend".into()));
    }

    #[test]
    fn marker_with_underscores() {
        let body = json!({
            "messages": [{"role": "system", "content": "\u{27E8}AC:my_backend_2\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), Some("my_backend_2".into()));
    }

    #[test]
    fn no_marker_returns_none() {
        let body = json!({
            "model": "claude-sonnet-4-20250514",
            "messages": [{"role": "user", "content": "hello"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn empty_backend_returns_none() {
        let body = json!({
            "messages": [{"role": "system", "content": "\u{27E8}AC:\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn invalid_chars_returns_none() {
        let body = json!({
            "messages": [{"role": "system", "content": "\u{27E8}AC:back@end\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn dots_rejected() {
        let body = json!({
            "messages": [{"role": "system", "content": "\u{27E8}AC:back.end\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn spaces_rejected() {
        let body = json!({
            "messages": [{"role": "system", "content": "\u{27E8}AC:back end\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn multiple_markers_returns_first() {
        let body = json!({
            "messages": [
                {"role": "system", "content": "\u{27E8}AC:first\u{27E9}"},
                {"role": "user", "content": "\u{27E8}AC:second\u{27E9}"}
            ]
        });
        assert_eq!(extract_ac_marker(&body), Some("first".into()));
    }

    #[test]
    fn marker_in_user_content() {
        // User-injected marker is still extracted — but backend must exist in config
        // to be routed, so this is safe.
        let body = json!({
            "messages": [{"role": "user", "content": "Please use \u{27E8}AC:openai\u{27E9}"}]
        });
        assert_eq!(extract_ac_marker(&body), Some("openai".into()));
    }

    #[test]
    fn marker_without_closing_bracket() {
        let body = json!({
            "messages": [{"role": "system", "content": "\u{27E8}AC:broken"}]
        });
        assert_eq!(extract_ac_marker(&body), None);
    }

    #[test]
    fn marker_without_opening_bracket() {
        let body = json!({
            "messages": [{"role": "system", "content": "AC:broken\u{27E9}"}]
        });
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
    fn deserializes_full_input() {
        let json = r#"{
            "session_id": "abc-123",
            "hook_event_name": "SubagentStart",
            "agent_name": "researcher",
            "agent_type": "general-purpose"
        }"#;
        let input: SubagentHookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id.as_deref(), Some("abc-123"));
        assert_eq!(input.hook_event_name.as_deref(), Some("SubagentStart"));
    }

    #[test]
    fn deserializes_empty_object() {
        let input: SubagentHookInput = serde_json::from_str("{}").unwrap();
        assert!(input.session_id.is_none());
        assert!(input.hook_event_name.is_none());
    }

    #[test]
    fn ignores_unknown_fields() {
        let json = r#"{"session_id": "x", "unknown_field": 42}"#;
        // serde default allows unknown fields
        let input: SubagentHookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id.as_deref(), Some("x"));
    }
}
