use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::backend::{BackendError, BackendState};
use crate::metrics::MetricsSnapshot;
use crate::proxy::shutdown::ShutdownManager;

const IPC_BUFFER: usize = 16;
const IPC_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub enum IpcError {
    Disconnected,
    Timeout,
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::Disconnected => write!(f, "IPC channel disconnected"),
            IpcError::Timeout => write!(f, "IPC request timed out"),
        }
    }
}

impl std::error::Error for IpcError {}

#[derive(Debug, Clone)]
pub struct ProxyStatus {
    pub active_backend: String,
    pub uptime_seconds: u64,
    pub total_requests: u64,
    pub healthy: bool,
}

#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub id: String,
    pub display_name: String,
    pub is_active: bool,
    pub is_configured: bool,
}

pub enum IpcCommand {
    SwitchBackend {
        backend_id: String,
        respond_to: oneshot::Sender<Result<String, BackendError>>,
    },
    GetStatus {
        respond_to: oneshot::Sender<ProxyStatus>,
    },
    GetMetrics {
        backend_id: Option<String>,
        respond_to: oneshot::Sender<MetricsSnapshot>,
    },
    ListBackends {
        respond_to: oneshot::Sender<Vec<BackendInfo>>,
    },
}

pub struct IpcLayer;

impl IpcLayer {
    pub fn new() -> (IpcClient, IpcServer) {
        let (sender, receiver) = mpsc::channel(IPC_BUFFER);
        (IpcClient::new(sender), IpcServer::new(receiver))
    }
}

#[derive(Clone)]
pub struct IpcClient {
    sender: mpsc::Sender<IpcCommand>,
}

impl IpcClient {
    pub fn new(sender: mpsc::Sender<IpcCommand>) -> Self {
        Self { sender }
    }

    pub async fn switch_backend(
        &self,
        backend_id: String,
    ) -> Result<Result<String, BackendError>, IpcError> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(IpcCommand::SwitchBackend {
                backend_id,
                respond_to,
            })
            .await
            .map_err(|_| IpcError::Disconnected)?;

        recv_with_timeout(receiver).await
    }

    pub async fn get_status(&self) -> Result<ProxyStatus, IpcError> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(IpcCommand::GetStatus { respond_to })
            .await
            .map_err(|_| IpcError::Disconnected)?;

        recv_with_timeout(receiver).await
    }

    pub async fn get_metrics(
        &self,
        backend_id: Option<String>,
    ) -> Result<MetricsSnapshot, IpcError> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(IpcCommand::GetMetrics {
                backend_id,
                respond_to,
            })
            .await
            .map_err(|_| IpcError::Disconnected)?;

        recv_with_timeout(receiver).await
    }

    pub async fn list_backends(&self) -> Result<Vec<BackendInfo>, IpcError> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(IpcCommand::ListBackends { respond_to })
            .await
            .map_err(|_| IpcError::Disconnected)?;

        recv_with_timeout(receiver).await
    }
}

pub struct IpcServer {
    receiver: mpsc::Receiver<IpcCommand>,
}

impl IpcServer {
    pub fn new(receiver: mpsc::Receiver<IpcCommand>) -> Self {
        Self { receiver }
    }

    pub async fn run(
        mut self,
        backend_state: BackendState,
        observability: crate::metrics::ObservabilityHub,
        shutdown: std::sync::Arc<ShutdownManager>,
        started_at: Instant,
    ) {
        while let Some(command) = self.receiver.recv().await {
            match command {
                IpcCommand::SwitchBackend {
                    backend_id,
                    respond_to,
                } => {
                    let result = backend_state
                        .switch_backend(&backend_id)
                        .map(|_| backend_state.get_active_backend());
                    if respond_to.send(result).is_err() {
                        tracing::trace!("IPC: SwitchBackend response dropped (receiver gone)");
                    }
                }
                IpcCommand::GetStatus { respond_to } => {
                    let snapshot = observability.snapshot();
                    let total_requests = snapshot
                        .per_backend
                        .values()
                        .map(|metrics| metrics.total)
                        .sum();
                    let status = ProxyStatus {
                        active_backend: backend_state.get_active_backend(),
                        uptime_seconds: started_at.elapsed().as_secs(),
                        total_requests,
                        healthy: !shutdown.is_shutting_down(),
                    };
                    if respond_to.send(status).is_err() {
                        tracing::trace!("IPC: GetStatus response dropped (receiver gone)");
                    }
                }
                IpcCommand::GetMetrics {
                    backend_id,
                    respond_to,
                } => {
                    let snapshot = observability.snapshot();
                    let filtered = filter_metrics(snapshot, backend_id.as_deref());
                    if respond_to.send(filtered).is_err() {
                        tracing::trace!("IPC: GetMetrics response dropped (receiver gone)");
                    }
                }
                IpcCommand::ListBackends { respond_to } => {
                    let config = backend_state.get_config();
                    let active_backend = backend_state.get_active_backend();
                    let mut backends = Vec::with_capacity(config.backends.len());
                    for backend in config.backends {
                        backends.push(BackendInfo {
                            id: backend.name.clone(),
                            display_name: backend.display_name.clone(),
                            is_active: backend.name == active_backend,
                            is_configured: backend.is_configured(),
                        });
                    }
                    if respond_to.send(backends).is_err() {
                        tracing::trace!("IPC: ListBackends response dropped (receiver gone)");
                    }
                }
            }
        }
    }
}

