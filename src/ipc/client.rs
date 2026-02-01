use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::backend::BackendError;
use crate::metrics::MetricsSnapshot;

use super::types::{BackendInfo, IpcCommand, IpcError, ProxyStatus};

const IPC_TIMEOUT: Duration = Duration::from_secs(1);

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

async fn recv_with_timeout<T>(receiver: oneshot::Receiver<T>) -> Result<T, IpcError> {
    match tokio::time::timeout(IPC_TIMEOUT, receiver).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(IpcError::Disconnected),
        Err(_) => Err(IpcError::Timeout),
    }
}
