//! Corner case tests for spawn.env ANTHROPIC_BASE_URL port consistency fix.
//!
//! This test file verifies that:
//! 1. Port 4000 (or any specific port) being available results in correct env
//! 2. Port fallback (when try_bind finds a different port) still produces consistent env
//! 3. TeammateShim uses actual_addr.port() consistently
//! 4. Restart flow maintains port consistency
//! 5. First spawn vs retry vs restart all use correct URLs

mod common;

use anyclaude::args::{build_restart_params, build_spawn_params, EnvSet};
use anyclaude::config::ClaudeSettingsManager;
use anyclaude::shim::TeammateShim;

// =============================================================================
// PORT CONSISTENCY TESTS
// =============================================================================

/// Test that spawn env contains the expected proxy URL when a specific port is provided.
/// This verifies the basic case: port 4000 available -> env should have port 4000.
#[test]
fn spawn_env_contains_provided_proxy_url() {
    let args: Vec<String> = vec!["--model".into(), "opus".into()];
    let proxy_url = "http://127.0.0.1:4000";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None, // no shim
    
        None,
    );

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some("http://127.0.0.1:4000".to_string()),
        "ANTHROPIC_BASE_URL should match the provided proxy URL"
    );
}

/// Test that restart params also contain the correct proxy URL.
#[test]
fn restart_env_contains_provided_proxy_url() {
    let args: Vec<String> = vec!["--resume".into(), "session123".into()];
    let proxy_url = "http://127.0.0.1:5000";

    let params = build_restart_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
        vec![], // no extra env
        vec![], // no extra args
    
        None,
    );

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some("http://127.0.0.1:5000".to_string()),
        "ANTHROPIC_BASE_URL should match the provided proxy URL in restart"
    );
}

/// Test that the proxy URL is preserved even when extra env vars are added.
#[test]
fn restart_merges_extra_env_preserves_proxy_url() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:8080";
    let extra_env = vec![
        ("CUSTOM_VAR".to_string(), "custom_value".to_string()),
        ("ANOTHER_VAR".to_string(), "another_value".to_string()),
    ];

    let params = build_restart_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
        extra_env,
        vec![],
            None,
);

    // Verify ANTHROPIC_BASE_URL is present and correct
    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());
    assert_eq!(
        anthropic_url,
        Some("http://127.0.0.1:8080".to_string()),
        "ANTHROPIC_BASE_URL should be preserved when merging extra env"
    );

    // Verify extra vars are also present
    assert!(
        params.env.iter().any(|(k, v)| k == "CUSTOM_VAR" && v == "custom_value"),
        "CUSTOM_VAR should be present"
    );
    assert!(
        params.env.iter().any(|(k, v)| k == "ANOTHER_VAR" && v == "another_value"),
        "ANOTHER_VAR should be present"
    );

    // Verify ANTHROPIC_BASE_URL appears only once
    let anthropic_count = params.env.iter().filter(|(k, _)| k == "ANTHROPIC_BASE_URL").count();
    assert_eq!(
        anthropic_count, 1,
        "ANTHROPIC_BASE_URL should appear exactly once in env"
    );
}

// =============================================================================
// SHIM + PROXY URL CONSISTENCY TESTS
// =============================================================================

/// Test that shim creation uses the port consistently with spawn env.
/// When shim is present, both the shim script and spawn env should use the same port.
#[test]
fn shim_script_port_matches_spawn_env_port() {
    // Create a shim with a specific port
    let test_port: u16 = 12345;

    let shim = match TeammateShim::create(test_port, "test-token", "test-session", false) {
        Ok(s) => s,
        Err(_) => {
            // Skip if shim creation fails (e.g., no tmux installed)
            return;
        }
    };

    // Read the shim script content
    let shim_dir = shim.path_env().1.split(':').next().unwrap().to_string();
    let script_content = std::fs::read_to_string(format!("{}/tmux", shim_dir)).unwrap();

    // Verify the script contains the correct port
    let expected_url = format!("http://127.0.0.1:{}/teammate", test_port);
    assert!(
        script_content.contains(&expected_url),
        "Shim script should contain URL with port {}: got script that doesn't contain {}",
        test_port, expected_url
    );

    // Now verify that when we build spawn params with this shim, env has consistent port
    let args: Vec<String> = vec![];
    let proxy_url = format!("http://127.0.0.1:{}", test_port);

    let params = build_spawn_params(
        &args,
        &proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        Some(&shim),
            None,
);

    // Verify env contains the proxy URL
    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url),
        "Spawn env ANTHROPIC_BASE_URL should match the proxy URL used for shim"
    );
}

