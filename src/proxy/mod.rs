pub mod error;
pub mod health;
pub mod router;
pub mod shutdown;
pub mod timeout;
pub mod upstream;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use crate::config::ConfigStore;
use crate::proxy::router::RouterEngine;
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
}

impl ProxyServer {
    pub fn new(config: ConfigStore) -> Self {
        let addr = "127.0.0.1:8080".parse().expect("Invalid bind address");
        let timeout_config = TimeoutConfig::from(&config.get().defaults);
        let router = RouterEngine::new(config, timeout_config);
        Self {
            addr,
            router,
            shutdown: Arc::new(ShutdownManager::new()),
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!("Starting proxy server on {}", self.addr);
        let listener = TcpListener::bind(self.addr).await?;
        tracing::info!("Proxy server listening on {}", self.addr);

        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _ = shutdown.wait_for_signal().await;
        });

        loop {
            if self.shutdown.is_shutting_down() {
                break;
            }

            match listener.accept().await {
                Ok((stream, _)) => {
                    let io = TokioIo::new(stream);
                    let router = self.router.clone();
                    let shutdown = self.shutdown.clone();

                    self.shutdown.increment_connections();

                    tokio::task::spawn(async move {
                        let _shutdown_guard = scopeguard::guard(shutdown.clone(), |shutdown| {
                            shutdown.decrement_connections();
                        });

                        let service = service_fn(move |req: Request<Incoming>| {
                            let router = router.clone();
                            async move {
                                router.route(req).await
                            }
                        });

                        let _ = http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    });
                }
                Err(_) if self.shutdown.is_shutting_down() => break,
                Err(err) => {
                    tracing::error!("Error accepting connection: {:?}", err);
                    break;
                }
            }
        }

        drop(listener);
        self.shutdown.wait_for_connections(Duration::from_secs(10)).await;
        tracing::info!("Shutting down gracefully");

        Ok(())
    }
}
