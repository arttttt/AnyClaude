use super::*;
use crate::backend::BackendState;
use crate::config::{
    Backend, Config, DebugLoggingConfig, Defaults, ProxyConfig, TerminalConfig, ThinkingConfig,
};
use crate::metrics::{DebugLogger, ObservabilityHub};
use crate::proxy::shutdown::ShutdownManager;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        backends: vec![
            Backend {
                name: "alpha".to_string(),
                display_name: "Alpha".to_string(),
                base_url: "https://alpha.example.com".to_string(),
                auth_type_str: "none".to_string(),
                api_key: None,
                pricing: None,
            },
            Backend {
                name: "beta".to_string(),
                display_name: "Beta".to_string(),
                base_url: "https://beta.example.com".to_string(),
                auth_type_str: "none".to_string(),
                api_key: None,
                pricing: None,
            },
        ],
    }
}

#[tokio::test]
async fn ipc_switch_backend_and_status() {
    let config = test_config();
    let backend_state = BackendState::from_config(config).expect("backend state");
    let debug_logger = Arc::new(DebugLogger::new(DebugLoggingConfig::default()));
    let observability = ObservabilityHub::new(10).with_plugins(vec![debug_logger.clone()]);
    let shutdown = Arc::new(ShutdownManager::new());
    let (client, server) = IpcLayer::new();

    let server_task = tokio::spawn(server.run(
        backend_state.clone(),
        observability,
        debug_logger,
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
