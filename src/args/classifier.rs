//! Argument classifier — raw args → classified args.

use crate::args::registry::{FlagArity, FlagBehavior, FlagDef};

/// A classified argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifiedArg {
    /// Wrapper-owned flag (already consumed by clap, shouldn't appear here).
    WrapperOwned {
        flag: String,
        value: Option<String>,
    },
    /// Intercepted flag with optional value.
    Intercepted {
        flag: String, // normalized to long form
        value: Option<String>,
    },
    /// Known passthrough flag with optional value.
    KnownPassthrough {
        flag: String, // normalized to long form
        value: Option<String>,
    },
    /// Unknown flag — not in registry. Forwarded with optional warning.
    UnknownPassthrough(String),
    /// Positional argument (not a flag).
    Positional(String),
}

/// Result of classifying raw arguments.
#[derive(Debug, Clone)]
pub struct ClassifyResult {
    /// Classified arguments in order.
    pub args: Vec<ClassifiedArg>,
    /// Warnings produced during classification (e.g., unknown flags).
    pub warnings: Vec<String>,
}

/// Classify raw args against the registry.
pub fn classify(raw_args: &[String], registry: &[FlagDef]) -> ClassifyResult {
    let mut args = Vec::new();
    let mut warnings = Vec::new();
    let mut iter = raw_args.iter().peekable();

    while let Some(arg) = iter.next() {
        // Check if this looks like a flag
        if arg.starts_with('-') {
            // Look up in registry
            let def = registry.iter().find(|d| d.matches(arg));

            if let Some(def) = def {
                // Normalize to long form
                let normalized_flag = def.long.to_string();

                // Handle value based on arity
                let value = match def.arity {
                    FlagArity::NoValue => None,
                    FlagArity::RequiresValue => {
                        // Consume next arg if it doesn't look like a flag
                        match iter.peek() {
                            Some(next) if !next.starts_with('-') => {
                                Some(iter.next().unwrap().clone())
                            }
                            Some(_) | None => {
                                warnings.push(format!(
                                    "{}: missing required value",
                                    normalized_flag
                                ));
                                None
                            }
                        }
                    }
                    FlagArity::OptionalValue => {
                        // Consume next arg if it doesn't look like a flag
                        match iter.peek() {
                            Some(next) if !next.starts_with('-') => {
                                Some(iter.next().unwrap().clone())
                            }
                            Some(_) | None => None,
                        }
                    }
                };

                let classified = match def.behavior {
                    FlagBehavior::WrapperOwned => ClassifiedArg::WrapperOwned {
                        flag: normalized_flag,
                        value,
                    },
                    FlagBehavior::Intercepted => ClassifiedArg::Intercepted {
                        flag: normalized_flag,
                        value,
                    },
                    FlagBehavior::Passthrough => ClassifiedArg::KnownPassthrough {
                        flag: normalized_flag,
                        value,
                    },
                };
                args.push(classified);
            } else {
                // Unknown flag — forward with warning
                warnings.push(format!("{}: unknown flag, forwarding to claude", arg));
                args.push(ClassifiedArg::UnknownPassthrough(arg.clone()));

                // Try to consume a value if next arg doesn't look like a flag
                if let Some(next) = iter.peek() {
                    if !next.starts_with('-') {
                        args.push(ClassifiedArg::UnknownPassthrough(iter.next().unwrap().clone()));
                    }
                }
            }
        } else {
            // Positional argument
            args.push(ClassifiedArg::Positional(arg.clone()));
        }
    }

    ClassifyResult { args, warnings }
}
