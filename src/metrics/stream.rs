use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use futures_core::Stream;
use tokio::time::{Instant, Sleep};

use super::hub::ObservabilityHub;
use super::redaction::redact_body;
use super::span::RequestSpan;
use super::types::ResponseMeta;

/// Callback type for response completion notification.
pub type ResponseCompleteCallback = Box<dyn Fn(&[u8]) + Send + Sync>;

/// Stream wrapper that adds observability and idle timeout to SSE streams.
///
/// If no data is received within `idle_timeout`, the stream returns an error
/// to prevent indefinite hangs during API stalls.
pub struct ObservedStream<S> {
    inner: S,
    span: Option<RequestSpan>,
    hub: ObservabilityHub,
    idle_timeout: Duration,
    deadline: Pin<Box<Sleep>>,
    response_preview: Option<ResponsePreview>,
    /// Optional callback to be called with full response bytes when stream completes.
    on_complete: Option<ResponseCompleteCallback>,
    /// Buffer to accumulate all response bytes for the callback.
    response_buffer: Vec<u8>,
}

pub struct ResponsePreview {
    pub limit: Option<usize>,
    pub content_type: String,
    pub buffer: Vec<u8>,
    pub pretty_print: bool,
}

impl ResponsePreview {
    pub fn new(limit: usize, content_type: String) -> Option<Self> {
        if limit == 0 {
            return None;
        }
        Some(Self {
            limit: Some(limit),
            content_type,
            buffer: Vec::new(),
            pretty_print: false,
        })
    }

    /// Create a ResponsePreview with full body capture (no limit).
    pub fn full(content_type: String, pretty_print: bool) -> Self {
        Self {
            limit: None,
            content_type,
            buffer: Vec::new(),
            pretty_print,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        match self.limit {
            Some(limit) if self.buffer.len() >= limit => (),
            Some(limit) => {
                let remaining = limit - self.buffer.len();
                let slice = if bytes.len() > remaining {
                    &bytes[..remaining]
                } else {
                    bytes
                };
                self.buffer.extend_from_slice(slice);
            }
            None => {
                // No limit - capture everything
                self.buffer.extend_from_slice(bytes);
            }
        }
    }
}

impl<S> ObservedStream<S> {
    pub fn new(
        inner: S,
        span: RequestSpan,
        hub: ObservabilityHub,
        idle_timeout: Duration,
        response_preview: Option<ResponsePreview>,
    ) -> Self {
        Self {
            inner,
            span: Some(span),
            hub,
            idle_timeout,
            deadline: Box::pin(tokio::time::sleep(idle_timeout)),
            response_preview,
            on_complete: None,
            response_buffer: Vec::new(),
        }
    }

    /// Set a callback to be called with full response bytes when stream completes.
    pub fn with_on_complete(mut self, callback: ResponseCompleteCallback) -> Self {
        self.on_complete = Some(callback);
        self
    }

    fn finish(&mut self) {
        // Call the completion callback with accumulated response bytes
        if let Some(callback) = self.on_complete.take() {
            if !self.response_buffer.is_empty() {
                callback(&self.response_buffer);
            }
        }

        if let Some(mut span) = self.span.take() {
            if let Some(preview) = self.response_preview.take() {
                let preview_value = redact_body(
                    &preview.buffer,
                    &preview.content_type,
                    preview.limit,
                    preview.pretty_print,
                );
                let meta = span
                    .record_mut()
                    .response_meta
                    .get_or_insert(ResponseMeta {
                        headers: None,
                        body_preview: None,
                    });
                meta.body_preview = preview_value;
            }
            self.hub.finish_request(span);
        }
    }

    fn reset_deadline(&mut self) {
        self.deadline
            .as_mut()
            .reset(Instant::now() + self.idle_timeout);
    }
}

impl<S> Stream for ObservedStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, StreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check if idle timeout has expired
        if self.deadline.as_mut().poll(cx).is_ready() {
            let duration = self.idle_timeout.as_secs();
            tracing::warn!(
                idle_timeout_secs = duration,
                "SSE stream idle timeout exceeded"
            );
            if let Some(span) = &mut self.span {
                span.mark_timed_out();
            }
            self.finish();
            return Poll::Ready(Some(Err(StreamError::IdleTimeout { duration })));
        }

        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                // Reset deadline on successful data receipt
                self.reset_deadline();
                if let Some(span) = &mut self.span {
                    span.mark_first_byte();
                    span.add_response_bytes(bytes.len());
                }
                if let Some(preview) = &mut self.response_preview {
                    preview.push(&bytes);
                }
                // Accumulate bytes for completion callback
                if self.on_complete.is_some() {
                    self.response_buffer.extend_from_slice(&bytes);
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                self.finish();
                Poll::Ready(Some(Err(StreamError::Upstream(err))))
            }
            Poll::Ready(None) => {
                self.finish();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Errors that can occur during SSE stream processing.
#[derive(Debug)]
pub enum StreamError {
    /// Upstream connection error
    Upstream(reqwest::Error),
    /// No data received within idle timeout
    IdleTimeout { duration: u64 },
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Upstream(e) => write!(f, "upstream error: {}", e),
            StreamError::IdleTimeout { duration } => {
                write!(f, "idle timeout after {}s of inactivity", duration)
            }
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StreamError::Upstream(e) => Some(e),
            StreamError::IdleTimeout { .. } => None,
        }
    }
}

impl<S> Drop for ObservedStream<S> {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_error_display() {
        let err = StreamError::IdleTimeout { duration: 60 };
        assert_eq!(err.to_string(), "idle timeout after 60s of inactivity");
    }
}
