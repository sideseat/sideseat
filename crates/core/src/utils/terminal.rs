//! Terminal utility functions

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_link_returns_colored_text() {
        let url = "https://example.com";
        let result = terminal_link(url);

        // The result should contain the URL
        assert!(result.contains(url));

        // The result should contain ANSI escape sequences for cyan color
        assert!(result.contains("\x1b[36m"));
        assert!(result.contains("\x1b[0m"));
    }

    #[test]
    fn test_terminal_link_with_empty_url() {
        let result = terminal_link("");
        // Should still return valid colored output
        assert!(result.contains("\x1b[36m"));
        assert!(result.contains("\x1b[0m"));
    }

    #[test]
    fn test_terminal_link_with_special_characters() {
        let url = "https://example.com/path?query=value&other=123";
        let result = terminal_link(url);

        // The URL should be preserved
        assert!(result.contains(url));
    }

    #[test]
    fn test_terminal_link_format_without_hyperlinks() {
        // When hyperlinks are not supported, format should be: \x1b[36m{url}\x1b[0m
        let url = "https://test.com";
        let expected_plain = format!("\x1b[36m{}\x1b[0m", url);

        let result = terminal_link(url);

        // Result should either be the plain format or the hyperlink format
        // Both should contain the cyan color codes
        assert!(result.contains("\x1b[36m"));
        assert!(result.contains("\x1b[0m"));

        // If not a hyperlink, should match plain format exactly
        if !result.contains("\x1b]8;;") {
            assert_eq!(result, expected_plain);
        }
    }

    #[test]
    fn test_terminal_link_hyperlink_format() {
        let url = "https://test.com";
        let result = terminal_link(url);

        // If hyperlinks are supported, should have OSC 8 sequences
        if result.contains("\x1b]8;;") {
            // Should have opening hyperlink sequence
            assert!(result.contains(&format!("\x1b]8;;{}\x07", url)));
            // Should have closing hyperlink sequence
            assert!(result.contains("\x1b]8;;\x07"));
        }
    }
}
