use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::backend::BackendState;
use crate::metrics::{DebugLogger, MetricsSnapshot, ObservabilityHub};
use crate::proxy::shutdown::ShutdownManager;
use crate::proxy::thinking::TransformerRegistry;

use super::types::{BackendInfo, IpcCommand, ProxyStatus};

pub struct IpcServer {
    pub(crate) receiver: mpsc::Receiver<IpcCommand>,
}

impl IpcServer {
    pub fn new(receiver: mpsc::Receiver<IpcCommand>) -> Self {
        Self { receiver }
    }

    pub async fn run(
        mut self,
        backend_state: BackendState,
        observability: ObservabilityHub,
        debug_logger: Arc<DebugLogger>,
        shutdown: Arc<ShutdownManager>,
        started_at: Instant,
        transformer_registry: Arc<TransformerRegistry>,
    ) {
        while let Some(command) = self.receiver.recv().await {
            match command {
                IpcCommand::SwitchBackend {
                    backend_id,
                    respond_to,
                } => {
                    let old_backend = backend_state.get_active_backend();
                    let result = backend_state
                        .switch_backend(&backend_id)
                        .map(|_| backend_state.get_active_backend());
                    if result.is_ok() {
                        if let Err(e) = transformer_registry
                            .on_backend_switch(&old_backend, &backend_id)
                            .await
                        {
                            tracing::warn!(
                                error = %e,
                                "on_backend_switch failed during SwitchBackend"
                            );
                        }
                    }
                    if respond_to.send(result).is_err() {
                        tracing::trace!("IPC: SwitchBackend response dropped (receiver gone)");
                    }
                }
                IpcCommand::SummarizeAndSwitchBackend {
                    from_backend,
                    to_backend,
                    respond_to,
                } => {
                    // 1. Call transformer to summarize
                    let summarize_result = transformer_registry
                        .on_backend_switch(&from_backend, &to_backend)
                        .await;

                    let result = match summarize_result {
                        Ok(()) => {
                            // 2. Switch backend
                            if let Err(e) = backend_state.switch_backend(&to_backend) {
                                Err(crate::proxy::thinking::TransformError::Config(
                                    e.to_string(),
                                ))
                            } else {
                                // Return a preview of success
                                Ok("Session summarized".to_string())
                            }
                        }
                        Err(e) => Err(e),
                    };

                    if respond_to.send(result).is_err() {
                        tracing::trace!(
                            "IPC: SummarizeAndSwitchBackend response dropped (receiver gone)"
                        );
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
                        thinking_mode: transformer_registry.current_mode().to_string(),
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
                            base_url: backend.base_url.clone(),
                        });
                    }
                    if respond_to.send(backends).is_err() {
                        tracing::trace!("IPC: ListBackends response dropped (receiver gone)");
                    }
                }
                IpcCommand::GetDebugLogging { respond_to } => {
                    let config = debug_logger.config();
                    if respond_to.send(config).is_err() {
                        tracing::trace!("IPC: GetDebugLogging response dropped (receiver gone)");
                    }
                }
                IpcCommand::SetDebugLogging { config, respond_to } => {
                    debug_logger.set_config(config);
                    if respond_to.send(Ok(())).is_err() {
                        tracing::trace!("IPC: SetDebugLogging response dropped (receiver gone)");
                    }
                }
            }
        }
    }
}

fn filter_metrics(snapshot: MetricsSnapshot, backend_id: Option<&str>) -> MetricsSnapshot {
    let Some(backend_id) = backend_id else {
        return snapshot;
    };

    let mut filtered = MetricsSnapshot {
        generated_at: snapshot.generated_at,
        per_backend: HashMap::new(),
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
