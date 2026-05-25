//! Feed raw ANSI bytes on stdin into the emulator and print the
//! resulting visible grid plus cursor/title/cwd state. Useful for
//! eyeballing how term_core handles a captured Claude Code session.
//!
//! Usage:
//!   echo -e "\x1b[1;31mhello\x1b[m world" | cargo run -p term_core --example dump
//!   cat session.log | cargo run -p term_core --example dump
//!
//! Optional env vars:
//!   TERM_COLS  (default 80)
//!   TERM_ROWS  (default 24)
//!   COLOR=1    re-emit ANSI colour codes so the dump itself is coloured
//!              when piped to a real terminal.

use std::io::Read;

use term_core::{Cell, CellFlags, TermColor, TerminalEmulator, VtEmulator};

fn main() {
    let cols = std::env::var("TERM_COLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80usize);
    let rows = std::env::var("TERM_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24usize);
    let colour = std::env::var("COLOR").is_ok();

    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .expect("failed to read stdin");

    let mut em = VtEmulator::new(cols, rows, 0);
    em.process(&bytes);
    let snap = em.snapshot();

    // Header
    eprintln!(
        "--- term_core dump ({}x{}, parsed {} bytes) ---",
        cols,
        rows,
        bytes.len()
    );
    if !snap.title.is_empty() {
        eprintln!("title : {}", snap.title);
    }
    if let Some(cwd) = &snap.cwd {
        eprintln!("cwd   : {cwd}");
    }
    eprintln!(
        "cursor: row {} col {} visible={} style={:?}",
        snap.cursor.row, snap.cursor.col, snap.cursor.visible, snap.cursor.style
    );
    let responses = em.take_responses();
    if !responses.is_empty() {
        eprintln!("pty-replies ({} bytes): {:?}", responses.len(), responses);
    }
    eprintln!("--- grid ---");

    let top_border = format!("+{}+", "-".repeat(cols));
    println!("{top_border}");
    for row in &snap.rows {
        print!("|");
        for cell in &row.cells {
            print_cell(cell, colour);
        }
        // Reset SGR at end of line so background colours don't bleed.
        if colour {
            print!("\x1b[0m");
        }
        println!("|");
    }
    println!("{top_border}");
}

fn print_cell(cell: &Cell, colour: bool) {
    if colour {
        apply_sgr(cell);
    }
    print!("{}", cell.c);
}

fn apply_sgr(cell: &Cell) {
    let mut parts: Vec<String> = vec!["0".to_string()];
    if cell.flags.bold() {
        parts.push("1".into());
    }
    if cell.flags.faint() {
        parts.push("2".into());
    }
    if cell.flags.italic() {
        parts.push("3".into());
    }
    if cell.flags.underline() {
        parts.push("4".into());
    }
    if cell.flags.inverse() {
        parts.push("7".into());
    }
    if cell.flags.strike() {
        parts.push("9".into());
    }
    parts.extend(colour_to_sgr(cell.fg, true));
    parts.extend(colour_to_sgr(cell.bg, false));
    print!("\x1b[{}m", parts.join(";"));
}

fn colour_to_sgr(c: TermColor, fg: bool) -> Vec<String> {
    let (base, ext) = if fg { (30, 38) } else { (40, 48) };
    match c {
        TermColor::Default => vec![format!("{}", if fg { 39 } else { 49 })],
        TermColor::Indexed(i) if i < 8 => vec![format!("{}", base + i as u32)],
        TermColor::Indexed(i) if i < 16 => vec![format!("{}", base + 60 + (i - 8) as u32)],
        TermColor::Indexed(i) => vec![format!("{ext};5;{i}")],
        TermColor::Rgb(r, g, b) => vec![format!("{ext};2;{r};{g};{b}")],
    }
}

#[allow(dead_code)]
fn flag_summary(f: CellFlags) -> String {
    let mut s = String::new();
    if f.bold() {
        s.push('B');
    }
    if f.italic() {
        s.push('I');
    }
    if f.underline() {
        s.push('U');
    }
    if f.inverse() {
        s.push('R');
    }
    s
}
