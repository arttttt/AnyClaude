use std::cmp::min;
use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde_json::json;

use crate::config::{
    DebugLogDestination, DebugLogFormat, DebugLogLevel, DebugLogRotationMode, DebugLoggingConfig,
};
use crate::metrics::types::{RequestMeta, ResponseMeta};
use crate::metrics::{
    ObservabilityPlugin, PostResponseContext, RequestAnalysis, RequestRecord, ResponseAnalysis,
    RoutingDecision,
};

const LOG_CHANNEL_SIZE: usize = 512;

/// Log event types for the debug logger channel.
#[derive(Debug, Clone)]
pub enum LogEvent {
    /// Standard proxy request/response event.
    Request(DebugLogEvent),
    /// Auxiliary event (summarizer, internal operations).
    Auxiliary(AuxiliaryLogEvent),
}

#[derive(Debug, Clone)]
pub struct DebugLogEvent {
    pub timestamp: SystemTime,
    pub level: DebugLogLevel,
    pub request_id: String,
    pub backend: String,
    pub status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub request_analysis: Option<RequestAnalysis>,
    pub response_analysis: Option<ResponseAnalysis>,
    pub routing_decision: Option<RoutingDecision>,
    pub request_meta: Option<RequestMeta>,
    pub response_meta: Option<ResponseMeta>,
}

/// Auxiliary log event for internal operations (summarizer, etc).
#[derive(Debug, Clone)]
pub struct AuxiliaryLogEvent {
    pub timestamp: SystemTime,
    pub operation: String,
    pub status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub message: Option<String>,
    pub error: Option<String>,
}

pub struct DebugLogger {
    level: AtomicU8,
    config: Arc<RwLock<DebugLoggingConfig>>,
    sender: SyncSender<LogEvent>,
}

impl DebugLogger {
    pub fn new(config: DebugLoggingConfig) -> Self {
        let level = AtomicU8::new(level_to_u8(config.level));
        let config = Arc::new(RwLock::new(config));
        let (sender, receiver) = sync_channel(LOG_CHANNEL_SIZE);
        let config_clone = config.clone();
        std::thread::Builder::new()
            .name("debug-logger".to_string())
            .spawn(move || writer_loop(receiver, config_clone))
            .ok();

        Self {
            level,
            config,
            sender,
        }
    }

    pub fn level(&self) -> DebugLogLevel {
        level_from_u8(self.level.load(Ordering::Relaxed))
    }

    pub fn config(&self) -> DebugLoggingConfig {
        self.config.read().clone()
    }

    pub fn set_config(&self, config: DebugLoggingConfig) {
        self.level
            .store(level_to_u8(config.level), Ordering::Relaxed);
        *self.config.write() = config;
    }

    /// Log an auxiliary event (summarizer, internal operations).
    pub fn log_auxiliary(
        &self,
        operation: &str,
        status: Option<u16>,
        latency_ms: Option<u64>,
        message: Option<&str>,
        error: Option<&str>,
    ) {
        if self.level() == DebugLogLevel::Off {
            return;
        }

        let event = AuxiliaryLogEvent {
            timestamp: SystemTime::now(),
            operation: operation.to_string(),
            status,
            latency_ms,
            message: message.map(|s| s.to_string()),
            error: error.map(|s| s.to_string()),
        };
        let _ = self.sender.try_send(LogEvent::Auxiliary(event));
    }
}

impl ObservabilityPlugin for DebugLogger {
    fn post_response(&self, ctx: &mut PostResponseContext<'_>) {
        let level = self.level();
        if level == DebugLogLevel::Off {
            return;
        }

        let event = DebugLogEvent::from_record(ctx.record, level);
        let _ = self.sender.try_send(LogEvent::Request(event));
    }
}

impl DebugLogEvent {
    pub fn from_record(record: &RequestRecord, level: DebugLogLevel) -> Self {
        let timestamp = record.completed_at.unwrap_or_else(SystemTime::now);
        Self {
            timestamp,
            level,
            request_id: record.id.clone(),
            backend: record.backend.clone(),
            status: record.status,
            latency_ms: record.latency_ms,
            ttfb_ms: record.ttfb_ms,
            request_bytes: record.request_bytes,
            response_bytes: record.response_bytes,
            request_analysis: record.request_analysis.clone(),
            response_analysis: record.response_analysis.clone(),
            routing_decision: record.routing_decision.clone(),
            request_meta: record.request_meta.clone(),
            response_meta: record.response_meta.clone(),
        }
    }
}

