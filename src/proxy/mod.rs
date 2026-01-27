pub mod health;
pub mod router;
pub mod shutdown;
pub mod upstream;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use tokio::net::TcpListener;

use crate::proxy::router::RouterEngine;
use crate::proxy::shutdown::ShutdownManager;

pub struct ProxyServer {
    pub addr: SocketAddr,
    router: RouterEngine,
    shutdown: Arc<ShutdownManager>,
}

impl ProxyServer {
    pub fn new() -> Self {
        let addr = "127.0.0.1:8080".parse().expect("Invalid bind address");
        Self {
            addr,
            router: RouterEngine::new(),
            shutdown: Arc::new(ShutdownManager::new()),
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(self.addr).await?;
        println!("Proxy server listening on {}", self.addr);

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
                                    .map(|resp| {
                                        resp.map(|body| {
                                            Full::new(body)
                                        })
                                    })
                            }
                        });

                        let _ = http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    });
                }
                Err(_) if self.shutdown.is_shutting_down() => break,
                Err(err) => {
                    eprintln!("Error accepting connection: {:?}", err);
                    break;
                }
            }
        }

        drop(listener);
        self.shutdown.wait_for_connections(Duration::from_secs(10)).await;

        Ok(())
    }
}

impl Default for ProxyServer {
    fn default() -> Self {
        Self::new()
    }
}
