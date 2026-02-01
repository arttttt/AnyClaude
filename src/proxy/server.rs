use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::backend::BackendState;
use crate::config::ConfigStore;
use crate::metrics::ObservabilityHub;
use crate::proxy::connection::ConnectionCounter;
use crate::proxy::pool::PoolConfig;
use crate::proxy::router::{build_router, RouterEngine};
use crate::proxy::shutdown::ShutdownManager;
use crate::proxy::timeout::TimeoutConfig;

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
