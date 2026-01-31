use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use crate::config::ThinkingMode;

const DEFAULT_SIGNATURE_CACHE_SIZE: usize = 2048;

#[derive(Debug, Default, Clone)]
pub struct ThinkingTransformResult {
    pub changed: bool,
    pub drop_count: u32,
    pub convert_count: u32,
    pub tag_count: u32,
}

#[derive(Debug)]
pub struct ThinkingTransformOutput {
    pub body: Option<Vec<u8>>,
    pub result: ThinkingTransformResult,
}

pub struct ThinkingTracker {
    mode: ThinkingMode,
    last_backend: Option<String>,
    signatures: SignatureCache,
}

impl ThinkingTracker {
    pub fn new(mode: ThinkingMode) -> Self {
        Self {
            mode,
            last_backend: None,
            signatures: SignatureCache::new(DEFAULT_SIGNATURE_CACHE_SIZE),
        }
    }

    pub fn set_mode(&mut self, mode: ThinkingMode) {
        self.mode = mode;
    }

    pub fn transform_request(
        &mut self,
        target_backend: &str,
        body: &[u8],
    ) -> ThinkingTransformOutput {
        let mut result = ThinkingTransformResult::default();
        let mut json: Value = match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(_) => {
                self.last_backend = Some(target_backend.to_string());
                return ThinkingTransformOutput { body: None, result };
            }
        };

        let mut changed = false;
        if let Some(messages) = json
            .get_mut("messages")
            .and_then(|value| value.as_array_mut())
        {
            for message in messages {
                let Some(content) = message.get_mut("content") else {
                    continue;
                };

                if let Value::Array(items) = content {
                    for item in items.iter_mut() {
                        let Some(obj) = item.as_object_mut() else {
                            continue;
                        };

                        let Some(item_type) = obj.get("type").and_then(|v| v.as_str()) else {
                            continue;
                        };

                        if item_type != "thinking" {
                            continue;
                        }

                        let signature = obj.get("signature").and_then(|v| v.as_str());
                        let text = obj
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| obj.get("thinking").and_then(|v| v.as_str()))
                            .unwrap_or("");

                        let source_backend = self.resolve_source_backend(signature, target_backend);

                        if source_backend == target_backend {
                            if let Some(sig) = signature {
                                self.signatures
                                    .insert(sig.to_string(), target_backend.to_string());
                            }
                            continue;
                        }

                        match self.mode {
                            ThinkingMode::DropSignature => {
                                if obj.remove("signature").is_some() {
                                    result.drop_count = result.drop_count.saturating_add(1);
                                    changed = true;
                                }
                            }
                            ThinkingMode::ConvertToText => {
                                *item = serde_json::json!({
                                    "type": "text",
                                    "text": text,
                                });
                                result.convert_count = result.convert_count.saturating_add(1);
                                changed = true;
                            }
                            ThinkingMode::ConvertToTags => {
                                *item = serde_json::json!({
                                    "type": "text",
                                    "text": format!("<think>{}</think>", text),
                                });
                                result.tag_count = result.tag_count.saturating_add(1);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }

        result.changed = changed;
        self.last_backend = Some(target_backend.to_string());

        if changed {
            let body = serde_json::to_vec(&json).ok();
            return ThinkingTransformOutput { body, result };
        }

        ThinkingTransformOutput { body: None, result }
    }

    fn resolve_source_backend(&mut self, signature: Option<&str>, target_backend: &str) -> String {
        let Some(signature) = signature else {
            return target_backend.to_string();
        };

        if let Some(mapped) = self.signatures.get(signature) {
            return mapped;
        }

        let switched = self
            .last_backend
            .as_deref()
            .map(|backend| backend != target_backend)
            .unwrap_or(false);

        if switched {
            return self
                .last_backend
                .clone()
                .unwrap_or_else(|| target_backend.to_string());
        }

        target_backend.to_string()
    }
}

struct SignatureCache {
    max_entries: usize,
    entries: HashMap<String, (String, u64)>,
    order: VecDeque<(String, u64)>,
    counter: u64,
}

impl SignatureCache {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: HashMap::new(),
            order: VecDeque::new(),
            counter: 0,
        }
    }

    fn get(&mut self, signature: &str) -> Option<String> {
        let (backend, _) = self.entries.get(signature)?.clone();
        self.touch(signature.to_string(), backend.clone());
        Some(backend)
    }

    fn insert(&mut self, signature: String, backend: String) {
        self.touch(signature, backend);
    }

    fn touch(&mut self, signature: String, backend: String) {
        self.counter = self.counter.saturating_add(1);
        let seq = self.counter;
        self.entries.insert(signature.clone(), (backend, seq));
        self.order.push_back((signature, seq));
        self.evict();
    }

    fn evict(&mut self) {
        while self.entries.len() > self.max_entries {
            let Some((signature, seq)) = self.order.pop_front() else {
                break;
            };

            let should_remove = self
                .entries
                .get(&signature)
                .map(|(_, current_seq)| *current_seq == seq)
                .unwrap_or(false);

            if should_remove {
                self.entries.remove(&signature);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_with_thinking(signature: &str) -> Vec<u8> {
        serde_json::json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "text": "ponder", "signature": signature},
                        {"type": "text", "text": "hello"}
                    ]
                }
            ]
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn keeps_signature_for_same_backend() {
        let mut tracker = ThinkingTracker::new(ThinkingMode::DropSignature);
        let body = body_with_thinking("sig-1");
        let output = tracker.transform_request("anthropic", &body);
        assert!(!output.result.changed);
        assert!(output.body.is_none());
        assert!(tracker.signatures.get("sig-1").is_some());
    }

    #[test]
    fn drops_signature_on_switch() {
        let mut tracker = ThinkingTracker::new(ThinkingMode::DropSignature);
        let body = body_with_thinking("sig-1");
        let _ = tracker.transform_request("anthropic", &body);
        let output = tracker.transform_request("glm", &body);
        assert!(output.result.changed);
        let transformed = String::from_utf8(output.body.unwrap()).unwrap();
        assert!(!transformed.contains("\"signature\""));
        assert_eq!(output.result.drop_count, 1);
    }

    #[test]
    fn converts_thinking_to_text() {
        let mut tracker = ThinkingTracker::new(ThinkingMode::ConvertToText);
        let body = body_with_thinking("sig-1");
        let _ = tracker.transform_request("anthropic", &body);
        let output = tracker.transform_request("glm", &body);
        assert!(output.result.changed);
        let transformed = String::from_utf8(output.body.unwrap()).unwrap();
        assert!(transformed.contains("\"type\":\"text\""));
        assert_eq!(output.result.convert_count, 1);
    }

    #[test]
    fn converts_thinking_to_tags() {
        let mut tracker = ThinkingTracker::new(ThinkingMode::ConvertToTags);
        let body = body_with_thinking("sig-1");
        let _ = tracker.transform_request("anthropic", &body);
        let output = tracker.transform_request("glm", &body);
        assert!(output.result.changed);
        let transformed = String::from_utf8(output.body.unwrap()).unwrap();
        assert!(transformed.contains("<think>ponder</think>"));
        assert_eq!(output.result.tag_count, 1);
    }
}
