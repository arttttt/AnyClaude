use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Bytes;
use futures_core::Stream;

use super::hub::ObservabilityHub;
use super::span::RequestSpan;

pub struct ObservedStream<S> {
    inner: S,
    span: Option<RequestSpan>,
    hub: ObservabilityHub,
}

impl<S> ObservedStream<S> {
    pub fn new(inner: S, span: RequestSpan, hub: ObservabilityHub) -> Self {
        Self {
            inner,
            span: Some(span),
            hub,
        }
    }

    fn finish(&mut self) {
        if let Some(span) = self.span.take() {
            self.hub.finish_request(span);
        }
    }
}

impl<S> Stream for ObservedStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Bytes, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                if let Some(span) = &mut self.span {
                    span.mark_first_byte();
                    span.add_response_bytes(bytes.len());
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                self.finish();
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                self.finish();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for ObservedStream<S> {
    fn drop(&mut self) {
        self.finish();
    }
}
