mod common;

use anyclaude::proxy::thinking::{NativeTransformer, TransformContext};
use serde_json::json;

#[test]
fn native_does_not_modify_request() {
    let transformer = NativeTransformer::new();
    let mut body = json!({
        "messages": [{
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "test thought", "signature": "sig"},
                {"type": "text", "text": "response"}
            ]
        }]
    });

    let original = body.clone();
    let context = TransformContext::new("test", "req-1", "/v1/messages");
    let result = transformer.transform_request(&mut body, &context).unwrap();

    assert!(!result.changed);
    assert_eq!(body, original);
}

#[test]
fn native_name() {
    let transformer = NativeTransformer::new();
    assert_eq!(transformer.name(), "native");
}
