//! File utility functions

use std::path::PathBuf;

/// Expand a path string to an absolute path.
///
/// Cross-platform path expansion that handles:
/// - Tilde expansion: `~` or `~/path` -> home directory
/// - Relative paths: `.`, `..`, `./path`, `../path` -> absolute path
/// - Bare names: `foo` -> `./foo` -> absolute path in current directory
/// - Absolute paths: passed through unchanged
///
/// Works on Windows, Linux, and macOS.
///
/// # Examples
///
/// ```text
/// // Tilde expansion
/// expand_path("~/.sideseat") // -> /home/user/.sideseat (Linux/macOS)
/// expand_path("~")           // -> /home/user
///
/// // Relative paths
/// expand_path("./data")      // -> /current/dir/data
/// expand_path("../config")   // -> /current/config
/// expand_path(".")           // -> /current/dir
/// expand_path("..")          // -> /current
///
/// // Bare names (treated as relative to current directory)
/// expand_path("mydata")      // -> /current/dir/mydata
///
/// // Absolute paths (unchanged)
/// expand_path("/etc/config") // -> /etc/config
/// expand_path("C:\\Users")   // -> C:\Users (Windows)
/// ```
pub fn expand_path(path: &str) -> PathBuf {
    let path = path.trim();

    if path.is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }

    // Handle tilde expansion (Unix convention, also works on Windows with dirs crate)
    let expanded = if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(path))
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(rest)
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };

    // Convert relative paths to absolute using current working directory
    // This handles: ".", "..", "./foo", "../foo", "foo" (bare name)
    if expanded.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    } else {
        expanded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_path_absolute_unix() {
        // Absolute Unix paths should remain unchanged
        let result = expand_path("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[cfg(windows)]
    #[test]
    fn test_expand_path_absolute_windows() {
        // Absolute Windows paths should remain unchanged
        let result = expand_path("C:\\Users\\test");
        assert_eq!(result, PathBuf::from("C:\\Users\\test"));

        let result = expand_path("D:\\data");
        assert_eq!(result, PathBuf::from("D:\\data"));
    }

    #[test]
    fn test_expand_path_relative_dot() {
        // "." should expand to an absolute path containing current directory
        let result = expand_path(".");
        assert!(result.is_absolute(), ". should become absolute");
        // Result should be cwd/. which is a valid absolute path
        assert!(
            result.to_string_lossy().ends_with("/.") || result.to_string_lossy().ends_with("\\."),
            "Result should end with '/.' or '\\.': {:?}",
            result
        );
    }

    #[test]
    fn test_expand_path_relative_dotdot() {
        // ".." should expand to parent directory
        let result = expand_path("..");
        assert!(result.is_absolute(), ".. should become absolute");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(result, cwd.join(".."));
    }

    #[test]
    fn test_expand_path_relative_dot_slash() {
        // "./relative" should expand to current directory + relative
        let result = expand_path("./relative");
        assert!(result.is_absolute(), "./relative should become absolute");
        assert!(result.ends_with("relative"));
    }

    #[test]
    fn test_expand_path_relative_dotdot_slash() {
        // "../config" should expand to parent directory + config
        let result = expand_path("../config");
        assert!(result.is_absolute(), "../config should become absolute");
        assert!(result.to_string_lossy().contains("config"));
    }

    #[test]
    fn test_expand_path_bare_name() {
        // Bare name "mydata" should expand to current directory + mydata
        let result = expand_path("mydata");
        assert!(result.is_absolute(), "Bare name should become absolute");
        assert!(result.ends_with("mydata"));
    }

    #[test]
    fn test_expand_path_bare_name_with_extension() {
        // Bare name with extension
        let result = expand_path("data.db");
        assert!(result.is_absolute());
        assert!(result.ends_with("data.db"));
    }

    #[test]
    fn test_expand_path_tilde() {
        // "~/.sideseat" should expand to home directory
        let result = expand_path("~/.sideseat");
        assert!(result.is_absolute(), "Tilde path should become absolute");
        assert!(
            !result.to_string_lossy().contains('~'),
            "Tilde should be expanded"
        );
        assert!(result.ends_with(".sideseat"));
    }

    #[test]
    fn test_expand_path_tilde_only() {
        // Just tilde should expand to home directory
        let result = expand_path("~");
        assert!(result.is_absolute());
        assert!(!result.to_string_lossy().contains('~'));

        // Should match home directory
        if let Some(home) = dirs::home_dir() {
            assert_eq!(result, home);
        }
    }

    #[test]
    fn test_expand_path_tilde_nested() {
        // Nested tilde path
        let result = expand_path("~/path/to/data");
        assert!(result.is_absolute());
        assert!(result.ends_with("path/to/data") || result.ends_with("path\\to\\data"));
    }

    #[test]
    fn test_expand_path_dot_sideseat() {
        // Common use case: ./.sideseat should become absolute
        let result = expand_path("./.sideseat");
        assert!(result.is_absolute(), "./.sideseat should become absolute");
        assert!(result.ends_with(".sideseat"));
    }

    #[test]
    fn test_expand_path_trims_whitespace() {
        // Whitespace should be trimmed
        let result = expand_path("  /path/to/dir  ");
        assert_eq!(result, PathBuf::from("/path/to/dir"));
    }

    #[test]
    fn test_expand_path_empty_string() {
        // Empty string should return current directory
        let result = expand_path("");
        assert!(result.is_absolute());
        // Just verify it's a valid absolute path
        assert!(!result.as_os_str().is_empty());
    }

    #[test]
    fn test_expand_path_whitespace_only() {
        // Whitespace-only should return current directory
        let result = expand_path("   ");
        assert!(result.is_absolute());
        // Just verify it's a valid absolute path
        assert!(!result.as_os_str().is_empty());
    }

    #[test]
    fn test_expand_path_complex_relative() {
        // Complex relative path
        let result = expand_path("./foo/../bar/./baz");
        assert!(result.is_absolute());
        // The path components are preserved (not canonicalized)
        assert!(result.to_string_lossy().contains("bar"));
    }

    #[test]
    fn test_expand_path_preserves_structure() {
        // Verify path structure is preserved for relative paths
        let result = expand_path("data/traces");
        assert!(result.is_absolute());
        assert!(result.ends_with("data/traces") || result.ends_with("data\\traces"));
    }
}
