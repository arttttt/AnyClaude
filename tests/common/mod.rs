//! Shared test utilities and mock infrastructure.

#![allow(dead_code)]

pub mod mock_backend;

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

/// Find an available port for testing.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to free port");
    listener.local_addr().unwrap().port()
}

/// Create a temporary config file with specified backends.
pub fn temp_config(backends: &[(&str, &str, &str)]) -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("config.toml");

    let mut content = String::from(
        r#"[defaults]
active = "test"
timeout_seconds = 5
connect_timeout_seconds = 2

[proxy]
bind_addr = "127.0.0.1:0"

"#,
    );

    for (name, url, auth_type) in backends {
        content.push_str(&format!(
            r#"[[backends]]
name = "{}"
display_name = "{}"
base_url = "{}"
auth_type = "{}"
"#,
            name,
            name.to_uppercase(),
            url,
            auth_type
        ));
        if *auth_type == "api_key" {
            content.push_str("api_key = \"test-key\"\n");
        }
        content.push('\n');
    }

    std::fs::write(&config_path, content).expect("Failed to write config");
    (temp_dir, config_path)
}

/// Wait for a server to become available.
pub async fn wait_for_server(addr: SocketAddr, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}
