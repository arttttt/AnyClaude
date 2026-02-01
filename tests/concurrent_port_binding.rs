//! Tests for concurrent port binding to verify race condition fix.
//!
//! These tests verify that multiple proxy server instances can safely
//! attempt to bind to ports concurrently without race conditions.

mod common;

use claudewrapper::config::{Backend, Config, ConfigStore, Defaults, ProxyConfig, TerminalConfig, ThinkingConfig};
use claudewrapper::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;

fn test_config(backend: Backend, bind_addr: &str) -> Config {
    Config {
        defaults: Defaults {
            active: backend.name.clone(),
            timeout_seconds: 5,
            connect_timeout_seconds: 2,
            idle_timeout_seconds: 30,
            pool_idle_timeout_seconds: 30,
            pool_max_idle_per_host: 2,
            max_retries: 1,
            retry_backoff_base_ms: 10,
        },
        proxy: ProxyConfig {
            bind_addr: bind_addr.to_string(),
            base_url: format!("http://{}", bind_addr),
        },
        thinking: ThinkingConfig::default(),
        terminal: TerminalConfig::default(),
        backends: vec![backend],
    }
}

fn create_backend(name: &str, base_url: &str) -> Backend {
    Backend {
        name: name.to_string(),
        display_name: name.to_uppercase(),
        base_url: base_url.to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
    }
}

