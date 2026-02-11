use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::backend::BackendState;
use crate::config::ConfigStore;
use crate::metrics::{DebugLogger, ObservabilityHub};
use crate::proxy::connection::ConnectionCounter;
use crate::proxy::pool::PoolConfig;
use crate::proxy::router::{build_router, RouterEngine};
use crate::proxy::shutdown::ShutdownManager;
use crate::proxy::thinking::TransformerRegistry;
use crate::proxy::timeout::TimeoutConfig;

pub struct ProxyServer {
    pub addr: SocketAddr,
    /// The bound listener, kept alive to prevent port race conditions.
    /// Populated by try_bind(), consumed by run().
    listener: Option<TcpListener>,
    router: RouterEngine,
    routing_rules: Vec<Box<dyn crate::proxy::routing::RoutingRule>>,
    shutdown: Arc<ShutdownManager>,
    backend_state: BackendState,
    observability: ObservabilityHub,
    debug_logger: Arc<DebugLogger>,
    transformer_registry: Arc<TransformerRegistry>,
}

impl ProxyServer {
    pub fn new(
        config: ConfigStore,
        debug_logger: Arc<DebugLogger>,
    ) -> Result<Self, crate::backend::BackendError> {
        let cfg = config.get();
        let timeout_config = TimeoutConfig::from(&cfg.defaults);
        let pool_config = PoolConfig::from(&cfg.defaults);
        let backend_state = BackendState::from_config(cfg.clone())?;
        let observability = ObservabilityHub::new(1000)
            .with_plugins(vec![debug_logger.clone()]);
        let transformer_registry = Arc::new(TransformerRegistry::new());
        let router = RouterEngine::new(
            timeout_config,
            pool_config,
            backend_state.clone(),
            observability.clone(),
            debug_logger.clone(),
            transformer_registry.clone(),
        );
        let routing_rules = crate::proxy::routing::build_rules(&cfg.agent_teams);
        Ok(Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            listener: None,
            router,
            routing_rules,
            shutdown: Arc::new(ShutdownManager::new()),
            backend_state,
            observability,
            debug_logger,
            transformer_registry,
        })
    }

    /// Try to bind to the configured address, falling back to incremental ports if busy.
    /// Returns the bound address and the base URL for Claude Code.
    ///
    /// The listener is kept alive to prevent port race conditions - another process
    /// cannot claim the port between try_bind() and run().
    pub async fn try_bind(&mut self, config: &ConfigStore) -> Result<(SocketAddr, String), Box<dyn std::error::Error>> {
        let bind_addr_str = config.get().proxy.bind_addr.clone();
        let base_url_template = config.get().proxy.base_url.clone();

        // Parse the configured bind address to get the starting port
        let bind_addr: SocketAddr = bind_addr_str.parse()
            .map_err(|e| format!("Invalid bind address '{}': {}", bind_addr_str, e))?;

        let start_port = bind_addr.port();
        let host = bind_addr.ip();

        // Try ports from start_port up to start_port + 100
        for port in start_port..=start_port.saturating_add(100) {
            let try_addr = SocketAddr::new(host, port);
            match TcpListener::bind(try_addr).await {
                Ok(listener) => {
                    let actual_addr = listener.local_addr()?;

                    // Build the base URL with the actual port
                    let actual_base_url = if base_url_template.contains("localhost") ||
                                           base_url_template.contains("127.0.0.1") {
                        format!("http://127.0.0.1:{}", actual_addr.port())
                    } else {
                        base_url_template
                    };

                    self.addr = actual_addr;
                    // Keep listener alive to prevent race conditions
                    self.listener = Some(listener);
                    crate::metrics::app_log("proxy", &format!("Proxy bound to {} (base_url: {})", actual_addr, actual_base_url));
                    return Ok((actual_addr, actual_base_url));
                }
                Err(e) => {
                    crate::metrics::app_log("proxy", &format!("Port {} busy: {}", port, e));
                    continue;
                }
            }
        }

        Err(format!("Could not find available port in range {}-{}", start_port, start_port + 100).into())
    }

    pub fn backend_state(&self) -> BackendState {
        self.backend_state.clone()
    }

    pub fn observability(&self) -> ObservabilityHub {
        self.observability.clone()
    }

    pub fn debug_logger(&self) -> Arc<DebugLogger> {
        self.debug_logger.clone()
    }

    pub fn shutdown_handle(&self) -> Arc<ShutdownManager> {
        self.shutdown.clone()
    }

    pub fn transformer_registry(&self) -> Arc<TransformerRegistry> {
        self.transformer_registry.clone()
    }

    pub fn handle(&self) -> ProxyHandle {
        ProxyHandle {
            shutdown: self.shutdown.clone(),
        }
    }

    /// Run the proxy server.
    ///
    /// Consumes self to take ownership of the pre-bound listener.
    /// Call try_bind() before run() to bind to an available port.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = self.listener
            .ok_or("try_bind() must be called before run()")?;

        crate::metrics::app_log("proxy", &format!("Starting proxy server on {}", self.addr));

        let app = build_router(self.router.clone(), self.routing_rules);
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
        crate::metrics::app_log("proxy", "Shutting down gracefully");

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
