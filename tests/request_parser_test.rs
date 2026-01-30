use claudewrapper::metrics::request_parser::RequestParser;
use serde_json::json;

#[test]
fn test_parse_model_info() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "max_tokens": 4096,
        "temperature": 0.7
    })
    .to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert_eq!(analysis.model, Some("claude-3-opus-20240229".to_string()));
    assert_eq!(analysis.max_tokens, Some(4096));
    assert_eq!(analysis.temperature, Some(0.7));
}

#[test]
fn test_parse_messages() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "messages": [
            {"role": "system", "content": "You are helpful"},
            {"role": "user", "content": "Hello"}
        ]
    })
    .to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert_eq!(analysis.message_count, 2);
    assert!(analysis.has_system_prompt);
}

#[test]
fn test_detect_images() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "What's in this image?"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                        }
                    }
                ]
            }
        ]
    }).to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert!(analysis.has_images);
    assert_eq!(analysis.image_count, 1);
    assert!(analysis.total_image_bytes > 0);
}

#[test]
fn test_detect_tools() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "tools": [
            {"name": "search", "description": "Search the web"},
            {"name": "calculate", "description": "Do math"}
        ]
    })
    .to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert!(analysis.has_tools);
    assert_eq!(analysis.tool_names, vec!["search", "calculate"]);
}

#[test]
fn test_detect_thinking() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "thinking": {
            "enabled": true,
            "budget_tokens": 5000
        }
    })
    .to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert!(analysis.thinking_enabled);
    assert_eq!(analysis.thinking_budget, Some(5000));
}

#[test]
fn test_estimate_tokens() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "messages": [
            {"role": "user", "content": "This is a test message with some text"}
        ]
    })
    .to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert!(analysis.estimated_input_tokens.is_some());
    assert!(analysis.estimated_input_tokens.unwrap() > 0);
}

#[test]
fn test_invalid_json_returns_default() {
    let parser = RequestParser::new(true);
    let body = "{invalid json}";

    let analysis = parser.parse_request(body.as_bytes());

    assert_eq!(analysis.model, None);
    assert_eq!(analysis.max_tokens, None);
    assert_eq!(analysis.message_count, 0);
    assert!(!analysis.has_images);
}

#[test]
fn test_complex_request_with_all_fields() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "max_tokens": 4096,
        "temperature": 0.7,
        "messages": [
            {"role": "system", "content": "You are helpful"},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "What's in this image?"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                        }
                    }
                ]
            }
        ],
        "tools": [{"name": "search", "description": "Search"}],
        "thinking": {
            "enabled": true,
            "budget_tokens": 5000
        }
    }).to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert_eq!(analysis.model, Some("claude-3-opus-20240229".to_string()));
    assert_eq!(analysis.max_tokens, Some(4096));
    assert_eq!(analysis.temperature, Some(0.7));
    assert_eq!(analysis.message_count, 2);
    assert!(analysis.has_system_prompt);
    assert!(analysis.has_images);
    assert_eq!(analysis.image_count, 1);
    assert!(analysis.has_tools);
    assert_eq!(analysis.tool_names, vec!["search"]);
    assert!(analysis.thinking_enabled);
    assert_eq!(analysis.thinking_budget, Some(5000));
}

#[test]
fn test_multiple_images() {
    let parser = RequestParser::new(true);
    let body = json!({
        "model": "claude-3-opus-20240229",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Analyze these"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                        }
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAP//////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA="
                        }
                    }
                ]
            }
        ]
    }).to_string();

    let analysis = parser.parse_request(body.as_bytes());

    assert!(analysis.has_images);
    assert_eq!(analysis.image_count, 2);
    assert!(analysis.total_image_bytes > 0);
}