fn writer_loop(receiver: Receiver<LogEvent>, config: Arc<RwLock<DebugLoggingConfig>>) {
    let mut stderr = io::stderr();
    let mut file_writer: Option<RotatingFile> = None;
    let mut last_file_path: Option<String> = None;

    while let Ok(log_event) = receiver.recv() {
        let config_snapshot = config.read().clone();
        if config_snapshot.level == DebugLogLevel::Off {
            continue;
        }

        let use_color = stderr.is_terminal() && config_snapshot.format == DebugLogFormat::Console;

        let (line_console, line_file) = match log_event {
            LogEvent::Request(event) => {
                let level = min(event.level, config_snapshot.level);
                match config_snapshot.format {
                    DebugLogFormat::Console => (
                        format_console(&event, level, use_color),
                        format_console(&event, level, false),
                    ),
                    DebugLogFormat::Json => {
                        let line = format_json(&event, level);
                        (line.clone(), line)
                    }
                }
            }
            LogEvent::Auxiliary(event) => match config_snapshot.format {
                DebugLogFormat::Console => (
                    format_auxiliary_console(&event, use_color),
                    format_auxiliary_console(&event, false),
                ),
                DebugLogFormat::Json => {
                    let line = format_auxiliary_json(&event);
                    (line.clone(), line)
                }
            },
        };

        match config_snapshot.destination {
            DebugLogDestination::Stderr => {
                let _ = writeln!(stderr, "{}", line_console);
            }
            DebugLogDestination::File => {
                file_writer =
                    ensure_file_writer(file_writer, &config_snapshot, &mut last_file_path);
                if let Some(writer) = file_writer.as_mut() {
                    let _ = writer.write_line(&line_file);
                }
            }
            DebugLogDestination::Both => {
                let _ = writeln!(stderr, "{}", line_console);
                file_writer =
                    ensure_file_writer(file_writer, &config_snapshot, &mut last_file_path);
                if let Some(writer) = file_writer.as_mut() {
                    let _ = writer.write_line(&line_file);
                }
            }
        }
    }
}

fn ensure_file_writer(
    current: Option<RotatingFile>,
    config: &DebugLoggingConfig,
    last_path: &mut Option<String>,
) -> Option<RotatingFile> {
    if config.destination == DebugLogDestination::Stderr {
        return None;
    }

    let path = config.file_path.clone();
    if last_path.as_ref() == Some(&path) {
        return current;
    }

    *last_path = Some(path.clone());
    Some(RotatingFile::new(path, config.clone()))
}

fn format_console(event: &DebugLogEvent, level: DebugLogLevel, use_color: bool) -> String {
    let timestamp = format_timestamp(event.timestamp);
    let (method, path, query) = match event.request_meta.as_ref() {
        Some(meta) => (meta.method.clone(), meta.path.clone(), meta.query.clone()),
        None => ("-".to_string(), "-".to_string(), None),
    };
    let path_with_query = match query.as_deref() {
        Some(value) if !value.is_empty() => format!("{}?{}", path, value),
        _ => path.clone(),
    };
    let status = event.status.map_or("-".to_string(), |s| s.to_string());
    let status_display = if use_color {
        colorize_status(&status, event.status)
    } else {
        status.clone()
    };
    let latency = event.latency_ms.map_or("-".to_string(), |v| v.to_string());

    let mut line = format!(
        "{} {} {} backend={} status={} latency_ms={}",
        timestamp, method, path_with_query, &event.backend, status_display, latency
    );

    if level >= DebugLogLevel::Verbose {
        let (input_tokens, output_tokens, stop_reason, cost_usd) = tokens_summary(event);
        let model = event
            .request_analysis
            .as_ref()
            .and_then(|analysis| analysis.model.as_ref())
            .map(|v| v.as_str())
            .unwrap_or("-");
        let images = event
            .request_analysis
            .as_ref()
            .map(|analysis| analysis.image_count)
            .unwrap_or(0);
        let routing = event
            .routing_decision
            .as_ref()
            .map(|decision| format!("{}:{}", decision.backend, decision.reason))
            .unwrap_or_else(|| "-".to_string());

        line.push_str(&format!(
            "\nmodel={} input_tokens={} output_tokens={} images={} stop_reason={} routing={} cost_usd={}",
            model,
            input_tokens,
            output_tokens,
            images,
            stop_reason,
            routing,
            cost_usd
        ));
    }

    if level >= DebugLogLevel::Full {
        if let Some(meta) = &event.request_meta {
            if let Some(headers) = &meta.headers {
                line.push_str(&format!("\nrequest_headers={:?}", headers));
            }
            if let Some(body) = &meta.body_preview {
                line.push_str(&format!("\nrequest_body_preview={}", body));
            }
        }
        if let Some(meta) = &event.response_meta {
            if let Some(headers) = &meta.headers {
                line.push_str(&format!("\nresponse_headers={:?}", headers));
            }
            if let Some(body) = &meta.body_preview {
                line.push_str(&format!("\nresponse_body_preview={}", body));
            }
        }
    }

    line
}