/// Test that different ports result in different shim scripts.
#[test]
fn different_ports_produce_different_shim_scripts() {
    let shim1 = match TeammateShim::create(11111, "test-token", "test-session", false) {
        Ok(s) => s,
        Err(_) => return, // Skip if shim creation fails
    };
    let shim2 = match TeammateShim::create(22222, "test-token", "test-session", false) {
        Ok(s) => s,
        Err(_) => return, // Skip if shim creation fails
    };

    let dir1 = shim1.path_env().1.split(':').next().unwrap().to_string();
    let dir2 = shim2.path_env().1.split(':').next().unwrap().to_string();

    let script1 = std::fs::read_to_string(format!("{}/tmux", dir1)).unwrap();
    let script2 = std::fs::read_to_string(format!("{}/tmux", dir2)).unwrap();

    // Both scripts should contain their respective ports
    assert!(script1.contains("http://127.0.0.1:11111/teammate"));
    assert!(script2.contains("http://127.0.0.1:22222/teammate"));

    // The scripts should be different
    assert_ne!(
        script1, script2,
        "Shim scripts for different ports should be different"
    );
}

// =============================================================================
// PORT FALLBACK SCENARIO TESTS
// =============================================================================

/// Test that when the requested port is not available, the actual bound port
/// is correctly propagated to spawn env.
/// This simulates the scenario where try_bind falls back from 3000 to 4000.
#[test]
fn fallback_port_reflected_in_spawn_env() {
    // Simulate the scenario: config says 3000, but 3000 is busy
    // try_bind returns 4000, so spawn env should have 4000

    let config_proxy_url = "http://127.0.0.1:3000"; // What user configured
    let actual_proxy_url = "http://127.0.0.1:4000"; // What try_bind found

    let args: Vec<String> = vec![];

    // Build spawn params with the ACTUAL URL (what runtime.rs does after try_bind)
    let params = build_spawn_params(
        &args,
        actual_proxy_url, // Use actual, not config
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(actual_proxy_url.to_string()),
        "ANTHROPIC_BASE_URL should use actual bound port (4000), not configured port (3000)"
    );

    assert_ne!(
        anthropic_url,
        Some(config_proxy_url.to_string()),
        "ANTHROPIC_BASE_URL should NOT use the configured port when fallback occurred"
    );
}

/// Test port consistency in restart after fallback.
#[test]
fn restart_maintains_port_consistency_after_fallback() {
    let actual_proxy_url = "http://127.0.0.1:4000";

    let args: Vec<String> = vec!["--resume".into(), "session123".into()];

    let params = build_restart_params(
        &args,
        actual_proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
        vec![],
        vec![],
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(actual_proxy_url.to_string()),
        "Restart should maintain the actual proxy URL after fallback"
    );
}

// =============================================================================
// SESSION TYPE CONSISTENCY TESTS
// =============================================================================

/// Test that both Initial and Resume session modes have consistent proxy URL.
#[test]
fn initial_and_resume_modes_both_have_proxy_url() {
    // Initial mode (no --resume flag)
    let initial_args: Vec<String> = vec!["--model".into(), "opus".into()];
    let proxy_url = "http://127.0.0.1:4000";

    let initial_params = build_spawn_params(
        &initial_args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    // Resume mode (with --resume flag)
    let resume_args: Vec<String> = vec!["--resume".into(), "session123".into()];
    let resume_params = build_spawn_params(
        &resume_args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    // Both should have the proxy URL
    let initial_url = initial_params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());
    let resume_url = resume_params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(initial_url, Some(proxy_url.to_string()));
    assert_eq!(resume_url, Some(proxy_url.to_string()));
}

// =============================================================================
// ENV_SET BUILDER TESTS
// =============================================================================

/// Test EnvSet directly to verify proxy URL is set correctly.
#[test]
fn env_set_with_proxy_url_directly() {
    let env = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .build();

    assert_eq!(env.len(), 1);
    assert_eq!(env[0].0, "ANTHROPIC_BASE_URL");
    assert_eq!(env[0].1, "http://127.0.0.1:4000");
}

/// Test that EnvSet preserves proxy URL when adding shim.
#[test]
fn env_set_proxy_url_with_shim() {
    let shim = match TeammateShim::create(4000, "test-token", "test-session", false) {
        Ok(s) => s,
        Err(_) => return, // Skip if shim creation fails
    };

    let env = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .with_shim(Some(&shim))
        .build();

    // Should have both ANTHROPIC_BASE_URL and PATH
    assert!(env.iter().any(|(k, _)| k == "ANTHROPIC_BASE_URL"));
    assert!(env.iter().any(|(k, _)| k == "PATH"));

    // ANTHROPIC_BASE_URL should be correct
    let url = env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());
    assert_eq!(url, Some("http://127.0.0.1:4000".to_string()));

    // PATH should contain shim directory
    let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone());
    assert!(path.is_some());
    let shim_dir = shim.path_env().1.split(':').next().unwrap().to_string();
    assert!(path.unwrap().contains(&shim_dir));
}

/// Test that proxy URL ordering in env doesn't matter for functionality.
#[test]
fn env_set_order_independent() {
    let env1 = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:4000")
        .with_extra(vec![("VAR1".into(), "val1".into())])
        .build();

    let env2 = EnvSet::new()
        .with_extra(vec![("VAR1".into(), "val1".into())])
        .with_proxy_url("http://127.0.0.1:4000")
        .build();

    // Both should have the same variables
    assert_eq!(env1.len(), env2.len());
    assert!(env1.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "http://127.0.0.1:4000"));
    assert!(env2.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "http://127.0.0.1:4000"));
}

