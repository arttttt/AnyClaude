use serde_json::Value;

use super::types::ResponseAnalysis;

pub struct ResponseParser;

impl ResponseParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_response(&self, body: &[u8]) -> ResponseAnalysis {
        let json = match serde_json::from_slice::<Value>(body) {
            Ok(value) => value,
            Err(_) => {
                return ResponseAnalysis {
                    summary: String::new(),
                    input_tokens: None,
                    output_tokens: None,
                    stop_reason: None,
                    cost_usd: None,
                }
            }
        };

        let input_tokens = json
            .get("usage")
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(|v| v.as_u64());
        let output_tokens = json
            .get("usage")
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(|v| v.as_u64());

        let stop_reason = json
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        ResponseAnalysis {
            summary: String::new(),
            input_tokens,
            output_tokens,
            stop_reason,
            cost_usd: None,
        }
    }
}
