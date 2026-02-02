//! Summarizer client for calling LLM APIs to summarize session history.
//!
//! This client is designed for Anthropic-compatible APIs (like Z.ai).
//! It's isolated but structured for future unification with the main backend code.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

use crate::config::SummarizeConfig;
use crate::metrics::DebugLogger;

use super::error::SummarizeError;

/// Hardcoded prompt for summarization.
/// This is intentionally not configurable for MVP.
const SUMMARIZE_PROMPT: &str = r#"Summarize this coding session for handoff to another AI assistant.

Focus on:
- Current task and goal
- Files modified or created
- Key decisions made and their rationale
- Any blockers or issues encountered
- Suggested next steps

Be concise but include all important context. Output only the summary, no preamble."#;

/// Client for calling the summarization API.
///
/// Uses Anthropic-compatible Messages API format.
pub struct SummarizerClient {
    client: Client,
    config: SummarizeConfig,
    api_key: String,
    debug_logger: Option<Arc<DebugLogger>>,
}

impl SummarizerClient {
    /// Create a new SummarizerClient from config.
    ///
    /// Returns `None` if no API key is available (neither in config nor env var).
    pub fn new(config: SummarizeConfig, debug_logger: Option<Arc<DebugLogger>>) -> Option<Self> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("SUMMARIZER_API_KEY").ok())?;

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .ok()?;

        Some(Self {
            client,
            config,
            api_key,
            debug_logger,
        })
    }

    /// Check if the client is properly configured.
    pub fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Get the model name.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Summarize the given messages.
    ///
    /// Takes the conversation history and returns a summary string.
    pub async fn summarize(&self, messages: &[Value]) -> Result<String, SummarizeError> {
        if self.api_key.is_empty() {
            return Err(SummarizeError::NotConfigured);
        }

        // Build the request body in Anthropic Messages API format
        let request_body = self.build_request(messages);
        let request_json = serde_json::to_string_pretty(&request_body)
            .unwrap_or_else(|_| "<serialization error>".to_string());

        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));

        tracing::debug!(
            url = %url,
            model = %self.config.model,
            message_count = messages.len(),
            "Sending summarization request"
        );

        let start = Instant::now();
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("anthropic-version", "2023-06-01")
            .json(&request_body)
            .send()
            .await?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = response.status();

        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());

            tracing::error!(
                status = %status,
                error = %error_text,
                "Summarization API error"
            );

            // Log error with request body for debugging
            if let Some(logger) = &self.debug_logger {
                logger.log_auxiliary_full(
                    "summarize",
                    Some(status.as_u16()),
                    Some(latency_ms),
                    None,
                    Some(&error_text),
                    Some(&request_json),
                    None,
                );
            }

            return Err(SummarizeError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        let response_text = response
            .text()
            .await
            .unwrap_or_else(|_| "<read error>".to_string());

        let response_body: ApiResponse = serde_json::from_str(&response_text).map_err(|e| {
            SummarizeError::ParseError(format!("Failed to parse response JSON: {}", e))
        })?;

        let result = self.extract_text_content(response_body)?;

        // Log success with request/response for debugging
        if let Some(logger) = &self.debug_logger {
            logger.log_auxiliary_full(
                "summarize",
                Some(status.as_u16()),
                Some(latency_ms),
                Some(&format!("summary_len={}", result.len())),
                None,
                Some(&request_json),
                Some(&response_text),
            );
        }

        Ok(result)
    }

    /// Build the Anthropic Messages API request body.
    fn build_request(&self, messages: &[Value]) -> ApiRequest {
        // Convert the session messages to a single user message with context
        let context = self.format_messages_for_summary(messages);

        ApiRequest {
            model: self.config.model.clone(),
            max_tokens: self.config.max_tokens,
            messages: vec![Message {
                role: "user".to_string(),
                content: format!("{}\n\n---\n\nSession history:\n{}", SUMMARIZE_PROMPT, context),
            }],
        }
    }

    /// Format conversation messages as text for the summarization prompt.
    ///
    /// Formats as chronological history with opening and closing tags:
    /// ```text
    /// [USER]
    /// message content
    /// [/USER]
    ///
    /// [ASSISTANT]
    /// response content
    /// [/ASSISTANT]
    /// ```
    fn format_messages_for_summary(&self, messages: &[Value]) -> String {
        let mut result = String::new();

        for msg in messages {
            let role = msg
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");

            let content = self.extract_message_content(msg);

            if !content.is_empty() {
                let tag = role.to_uppercase();
                result.push_str(&format!("[{}]\n{}\n[/{}]\n\n", tag, content, tag));
            }
        }

        result
    }

    /// Extract text content from a message, handling both string and array formats.
    /// Filters out `<system-reminder>` tags which are internal metadata.
    fn extract_message_content(&self, msg: &Value) -> String {
        let content = msg.get("content");

        let raw = match content {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(arr)) => {
                arr.iter()
                    .filter_map(|item| {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                            item.get("text").and_then(|t| t.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => String::new(),
        };

        // Filter out <system-reminder> tags - they're internal metadata, not conversation
        strip_system_reminders(&raw)
    }

    /// Extract text content from the API response.
    fn extract_text_content(&self, response: ApiResponse) -> Result<String, SummarizeError> {
        for content in response.content {
            if content.content_type == "text" {
                if let Some(text) = content.text {
                    return Ok(text);
                }
            }
        }

        Err(SummarizeError::EmptyResponse)
    }
}

/// Anthropic Messages API request format.
#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

/// Message in Anthropic format.
#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

/// Anthropic Messages API response format.
#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

/// Content block in response.
#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

/// Strip `<system-reminder>...</system-reminder>` tags from text.
///
/// These tags contain internal Claude Code metadata that shouldn't be
/// included in summarization as they add noise without relevant context.
fn strip_system_reminders(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;

    while let Some(start_idx) = remaining.find("<system-reminder>") {
        // Add text before the tag
        result.push_str(&remaining[..start_idx]);

        // Find the closing tag
        if let Some(end_idx) = remaining[start_idx..].find("</system-reminder>") {
            // Skip past the closing tag
            let skip_to = start_idx + end_idx + "</system-reminder>".len();
            remaining = &remaining[skip_to..];
        } else {
            // No closing tag found, keep the rest as-is
            break;
        }
    }

    // Add any remaining text
    result.push_str(remaining);

    // Clean up extra whitespace from removed tags
    result
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_test_config() -> SummarizeConfig {
        SummarizeConfig {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            max_tokens: 500,
        }
    }

    #[test]
    fn client_creation_with_config_key() {
        let config = make_test_config();
        let client = SummarizerClient::new(config, None);
        assert!(client.is_some());
    }

    #[test]
    fn client_creation_without_key_returns_none() {
        let config = SummarizeConfig {
            api_key: None,
            ..make_test_config()
        };
        // This will only succeed if SUMMARIZER_API_KEY is set in env
        // For unit test, we can't guarantee the env var is unset
        // So we just verify the function doesn't panic
        let _ = SummarizerClient::new(config, None);
    }

    #[test]
    fn format_messages_handles_string_content() {
        let config = make_test_config();
        let client = SummarizerClient::new(config, None).unwrap();

        let messages = vec![
            json!({"role": "user", "content": "Hello"}),
            json!({"role": "assistant", "content": "Hi there!"}),
            json!({"role": "user", "content": "How are you?"}),
            json!({"role": "assistant", "content": "I'm doing well!"}),
        ];

        let formatted = client.format_messages_for_summary(&messages);

        // Should have proper opening and closing tags
        assert!(formatted.contains("[USER]\nHello\n[/USER]"));
        assert!(formatted.contains("[ASSISTANT]\nHi there!\n[/ASSISTANT]"));
        assert!(formatted.contains("[USER]\nHow are you?\n[/USER]"));
        assert!(formatted.contains("[ASSISTANT]\nI'm doing well!\n[/ASSISTANT]"));

        // Verify chronological order: first USER, then ASSISTANT, etc.
        let user1_pos = formatted.find("[USER]\nHello").unwrap();
        let asst1_pos = formatted.find("[ASSISTANT]\nHi there!").unwrap();
        let user2_pos = formatted.find("[USER]\nHow are you?").unwrap();
        let asst2_pos = formatted.find("[ASSISTANT]\nI'm doing well!").unwrap();
        assert!(user1_pos < asst1_pos);
        assert!(asst1_pos < user2_pos);
        assert!(user2_pos < asst2_pos);
    }

    #[test]
    fn format_messages_handles_array_content() {
        let config = make_test_config();
        let client = SummarizerClient::new(config, None).unwrap();

        let messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "Let me think..."},
                {"type": "text", "text": "Here's my answer"}
            ]
        })];

        let formatted = client.format_messages_for_summary(&messages);

        assert!(formatted.contains("Here's my answer"));
        // Thinking blocks should be filtered out
        assert!(!formatted.contains("Let me think"));
    }

    #[test]
    fn build_request_includes_prompt() {
        let config = make_test_config();
        let client = SummarizerClient::new(config, None).unwrap();

        let messages = vec![json!({"role": "user", "content": "Test"})];
        let request = client.build_request(&messages);

        assert_eq!(request.model, "test-model");
        assert_eq!(request.max_tokens, 500);
        assert_eq!(request.messages.len(), 1);
        assert!(request.messages[0].content.contains("Summarize"));
    }

    #[test]
    fn extract_text_content_works() {
        let config = make_test_config();
        let client = SummarizerClient::new(config, None).unwrap();

        let response = ApiResponse {
            content: vec![ContentBlock {
                content_type: "text".to_string(),
                text: Some("Summary here".to_string()),
            }],
        };

        let result = client.extract_text_content(response);
        assert_eq!(result.unwrap(), "Summary here");
    }

    #[test]
    fn extract_text_content_empty_response() {
        let config = make_test_config();
        let client = SummarizerClient::new(config, None).unwrap();

        let response = ApiResponse { content: vec![] };

        let result = client.extract_text_content(response);
        assert!(matches!(result, Err(SummarizeError::EmptyResponse)));
    }

    /// Integration test with real API.
    /// Run with: SUMMARIZER_API_KEY=your-key cargo test test_summarizer_real_api -- --ignored
    #[tokio::test]
    #[ignore = "requires SUMMARIZER_API_KEY"]
    async fn test_summarizer_real_api() {
        let config = SummarizeConfig {
            base_url: "https://api.z.ai/api/anthropic".to_string(),
            api_key: None, // Will use env var
            model: "glm-4.7".to_string(),
            max_tokens: 200,
        };

        let client = SummarizerClient::new(config, None).expect("SUMMARIZER_API_KEY must be set");

        let messages = vec![
            json!({"role": "user", "content": "Help me write a function to calculate fibonacci numbers"}),
            json!({"role": "assistant", "content": "Here's a Python function:\n\n```python\ndef fib(n):\n    if n <= 1:\n        return n\n    return fib(n-1) + fib(n-2)\n```"}),
            json!({"role": "user", "content": "Can you make it iterative?"}),
            json!({"role": "assistant", "content": "Sure:\n\n```python\ndef fib(n):\n    a, b = 0, 1\n    for _ in range(n):\n        a, b = b, a + b\n    return a\n```"}),
        ];

        let result = client.summarize(&messages).await;

        match result {
            Ok(summary) => {
                println!("Summary received:\n{}", summary);
                assert!(!summary.is_empty());
                // Summary should mention fibonacci or the task
                let summary_lower = summary.to_lowercase();
                assert!(
                    summary_lower.contains("fibonacci")
                        || summary_lower.contains("function")
                        || summary_lower.contains("python"),
                    "Summary should mention the task context"
                );
            }
            Err(e) => {
                panic!("Summarization failed: {}", e);
            }
        }
    }

    #[test]
    fn strip_system_reminders_removes_tags() {
        let text = "Hello <system-reminder>internal stuff</system-reminder> world";
        let result = strip_system_reminders(text);
        assert_eq!(result, "Hello  world");
    }

    #[test]
    fn strip_system_reminders_handles_multiple_tags() {
        let text = "<system-reminder>tag1</system-reminder>Content<system-reminder>tag2</system-reminder>More";
        let result = strip_system_reminders(text);
        assert_eq!(result, "ContentMore");
    }

    #[test]
    fn strip_system_reminders_preserves_text_without_tags() {
        let text = "Normal text without any tags";
        let result = strip_system_reminders(text);
        assert_eq!(result, "Normal text without any tags");
    }

    #[test]
    fn strip_system_reminders_handles_multiline() {
        let text = "Line 1\n<system-reminder>\nInternal\nContent\n</system-reminder>\nLine 2";
        let result = strip_system_reminders(text);
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
        assert!(!result.contains("Internal"));
    }
}