fn format_auxiliary_console(event: &AuxiliaryLogEvent, use_color: bool) -> String {
    let timestamp = format_timestamp(event.timestamp);
    let status = event.status.map_or("-".to_string(), |s| s.to_string());
    let status_display = if use_color {
        colorize_status(&status, event.status)
    } else {
        status.clone()
    };
    let latency = event.latency_ms.map_or("-".to_string(), |v| v.to_string());

    let mut line = format!(
        "{} [{}] status={} latency_ms={}",
        timestamp, event.operation, status_display, latency
    );

    if let Some(msg) = &event.message {
        line.push_str(&format!(" message=\"{}\"", msg));
    }

    if let Some(err) = &event.error {
        let err_display = if use_color {
            format!("\x1b[31m{}\x1b[0m", err) // Red
        } else {
            err.clone()
        };
        line.push_str(&format!(" error=\"{}\"", err_display));
    }

    line
}

fn format_auxiliary_json(event: &AuxiliaryLogEvent) -> String {
    let value = json!({
        "ts": format_timestamp(event.timestamp),
        "type": "auxiliary",
        "operation": event.operation,
        "status": event.status,
        "latency_ms": event.latency_ms,
        "message": event.message,
        "error": event.error,
    });
    value.to_string()
}

fn colorize_status(value: &str, status: Option<u16>) -> String {
    let color = match status.unwrap_or(0) {
        200..=299 => "32",
        300..=399 => "33",
        400..=499 => "33",
        500..=599 => "31",
        _ => "0",
    };
    if color == "0" {
        return value.to_string();
    }
    format!("\x1b[{}m{}\x1b[0m", color, value)
}

fn format_json(event: &DebugLogEvent, level: DebugLogLevel) -> String {
    let (method, path, query) = match event.request_meta.as_ref() {
        Some(meta) => (meta.method.clone(), meta.path.clone(), meta.query.clone()),
        None => ("-".to_string(), "-".to_string(), None),
    };
    let (input_tokens, output_tokens, stop_reason, cost_usd) = tokens_summary(event);

    let value = json!({
        "ts": format_timestamp(event.timestamp),
        "level": format!("{:?}", level).to_lowercase(),
        "request_id": event.request_id.clone(),
        "method": method,
        "path": path,
        "query": query,
        "backend": event.backend.clone(),
        "status": event.status,
        "latency_ms": event.latency_ms,
        "ttfb_ms": event.ttfb_ms,
        "request_bytes": event.request_bytes,
        "response_bytes": event.response_bytes,
        "model": event.request_analysis.as_ref().and_then(|analysis| analysis.model.clone()),
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "images": event.request_analysis.as_ref().map(|analysis| analysis.image_count),
        "stop_reason": stop_reason,
        "routing": event.routing_decision.as_ref().map(|decision| json!({
            "backend": decision.backend,
            "reason": decision.reason,
        })),
        "cost_usd": cost_usd,
        "request": event.request_meta.clone(),
        "response": event.response_meta.clone(),
    });

    value.to_string()
}

