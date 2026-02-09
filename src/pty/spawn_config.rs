/// Configuration for spawning the Claude Code PTY process.
///
/// Owns the session-lifetime constants (command, base args, proxy URL)
/// and a session ID used for `--session-id` / `--resume`.
///
/// Session ID is determined at construction time:
/// - If user passed `--session-id <id>` or `--resume <id>` → use their ID.
/// - If user passed `--continue` → resolve last session ID from Claude config.
/// - Otherwise → generate a new UUID v4.
pub struct PtySpawnConfig {
    command: String,
    /// Base args with session flags (`--continue`, `--resume`, `--session-id`) stripped.
    base_args: Vec<String>,
    base_url: String,
    /// Session ID: user-provided or auto-generated UUID v4.
    session_id: String,
    /// Warnings produced during argument parsing (e.g. `--continue` without previous session).
    warnings: Vec<String>,
}

/// Ready-to-use args and env for `PtySession::spawn()`.
pub struct SpawnParams {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// How to handle session continuation when building spawn parameters.
pub enum SessionMode {
    /// Initial spawn — use base args + `--session-id <id>`.
    Initial,
    /// Restart — resume our session via `--resume <id>`.
    Resume,
}

impl PtySpawnConfig {
    pub fn new(command: String, base_args: Vec<String>, base_url: String) -> Self {
        let mut warnings = Vec::new();
        let (cleaned_args, session_id) = extract_session(&base_args, &mut warnings);
        Self {
            command,
            base_args: cleaned_args,
            base_url,
            session_id,
            warnings,
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Warnings produced during argument parsing.
    ///
    /// For example, `--continue` when no previous session exists for the cwd.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Build spawn parameters.
    ///
    /// - `extra_env`: additional env vars (e.g. from settings)
    /// - `extra_args`: additional CLI flags (e.g. from settings)
    /// - `session`: how to handle session continuation
    pub fn build(
        &self,
        extra_env: Vec<(String, String)>,
        extra_args: Vec<String>,
        session: SessionMode,
    ) -> SpawnParams {
        let mut args = self.base_args.clone();
        match session {
            SessionMode::Initial => {
                args.push("--session-id".to_string());
                args.push(self.session_id.clone());
            }
            SessionMode::Resume => {
                args.push("--resume".to_string());
                args.push(self.session_id.clone());
            }
        }
        args.extend(extra_args);

        let mut env = vec![("ANTHROPIC_BASE_URL".to_string(), self.base_url.clone())];
        env.extend(extra_env);

        SpawnParams { args, env }
    }
}

/// Extract session flags from args and determine the session ID.
///
/// Returns (cleaned_args, session_id). Session flags are removed from args.
fn extract_session(args: &[String], warnings: &mut Vec<String>) -> (Vec<String>, String) {
    let session_flags = &["--continue", "--resume", "--session-id"];
    let mut cleaned = Vec::new();
    let mut user_session_id: Option<String> = None;
    let mut saw_continue = false;

    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if session_flags.contains(&arg.as_str()) {
            match arg.as_str() {
                "--session-id" | "--resume" => {
                    // Consume the value if present.
                    if let Some(next) = iter.peek() {
                        if !next.starts_with("--") {
                            if user_session_id.is_some() {
                                warnings.push(format!(
                                    "{arg}: overrides previously specified session flag"
                                ));
                            }
                            user_session_id = Some(iter.next().unwrap().clone());
                        } else {
                            warnings.push(format!(
                                "{arg}: missing value, starting new session"
                            ));
                        }
                    } else {
                        warnings.push(format!(
                            "{arg}: missing value, starting new session"
                        ));
                    }
                }
                "--continue" => {
                    saw_continue = true;
                }
                _ => {}
            }
            continue;
        }
        cleaned.push(arg.clone());
    }

    if user_session_id.is_some() && saw_continue {
        warnings.push(
            "--continue ignored because --session-id/--resume was also specified".to_string(),
        );
    }

    let session_id = if let Some(id) = user_session_id {
        id
    } else if saw_continue {
        match read_last_session_id() {
            Some(id) => id,
            None => {
                warnings.push(
                    "--continue: no previous session found for current directory, starting new session"
                        .to_string(),
                );
                uuid::Uuid::new_v4().to_string()
            }
        }
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    (cleaned, session_id)
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
fn encode_project_path(path: &str) -> String {
    path.replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_replaces_slashes() {
        assert_eq!(
            encode_project_path("/Users/artem/Projects/Foo"),
            "-Users-artem-Projects-Foo"
        );
    }

    #[test]
    fn encode_empty_path() {
        assert_eq!(encode_project_path(""), "");
    }
}