/// Test that multiple servers trying to bind to port 0 each get unique ports.
/// This verifies the fix for the TOCTOU race condition - each server should
/// keep its listener alive between try_bind() and run().
#[tokio::test]
async fn test_concurrent_port_zero_binding() {
    let mock = MockBackend::start().await;

    // Enqueue enough responses for all servers
    for i in 0..5 {
        mock.enqueue_response(MockResponse::json(&format!(r#"{{"server": {}}}"#, i))).await;
    }

    let num_servers = 5;
    let barrier = Arc::new(Barrier::new(num_servers));
    let mut handles = vec![];
    let bound_ports = Arc::new(parking_lot::Mutex::new(HashSet::new()));

    for i in 0..num_servers {
        let mock_url = mock.base_url();
        let barrier_clone = barrier.clone();
        let ports_clone = bound_ports.clone();

        let handle = tokio::spawn(async move {
            // Wait for all tasks to be ready
            barrier_clone.wait().await;

            // All servers try to bind at roughly the same time
            let bind_addr = "127.0.0.1:0";
            let config = test_config(create_backend(&format!("test{}", i), &mock_url), bind_addr);
            let config_store = ConfigStore::new(config.clone(), PathBuf::from(&format!("/tmp/test{}.toml", i)));
            let mut server = ProxyServer::new(config_store.clone()).unwrap();

            // Bind to port - this should atomically allocate the port
            let (addr, _) = server.try_bind(&config_store).await.unwrap();
            let port = addr.port();

            // Record the port we got
            {
                let mut ports = ports_clone.lock();
                // Verify we got a unique port (not already taken by another server in this test)
                assert!(!ports.contains(&port), "Port {} was already bound by another server!", port);
                ports.insert(port);
            }

            // Start the server (uses the already-bound listener)
            let handle = server.handle();
            tokio::spawn(async move {
                let _ = server.run().await;
            });

            // Give server time to start
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Make a request to verify the server is working
            let client = Client::new();
            let resp = client
                .post(format!("http://{}/v1/messages", addr))
                .body("{}")
                .send()
                .await
                .unwrap();

            assert_eq!(resp.status(), 200, "Server {} failed to respond", i);

            // Shutdown
            handle.shutdown();

            port
        });

        handles.push(handle);
    }

    // Wait for all servers to complete
    let mut all_ports = HashSet::new();
    for handle in handles {
        let port = handle.await.unwrap();
        all_ports.insert(port);
    }

    // Verify we got exactly num_servers unique ports
    assert_eq!(all_ports.len(), num_servers, "Expected {} unique ports but got {}", num_servers, all_ports.len());
}

/// Test that when multiple servers try to bind starting from the same port,
/// they each get consecutive unique ports due to the fallback logic.
/// This verifies that the port binding is atomic - the first server to bind
/// to a port keeps it, and others fall back to the next available port.
#[tokio::test]
async fn test_concurrent_same_start_port_gets_consecutive_ports() {
    let mock = MockBackend::start().await;

    // Enqueue enough responses for all servers
    for i in 0..3 {
        mock.enqueue_response(MockResponse::json(&format!(r#"{{"server": {}}}"#, i))).await;
    }

    // Use a specific port range - all servers will try to start from the same port
    let start_port = common::free_port();
    let bind_addr = format!("127.0.0.1:{}", start_port);

    let num_servers = 3;
    let barrier = Arc::new(Barrier::new(num_servers));
    let mut handles = vec![];
    let bound_ports = Arc::new(parking_lot::Mutex::new(HashSet::new()));

    for i in 0..num_servers {
        let mock_url = mock.base_url();
        let barrier_clone = barrier.clone();
        let bind_addr_clone = bind_addr.clone();
        let ports_clone = bound_ports.clone();

        let handle = tokio::spawn(async move {
            // Wait for all tasks to be ready
            barrier_clone.wait().await;

            // All servers try to bind starting from the same port
            let config = test_config(create_backend(&format!("test{}", i), &mock_url), &bind_addr_clone);
            let config_store = ConfigStore::new(config.clone(), PathBuf::from(&format!("/tmp/test{}.toml", i)));
            let mut server = ProxyServer::new(config_store.clone()).unwrap();

            // Bind - due to fallback logic, each will get a unique port
            let (addr, _) = server.try_bind(&config_store).await.unwrap();
            let port = addr.port();

            // Record the port
            {
                let mut ports = ports_clone.lock();
                assert!(!ports.contains(&port), "Port {} was already bound!", port);
                ports.insert(port);
            }

            // Start server and verify it works
            tokio::spawn(async move {
                let _ = server.run().await;
            });

            tokio::time::sleep(Duration::from_millis(50)).await;

            let client = Client::new();
            let resp = client
                .get(format!("http://{}/health", addr))
                .send()
                .await
                .unwrap();

            assert_eq!(resp.status(), 200, "Server {} failed health check", i);

            port
        });

        handles.push(handle);
    }

    // Collect all ports
    let mut all_ports: Vec<u16> = vec![];
    for handle in handles {
        let port = handle.await.unwrap();
        all_ports.push(port);
    }

    // Verify all ports are unique
    let unique_ports: HashSet<u16> = all_ports.iter().cloned().collect();
    assert_eq!(unique_ports.len(), num_servers, "Expected {} unique ports but got {}", num_servers, unique_ports.len());

    // Verify ports are in the expected range (start_port to start_port + num_servers - 1)
    // Due to race conditions, we can't guarantee exact port numbers, but they should
    // all be >= start_port and close to each other
    for port in &all_ports {
        assert!(*port >= start_port, "Port {} is below start port {}", port, start_port);
        assert!(*port < start_port + 10, "Port {} is too far from start port {}", port, start_port);
    }
}

/// Test that the port remains bound between try_bind() and run().
/// This verifies that another process cannot steal the port.
#[tokio::test]
async fn test_port_remains_bound_between_try_bind_and_run() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok": true}"#)).await;

    let bind_addr = "127.0.0.1:0";
    let config = test_config(create_backend("test", &mock.base_url()), bind_addr);
    let config_store = ConfigStore::new(config.clone(), PathBuf::from("/tmp/test.toml"));
    let mut server = ProxyServer::new(config_store.clone()).unwrap();

    // Bind to port
    let (addr, _) = server.try_bind(&config_store).await.unwrap();
    let port = addr.port();

    // Simulate a delay before run() - during this time the port should stay bound
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Try to bind another listener to the same port - should fail
    let result = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await;
    assert!(result.is_err(), "Should not be able to bind to the same port while server holds it");

    // Now start the server - it should work because it still holds the listener
    let handle = server.handle();
    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify the server is working
    let client = Client::new();
    let resp = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    handle.shutdown();
}
