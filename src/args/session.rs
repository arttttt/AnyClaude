//! Session resolver — determine session ID from classified args.

use crate::args::classifier::ClassifiedArg;

/// Result of session resolution.
#[derive(Debug, Clone)]
pub struct SessionResolution {
    /// Determined session ID.
    pub session_id: String,
    /// How the session was resolved (for logging/debugging).
    pub source: SessionSource,
    /// Warnings (conflicts, missing values, etc.)
    pub warnings: Vec<String>,
}

/// How the session ID was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    /// User passed --session-id <id>.
    ExplicitId,
    /// User passed --resume <id>.
    ResumeId,
    /// User passed --continue, resolved from ~/.claude.json or sessions-index.
    ContinueLast,
    /// No session flags — generated new UUID.
    Generated,
}

/// Resolve session from classified args.
pub fn resolve_session(classified: &[ClassifiedArg]) -> SessionResolution {
    let mut warnings = Vec::new();
    let mut user_session_id: Option<String> = None;
    let mut session_source: Option<SessionSource> = None;
    let mut saw_continue = false;

    // Extract intercepted session flags
    for arg in classified {
        if let ClassifiedArg::Intercepted { flag, value } = arg {
            match flag.as_str() {
                "--session-id" => {
                    if let Some(id) = value {
                        if user_session_id.is_some() {
                            warnings.push("--session-id: overrides previously specified session flag".to_string());
                        }
                        user_session_id = Some(id.clone());
                        session_source = Some(SessionSource::ExplicitId);
                    }
                }
                "--resume" => {
                    if let Some(id) = value {
                        if user_session_id.is_some() {
                            warnings.push("--resume: overrides previously specified session flag".to_string());
                        }
                        user_session_id = Some(id.clone());
                        session_source = Some(SessionSource::ResumeId);
                    }
                }
                "--continue" => {
                    saw_continue = true;
                }
                _ => {}
            }
        }
    }

    // Check for conflicting flags
    if user_session_id.is_some() && saw_continue {
        warnings.push(
            "--continue ignored because --session-id/--resume was also specified".to_string(),
        );
    }

    // Determine final session ID
    let (session_id, source) = if let Some(id) = user_session_id {
        (id, session_source.unwrap_or(SessionSource::ExplicitId))
    } else if saw_continue {
        match read_last_session_id() {
            Some(id) => (id, SessionSource::ContinueLast),
            None => {
                warnings.push(
                    "--continue: no previous session found for current directory, starting new session"
                        .to_string(),
                );
                (uuid::Uuid::new_v4().to_string(), SessionSource::Generated)
            }
        }
    } else {
        (uuid::Uuid::new_v4().to_string(), SessionSource::Generated)
    };

    SessionResolution {
        session_id,
        source,
        warnings,
    }
}

/// Try to find the last session ID for the current working directory.
///
/// Checks two sources in order:
/// 1. `~/.claude.json` → `projects[<cwd>]` → `lastSessionId`
/// 2. `~/.claude/projects/<encoded-cwd>/sessions-index.json` → most recent entry by `modified`
fn read_last_session_id() -> Option<String> {
    let home = dirs::home_dir()?;
    let cwd = std::env::current_dir().ok()?;
    let cwd_str = cwd.to_str()?;

    // Source 1: ~/.claude.json
    if let Some(id) = read_session_from_claude_json(&home, cwd_str) {
        return Some(id);
    }

    // Source 2: ~/.claude/projects/<encoded-cwd>/sessions-index.json
    read_session_from_sessions_index(&home, cwd_str)
}

/// Read `lastSessionId` from `~/.claude.json` for the given project path.
fn read_session_from_claude_json(home: &std::path::Path, cwd: &str) -> Option<String> {
    let path = home.join(".claude.json");
    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("projects")?
        .get(cwd)?
        .get("lastSessionId")?
        .as_str()
        .map(String::from)
}

/// Read the most recent session ID from `sessions-index.json`.
fn read_session_from_sessions_index(home: &std::path::Path, cwd: &str) -> Option<String> {
    let encoded = encode_project_path(cwd);
    let path = home
        .join(".claude")
        .join("projects")
        .join(&encoded)
        .join("sessions-index.json");
    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let entries = json.get("entries")?.as_array()?;

    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("sessionId")?.as_str()?;
            let modified = entry.get("modified")?.as_str()?;
            Some((id.to_string(), modified.to_string()))
        })
        // ISO 8601 timestamps sort correctly via lexicographic comparison
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(id, _)| id)
}

/// Encode a project path for the `.claude/projects/` directory.
///
/// `/Users/artem/Projects/Foo` → `-Users-artem-Projects-Foo`
pub fn encode_project_path(path: &str) -> String {
    path.replace('/', "-")
}
