//! Flag registry — single source of truth for all flags.

/// How AnyClaude handles a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagBehavior {
    /// AnyClaude's own flag — consumed, never forwarded to claude.
    WrapperOwned,
    /// Intercepted from passthrough — consumed by wrapper, not forwarded.
    /// May be re-injected in modified form (e.g., --continue → --session-id).
    Intercepted,
    /// Known claude flag — forwarded as-is with optional validation.
    Passthrough,
}

/// Whether a flag takes a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagArity {
    /// Boolean flag, no value (e.g., --continue, --verbose).
    NoValue,
    /// Requires exactly one value (e.g., --session-id <ID>).
    RequiresValue,
    /// Optional value — present or absent (e.g., --model [NAME]).
    OptionalValue,
}

/// A single flag definition.
#[derive(Debug, Clone)]
pub struct FlagDef {
    /// Primary long form (e.g., "--session-id").
    pub long: &'static str,
    /// Optional short form (e.g., "-s").
    pub short: Option<&'static str>,
    /// Does it take a value?
    pub arity: FlagArity,
    /// How the wrapper handles it.
    pub behavior: FlagBehavior,
    /// Human-readable description (for help text and warnings).
    pub description: &'static str,
}

/// Build the complete flag registry.
pub fn flag_registry() -> Vec<FlagDef> {
    vec![
        // === Wrapper-owned flags (handled by clap in main.rs) ===
        FlagDef {
            long: "--backend",
            short: Some("-b"),
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::WrapperOwned,
            description: "Override default backend",
        },
        // === Intercepted flags (session management) ===
        FlagDef {
            long: "--session-id",
            short: None,
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Intercepted,
            description: "Use specific session ID",
        },
        FlagDef {
            long: "--resume",
            short: Some("-r"),
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Intercepted,
            description: "Resume a session by ID",
        },
        FlagDef {
            long: "--continue",
            short: Some("-c"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Intercepted,
            description: "Continue last session in current directory",
        },
        // === Known passthrough flags (forwarded to claude) ===
        FlagDef {
            long: "--model",
            short: Some("-m"),
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Passthrough,
            description: "Model override",
        },
        FlagDef {
            long: "--verbose",
            short: Some("-v"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Passthrough,
            description: "Verbose output",
        },
        FlagDef {
            long: "--print",
            short: Some("-p"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Passthrough,
            description: "Print mode (non-interactive)",
        },
        FlagDef {
            long: "--output-format",
            short: None,
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Passthrough,
            description: "Output format for print mode",
        },
        FlagDef {
            long: "--dangerously-skip-permissions",
            short: None,
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Passthrough,
            description: "Skip permission prompts",
        },
        FlagDef {
            long: "--teammate-mode",
            short: None,
            arity: FlagArity::RequiresValue,
            behavior: FlagBehavior::Passthrough,
            description: "Teammate mode for agent teams",
        },
        FlagDef {
            long: "--help",
            short: Some("-h"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Passthrough,
            description: "Show help",
        },
        FlagDef {
            long: "--version",
            short: Some("-V"),
            arity: FlagArity::NoValue,
            behavior: FlagBehavior::Passthrough,
            description: "Show version",
        },
    ]
}

impl FlagDef {
    /// Check if this definition matches the given argument string.
    pub fn matches(&self, arg: &str) -> bool {
        arg == self.long || (self.short == Some(arg))
    }
}
