//! Utility functions for the application

/// Format a URL as a clickable terminal hyperlink if supported.
///
/// Uses OSC 8 escape sequences for terminals that support hyperlinks
/// (iTerm2, Windows Terminal, GNOME Terminal, VS Code terminal, etc.).
/// Falls back to plain colored text on unsupported terminals.
pub fn terminal_link(url: &str) -> String {
    if supports_hyperlinks::on(supports_hyperlinks::Stream::Stdout) {
        // OSC 8 hyperlink: \x1b]8;;URL\x07TEXT\x1b]8;;\x07
        format!("\x1b]8;;{}\x07\x1b[36m{}\x1b[0m\x1b]8;;\x07", url, url)
    } else {
        // Plain colored text for unsupported terminals
        format!("\x1b[36m{}\x1b[0m", url)
    }
}
