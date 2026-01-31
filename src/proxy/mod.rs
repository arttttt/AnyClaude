pub mod error;
pub mod health;
pub mod pool;
pub mod router;
pub mod shutdown;
pub mod timeout;
pub mod upstream;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::net::TcpListener;
use tower::Service;
use tracing_subscriber::EnvFilter;

use crate::backend::BackendState;
use crate::config::ConfigStore;
use crate::metrics::ObservabilityHub;
use crate::proxy::pool::PoolConfig;
use crate::proxy::router::{build_router, RouterEngine};
use crate::proxy::shutdown::ShutdownManager;
use crate::proxy::timeout::TimeoutConfig;

pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_level(true)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .init();
}

pub struct ProxyServer {
    pub addr: SocketAddr,
    router: RouterEngine,
    shutdown: Arc<ShutdownManager>,
    backend_state: BackendState,
    observability: ObservabilityHub,
}

impl ProxyServer {
    pub fn new(
        config: ConfigStore,
    ) -> Result<Self, crate::backend::BackendError> {
        let addr = config
            .get()
            .proxy
            .bind_addr
            .parse()
            .expect("Invalid bind address");
        let timeout_config = TimeoutConfig::from(&config.get().defaults);
        let pool_config = PoolConfig::from(&config.get().defaults);
        let backend_state = BackendState::from_config(config.get())?;
        let observability = ObservabilityHub::new(1000);
        let router = RouterEngine::new(
            config,
            timeout_config,
            pool_config,
            backend_state.clone(),
            observability.clone(),
        );
        Ok(Self {
            addr,
            router,
            shutdown: Arc::new(ShutdownManager::new()),
            backend_state,
            observability,
        })
    }

    pub fn backend_state(&self) -> BackendState {
        self.backend_state.clone()
    }

    pub fn observability(&self) -> ObservabilityHub {
        self.observability.clone()
    }

    pub fn shutdown_handle(&self) -> Arc<ShutdownManager> {
        self.shutdown.clone()
    }

    pub fn handle(&self) -> ProxyHandle {
        ProxyHandle {
            shutdown: self.shutdown.clone(),
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!("Starting proxy server on {}", self.addr);
        let listener = TcpListener::bind(self.addr).await?;
        tracing::info!("Proxy server listening on {}", self.addr);

        let app = build_router(self.router.clone());
        let make_service = app.into_make_service();
        let make_service = ConnectionCounter::new(make_service, self.shutdown.clone());

        let shutdown = self.shutdown.clone();
        axum::serve(listener, make_service)
            .with_graceful_shutdown(async move {
                let _ = shutdown.wait_for_shutdown().await;
            })
            .into_future()
            .await?;

        self.shutdown.wait_for_connections(Duration::from_secs(10)).await;
        tracing::info!("Shutting down gracefully");

        Ok(())
    }
}

#[derive(Clone)]
pub struct ProxyHandle {
    shutdown: Arc<ShutdownManager>,
}

impl ProxyHandle {
    pub fn shutdown(&self) {
        self.shutdown.signal_shutdown();
    }
}

struct ConnectionCounter<M> {
    inner: M,
    shutdown: Arc<ShutdownManager>,
}

impl<M> ConnectionCounter<M> {
    fn new(inner: M, shutdown: Arc<ShutdownManager>) -> Self {
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

struct ConnectionGuard<S> {
    inner: S,
    shutdown: Arc<ShutdownManager>,
}

impl<S: Clone> Clone for ConnectionGuard<S> {
    fn clone(&self) -> Self {
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
