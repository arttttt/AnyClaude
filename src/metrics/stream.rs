use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use futures_core::Stream;
use tokio::time::{Instant, Sleep};

use super::hub::ObservabilityHub;
use super::span::RequestSpan;

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
}

impl<S> ObservedStream<S> {
    pub fn new(inner: S, span: RequestSpan, hub: ObservabilityHub, idle_timeout: Duration) -> Self {
        Self {
            inner,
            span: Some(span),
            hub,
            idle_timeout,
            deadline: Box::pin(tokio::time::sleep(idle_timeout)),
        }
    }

    fn finish(&mut self) {
        if let Some(span) = self.span.take() {
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