fn tokens_summary(event: &DebugLogEvent) -> (String, String, String, String) {
    let input_tokens = event
        .response_analysis
        .as_ref()
        .and_then(|analysis| analysis.input_tokens)
        .or_else(|| {
            event
                .request_analysis
                .as_ref()
                .and_then(|analysis| analysis.estimated_input_tokens)
        })
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());

    let output_tokens = event
        .response_analysis
        .as_ref()
        .and_then(|analysis| analysis.output_tokens)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());

    let stop_reason = event
        .response_analysis
        .as_ref()
        .and_then(|analysis| analysis.stop_reason.as_ref())
        .map(|v| v.as_str())
        .unwrap_or("-")
        .to_string();

    let cost = event
        .response_analysis
        .as_ref()
        .and_then(|analysis| analysis.cost_usd)
        .map(|v| format!("{:.6}", v))
        .unwrap_or_else(|| "-".to_string());

    (input_tokens, output_tokens, stop_reason, cost)
}

fn format_timestamp(timestamp: SystemTime) -> String {
    let duration = timestamp.duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{}", duration.as_secs(), duration.subsec_millis())
}

fn level_to_u8(level: DebugLogLevel) -> u8 {
    match level {
        DebugLogLevel::Off => 0,
        DebugLogLevel::Basic => 1,
        DebugLogLevel::Verbose => 2,
        DebugLogLevel::Full => 3,
    }
}

fn level_from_u8(value: u8) -> DebugLogLevel {
    match value {
        1 => DebugLogLevel::Basic,
        2 => DebugLogLevel::Verbose,
        3 => DebugLogLevel::Full,
        _ => DebugLogLevel::Off,
    }
}

struct RotatingFile {
    path: PathBuf,
    rotation_mode: DebugLogRotationMode,
    max_bytes: u64,
    max_files: usize,
    current_size: u64,
    current_day: Option<u64>,
    file: File,
}

impl RotatingFile {
    fn new(path: String, config: DebugLoggingConfig) -> Self {
        let path = expand_tilde(Path::new(&path));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = File::create(&path).unwrap_or_else(|_| File::create("/dev/null").unwrap());
        let current_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Self {
            path,
            rotation_mode: config.rotation.mode,
            max_bytes: config.rotation.max_bytes,
            max_files: config.rotation.max_files,
            current_size,
            current_day: None,
            file,
        }
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.rotate_if_needed(line.len() as u64)?;
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.current_size = self.current_size.saturating_add(line.len() as u64 + 1);
        Ok(())
    }

    fn rotate_if_needed(&mut self, incoming: u64) -> io::Result<()> {
        match self.rotation_mode {
            DebugLogRotationMode::None => return Ok(()),
            DebugLogRotationMode::Size => {
                if self.current_size + incoming <= self.max_bytes {
                    return Ok(());
                }
            }
            DebugLogRotationMode::Daily => {
                let day = current_day();
                if self.current_day.is_none() {
                    self.current_day = Some(day);
                    return Ok(());
                }
                if self.current_day == Some(day) {
                    return Ok(());
                }
                self.current_day = Some(day);
            }
        }

        let rotated = rotated_path(&self.path);
        let _ = std::fs::rename(&self.path, rotated);
        self.file = File::create(&self.path)?;
        self.current_size = 0;
        cleanup_rotated(&self.path, self.max_files);
        Ok(())
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if let Some(rest) = path_str.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

fn rotated_path(base: &Path) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file_name = base
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "debug.log".to_string());
    let rotated_name = format!("{}.{}", file_name, suffix);
    base.with_file_name(rotated_name)
}

fn cleanup_rotated(base: &Path, max_files: usize) {
    let dir = match base.parent() {
        Some(dir) => dir,
        None => return,
    };
    let base_name = match base.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => return,
    };

    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&base_name))
            .collect(),
        Err(_) => return,
    };

    entries.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());

    if entries.len() <= max_files {
        return;
    }

    let prune_count = entries.len().saturating_sub(max_files);
    for entry in entries.into_iter().take(prune_count) {
        let _ = std::fs::remove_file(entry.path());
    }
}

fn current_day() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}
