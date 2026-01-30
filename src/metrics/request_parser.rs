use crate::metrics::{ObservabilityPlugin, PreRequestContext};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct RequestAnalysis {
    pub model: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub message_count: u32,
    pub has_system_prompt: bool,
    pub has_images: bool,
    pub image_count: u32,
    pub total_image_bytes: u64,
    pub has_tools: bool,
    pub tool_names: Vec<String>,
    pub thinking_enabled: bool,
    pub thinking_budget: Option<u64>,
    pub estimated_input_tokens: Option<u64>,
}

pub struct RequestParser {
    enabled: bool,
}

impl RequestParser {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn parse_request(&self, body: &[u8]) -> RequestAnalysis {
        let json = match serde_json::from_slice::<Value>(body) {
            Ok(v) => v,
            Err(_) => return RequestAnalysis::default(),
        };

        self.parse_model_info(&json)
    }

    fn parse_model_info(&self, json: &Value) -> RequestAnalysis {
        let mut analysis = RequestAnalysis::default();

        if let Some(model) = json.get("model").and_then(|v| v.as_str()) {
            analysis.model = Some(model.to_string());
        }

        if let Some(max_tokens) = json.get("max_tokens").and_then(|v| v.as_u64()) {
            analysis.max_tokens = Some(max_tokens);
        }

        if let Some(temperature) = json.get("temperature").and_then(|v| v.as_f64()) {
            analysis.temperature = Some(temperature);
        }

        if let Some(messages) = json.get("messages").and_then(|v| v.as_array()) {
            analysis.message_count = messages.len() as u32;

            for msg in messages {
                if let Some(role) = msg.get("role").and_then(|v| v.as_str()) {
                    if role == "system" {
                        analysis.has_system_prompt = true;
                    }
                }

                if let Some(content) = msg.get("content") {
                    self.parse_content(content, &mut analysis);
                }
            }
        }

        if let Some(tools) = json.get("tools").and_then(|v| v.as_array()) {
            analysis.has_tools = true;
            for tool in tools {
                if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
                    analysis.tool_names.push(name.to_string());
                }
            }
        }

        if let Some(thinking) = json.get("thinking") {
            if let Some(enabled) = thinking.get("enabled").and_then(|v| v.as_bool()) {
                analysis.thinking_enabled = enabled;
            }

            if let Some(budget) = thinking.get("budget_tokens").and_then(|v| v.as_u64()) {
                analysis.thinking_budget = Some(budget);
            }
        }

        analysis.estimated_input_tokens = self.estimate_tokens(&json);

        analysis
    }

    fn parse_content(&self, content: &Value, analysis: &mut RequestAnalysis) {
        match content {
            Value::String(_) => {}
            Value::Array(items) => {
                for item in items {
                    if let Some(obj) = item.as_object() {
                        if let Some(type_field) = obj.get("type").and_then(|v| v.as_str()) {
                            match type_field {
                                "text" => {}
                                "image" => {
                                    analysis.has_images = true;
                                    analysis.image_count += 1;

                                    if let Some(source) = obj.get("source") {
                                        if let Some(data) =
                                            source.get("data").and_then(|v| v.as_str())
                                        {
                                            let base64_len = data.len();
                                            let estimated_bytes = (base64_len as f64 * 0.75) as u64;
                                            analysis.total_image_bytes += estimated_bytes;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn estimate_tokens(&self, json: &Value) -> Option<u64> {
        let mut total_chars = 0u64;

        if let Some(messages) = json.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                if let Some(content) = msg.get("content") {
                    match content {
                        Value::String(text) => {
                            total_chars += text.chars().count() as u64;
                        }
                        Value::Array(items) => {
                            for item in items {
                                if let Some(obj) = item.as_object() {
                                    if let Some(type_field) =
                                        obj.get("type").and_then(|v| v.as_str())
                                    {
                                        if type_field == "text" {
                                            if let Some(text) =
                                                obj.get("text").and_then(|v| v.as_str())
                                            {
                                                total_chars += text.chars().count() as u64;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if total_chars > 0 {
            Some((total_chars / 4) + 1)
        } else {
            None
        }
    }
}

impl Default for RequestAnalysis {
    fn default() -> Self {
        Self {
            model: None,
            max_tokens: None,
            temperature: None,
            message_count: 0,
            has_system_prompt: false,
            has_images: false,
            image_count: 0,
            total_image_bytes: 0,
            has_tools: false,
            tool_names: Vec::new(),
            thinking_enabled: false,
            thinking_budget: None,
            estimated_input_tokens: None,
        }
    }
}

impl ObservabilityPlugin for RequestParser {
    fn pre_request(
        &self,
        _ctx: &mut PreRequestContext<'_>,
    ) -> Option<crate::metrics::BackendOverride> {
        None
    }
}
