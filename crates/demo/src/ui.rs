//! Terminal output: colour, headings, and the two things the demo draws again and again — the
//! chain, and what just landed on each homeserver.

use std::io::Write;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";

/// A step heading.
pub fn heading(n: usize, title: &str) {
    println!();
    println!(
        "{BOLD}{CYAN}━━ Step {n}: {title} {}{RESET}",
        "━".repeat(60usize.saturating_sub(title.len() + 12))
    );
    println!();
}

/// A paragraph of narration, wrapped at 88 columns.
pub fn say(text: &str) {
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.len() + word.len() + 1 > 88 {
            println!("  {line}");
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        println!("  {line}");
    }
}

/// A labelled value.
pub fn kv(label: &str, value: &str) {
    println!("  {DIM}{label:>14}{RESET}  {value}");
}

/// Something went right.
pub fn ok(text: &str) {
    println!("  {GREEN}✔{RESET} {text}");
}

/// Something was refused or is wrong, as designed.
pub fn refused(text: &str) {
    println!("  {RED}✘{RESET} {text}");
}

/// Something to notice.
pub fn note(text: &str) {
    println!("  {YELLOW}!{RESET} {text}");
}

/// The watchdog speaks.
pub fn watchdog(text: &str) {
    println!("  {MAGENTA}👁{RESET} {text}");
}

/// A blank line.
pub fn gap() {
    println!();
}

/// Wait for Enter — or, unattended, for `pause_secs` seconds (a stand-in for reading time).
pub async fn pause(auto: bool, pause_secs: u64) {
    if auto {
        if pause_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(pause_secs)).await;
        }
        return;
    }
    print!("{DIM}  ── press Enter to continue ──{RESET}");
    std::io::stdout().flush().ok();
    let _ = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).ok();
    })
    .await;
}

/// Count down `secs` seconds with a reason.
pub async fn countdown(secs: u64, why: &str) {
    for left in (1..=secs).rev() {
        print!("\r  {DIM}⏳ {why} — {left}s {RESET}");
        std::io::stdout().flush().ok();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    println!("\r  {DIM}⏳ {why} — done.      {RESET}");
}

/// Short form of a z32 key or hash for display.
pub fn short(s: &str) -> String {
    if s.len() > 12 {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
}