// =============================================================================
// TMUX SHIM SCRIPT CONTENT TESTS
// =============================================================================

/// Test that the tmux shim script correctly references the /teammate route.
#[test]
fn tmux_shim_includes_teammate_route() {
    let shim = match TeammateShim::create(4000, "test-token", "test-session", false) {
        Ok(s) => s,
        Err(_) => return, // Skip if shim creation fails
    };

    let dir = shim.path_env().1.split(':').next().unwrap().to_string();
    let script = std::fs::read_to_string(format!("{}/tmux", dir)).unwrap();

    // Script should contain /teammate route
    assert!(
        script.contains("/teammate"),
        "Shim script should route to /teammate"
    );

    // Script should inject the URL before claude invocation
    assert!(
        script.contains("INJECT_URL"),
        "Shim script should use INJECT_URL variable"
    );
}

/// Test that shim handles the claude path detection correctly.
#[test]
fn tmux_shim_detects_claude_invocation() {
    let shim = match TeammateShim::create(4000, "test-token", "test-session", false) {
        Ok(s) => s,
        Err(_) => return, // Skip if shim creation fails
    };

    let dir = shim.path_env().1.split(':').next().unwrap().to_string();
    let script = std::fs::read_to_string(format!("{}/tmux", dir)).unwrap();

    // Should detect send-keys
    assert!(
        script.contains("send-keys"),
        "Should detect send-keys command"
    );

    // Should detect teammate spawn via --agent-id
    assert!(
        script.contains("--agent-id"),
        "Should detect teammate spawn via --agent-id flag"
    );
}

// =============================================================================
// ERROR SCENARIO TESTS
// =============================================================================

/// Test that build_spawn_params handles empty args correctly.
#[test]
fn spawn_params_with_empty_args() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:4000";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    // Should still have the proxy URL in env
    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "Empty args should still produce correct proxy URL"
    );
}

/// Test that build_restart_params handles empty everything correctly.
#[test]
fn restart_params_with_empty_everything() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:4000";

    let params = build_restart_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
        vec![], // empty extra env
        vec![], // empty extra args
    
        None,
    );

    // Should still have the proxy URL in env
    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "Empty restart params should still produce correct proxy URL"
    );
}

/// Test URL with localhost (not 127.0.0.1) is preserved correctly.
#[test]
fn spawn_env_preserves_localhost_url() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://localhost:4000";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "localhost URL should be preserved as-is"
    );
}

/// Test URL with IPv6 address.
#[test]
fn spawn_env_preserves_ipv6_url() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://[::1]:4000";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "IPv6 URL should be preserved as-is"
    );
}

// =============================================================================
// MULTI-SPAWN CONSISTENCY TESTS
// =============================================================================

/// Test that multiple spawns with the same proxy URL are consistent.
#[test]
fn multiple_spawns_same_proxy_url() {
    let args1: Vec<String> = vec!["--model".into(), "opus".into()];
    let args2: Vec<String> = vec!["--model".into(), "sonnet".into()];
    let proxy_url = "http://127.0.0.1:4000";

    let params1 = build_spawn_params(
        &args1,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);
    let params2 = build_spawn_params(
        &args2,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let url1 = params1
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());
    let url2 = params2
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(url1, url2, "Multiple spawns should have the same proxy URL");
    assert_eq!(url1, Some(proxy_url.to_string()));
}

/// Test spawn followed by restart maintains URL consistency.
#[test]
fn spawn_then_restart_maintains_url() {
    let proxy_url = "http://127.0.0.1:4000";

    // First spawn (initial)
    let spawn_args: Vec<String> = vec!["--model".into(), "opus".into()];
    let spawn_params = build_spawn_params(
        &spawn_args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    // Then restart
    let restart_args: Vec<String> = vec!["--resume".into(), spawn_params.session_id.clone()];
    let restart_params = build_restart_params(
        &restart_args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
        vec![],
        vec![],
            None,
);

    let spawn_url = spawn_params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());
    let restart_url = restart_params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        spawn_url, restart_url,
        "Spawn and restart should have the same proxy URL"
    );
}

// =============================================================================
// URL FORMAT EDGE CASES
// =============================================================================

/// Test URL with trailing slash is preserved.
#[test]
fn spawn_env_preserves_trailing_slash() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:4000/";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "Trailing slash should be preserved"
    );
}

/// Test URL with path is preserved.
#[test]
fn spawn_env_preserves_path() {
    let args: Vec<String> = vec![];
    let proxy_url = "http://127.0.0.1:4000/api/v1";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "Path in URL should be preserved"
    );
}

/// Test HTTPS URL is preserved.
#[test]
fn spawn_env_preserves_https() {
    let args: Vec<String> = vec![];
    let proxy_url = "https://127.0.0.1:4000";

    let params = build_spawn_params(
        &args,
        proxy_url,
        "test-session-token",
        &ClaudeSettingsManager::new(),
        None,
            None,
);

    let anthropic_url = params
        .env
        .iter()
        .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
        .map(|(_, v)| v.clone());

    assert_eq!(
        anthropic_url,
        Some(proxy_url.to_string()),
        "HTTPS should be preserved"
    );
}
