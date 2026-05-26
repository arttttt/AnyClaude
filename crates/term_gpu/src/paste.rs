//! Paste-flow utilities: bracketed-paste wrap, POSIX shell quoting.
//!
//! `encode_paste` prepares clipboard text for write to a PTY:
//! normalises line endings (the macOS pasteboard often carries CRLF
//! from Windows-origin content) and wraps in the bracketed-paste
//! markers `\e[200~` / `\e[201~` when the emulator has that mode on.
//!
//! `shell_quote_path` single-quotes a file path so a shell tokenises
//! it as one argument. Used when pasting image-from-clipboard paths
//! into a PTY — see `term_clipboard::save_image_to_temp` for the
//! image side of the bridge.

/// Prepare clipboard text for write to a PTY. CRLF / lone-CR are
/// folded to plain `\n`; bracketed-paste markers wrap the payload
/// when the emulator has that mode on.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized: String = text.replace("\r\n", "\n").replace('\r', "\n");
    if bracketed {
        let mut out = Vec::with_capacity(normalized.len() + 8);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(normalized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}

/// Single-quote-escape a file path for safe shell tokenization.
/// `'` inside the path is escaped as `'\''` (POSIX-compatible —
/// bash, zsh, sh, dash all accept it). Empty paths become empty
/// strings (caller should filter those out).
pub fn shell_quote_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let escaped = path.replace('\'', "'\\''");
    format!("'{escaped}'")
}
