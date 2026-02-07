use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use crate::proxy::shutdown::ShutdownManager;

pub struct ConnectionCounter<M> {
    inner: M,
    shutdown: Arc<ShutdownManager>,
}

impl<M> ConnectionCounter<M> {
    pub fn new(inner: M, shutdown: Arc<ShutdownManager>) -> Self {
        Self { inner, shutdown }
    }
}

impl<M: Clone> Clone for ConnectionCounter<M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            shutdown: self.shutdown.clone(),
        }
    }
}

impl<M, T> Service<T> for ConnectionCounter<M>
where
    M: Service<T> + Send,
    M::Future: Send + 'static,
    M::Response: Send + 'static,
{
    type Response = ConnectionGuard<M::Response>;
    type Error = M::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, target: T) -> Self::Future {
        let shutdown = self.shutdown.clone();
        shutdown.increment_connections();
        let fut = self.inner.call(target);

        Box::pin(async move {
            match fut.await {
                Ok(service) => Ok(ConnectionGuard {
                    inner: service,
                    shutdown,
                }),
                Err(err) => {
                    shutdown.decrement_connections();
                    Err(err)
                }
            }
        })
    }
}

pub struct ConnectionGuard<S> {
    inner: S,
    shutdown: Arc<ShutdownManager>,
}

impl<S: Clone> Clone for ConnectionGuard<S> {
    fn clone(&self) -> Self {
        self.shutdown.increment_connections();
        Self {
            inner: self.inner.clone(),
            shutdown: self.shutdown.clone(),
        }
    }
}

impl<S> Drop for ConnectionGuard<S> {
    fn drop(&mut self) {
        self.shutdown.decrement_connections();
    }
}

impl<S, Req> Service<Req> for ConnectionGuard<S>
where
    S: Service<Req>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        self.inner.call(req)
    }
}