async fn recv_with_timeout<T>(receiver: oneshot::Receiver<T>) -> Result<T, IpcError> {
    match tokio::time::timeout(IPC_TIMEOUT, receiver).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(IpcError::Disconnected),
        Err(_) => Err(IpcError::Timeout),
    }
}

fn filter_metrics(snapshot: MetricsSnapshot, backend_id: Option<&str>) -> MetricsSnapshot {
    let Some(backend_id) = backend_id else {
        return snapshot;
    };

    let mut filtered = MetricsSnapshot {
        generated_at: snapshot.generated_at,
        per_backend: std::collections::HashMap::new(),
        recent: Vec::new(),
    };

    if let Some(metrics) = snapshot.per_backend.get(backend_id) {
        filtered
            .per_backend
            .insert(backend_id.to_string(), metrics.clone());
    }

    filtered.recent = snapshot
        .recent
        .into_iter()
        .filter(|record| record.backend == backend_id)
        .collect();

    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Backend, Config, Defaults, ProxyConfig, ThinkingConfig};
    use crate::metrics::ObservabilityHub;
    use std::sync::Arc;

    fn test_config() -> Config {
        Config {
            defaults: Defaults {
                active: "alpha".to_string(),
                timeout_seconds: 30,
                connect_timeout_seconds: 5,
                idle_timeout_seconds: 60,
                pool_idle_timeout_seconds: 90,
                pool_max_idle_per_host: 8,
                max_retries: 3,
                retry_backoff_base_ms: 100,
            },
            proxy: ProxyConfig::default(),
            thinking: ThinkingConfig::default(),
            backends: vec![
                Backend {
                    name: "alpha".to_string(),
                    display_name: "Alpha".to_string(),
                    base_url: "https://alpha.example.com".to_string(),
                    auth_type_str: "none".to_string(),
                    api_key: None,
                    models: vec!["alpha-1".to_string()],
                },
                Backend {
                    name: "beta".to_string(),
                    display_name: "Beta".to_string(),
                    base_url: "https://beta.example.com".to_string(),
                    auth_type_str: "none".to_string(),
                    api_key: None,
                    models: vec!["beta-1".to_string()],
                },
            ],
        }
    }

    #[tokio::test]
    async fn ipc_switch_backend_and_status() {
        let config = test_config();
        let backend_state = BackendState::from_config(config).expect("backend state");
        let observability = ObservabilityHub::new(10);
        let shutdown = Arc::new(ShutdownManager::new());
        let (client, server) = IpcLayer::new();

        let server_task = tokio::spawn(server.run(
            backend_state.clone(),
            observability,
            shutdown,
            Instant::now(),
        ));

        let status = client.get_status().await.expect("status");
        assert_eq!(status.active_backend, "alpha");
        assert_eq!(status.total_requests, 0);
        assert!(status.healthy);

        let switch = client
            .switch_backend("beta".to_string())
            .await
            .expect("switch")
            .expect("switch result");
        assert_eq!(switch, "beta");

        let status = client.get_status().await.expect("status");
        assert_eq!(status.active_backend, "beta");

        let backends = client.list_backends().await.expect("backends");
        assert_eq!(backends.len(), 2);
        assert!(backends.iter().any(|backend| backend.id == "beta" && backend.is_active));
        assert!(backends.iter().all(|backend| backend.is_configured));

        drop(client);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn ipc_disconnect_returns_error() {
        let (client, server) = IpcLayer::new();
        drop(server);
        let result = client.get_status().await;
        assert!(matches!(result, Err(IpcError::Disconnected)));
    }

    #[tokio::test]
    async fn ipc_timeout_returns_error() {
        let (client, mut server) = IpcLayer::new();

        // Spawn a "slow" server that receives but never responds
        let server_task = tokio::spawn(async move {
            if let Some(_command) = server.receiver.recv().await {
                // Intentionally don't respond - simulate hung proxy
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });

        let result = client.get_status().await;
        assert!(matches!(result, Err(IpcError::Timeout)));

        server_task.abort();
    }
}
