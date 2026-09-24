//! File utility functions.

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

    #[cfg(unix)]
    #[test]
    fn absolute_unix_path_is_unchanged() {
        assert_eq!(
            expand_path("  /absolute/path  "),
            PathBuf::from("/absolute/path")
        );
    }

    #[cfg(windows)]
    #[test]
    fn absolute_windows_paths_are_unchanged() {
        for path in ["C:\\Users\\sideseat", "D:\\data"] {
            assert_eq!(expand_path(path), PathBuf::from(path));
        }
    }

    #[test]
    fn relative_paths_are_rooted_at_the_current_directory() {
        let cwd = std::env::current_dir().unwrap();
        for path in [
            ".",
            "..",
            "./relative",
            "../config",
            "mydata",
            "data.db",
            "./foo/../bar/./baz",
            "data/traces",
        ] {
            assert_eq!(expand_path(path), cwd.join(path), "{path}");
        }
    }

    #[test]
    fn tilde_paths_are_rooted_at_the_home_directory() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_path("~"), home);
            assert_eq!(expand_path("~/.sideseat"), home.join(".sideseat"));
            assert_eq!(
                expand_path("~/path/to/data"),
                home.join("path").join("to").join("data")
            );
        }
    }

    #[test]
    fn empty_and_whitespace_paths_resolve_to_the_current_directory() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(expand_path(""), cwd);
        assert_eq!(expand_path("   "), cwd);
        assert_eq!(expand_path("  data  "), cwd.join("data"));
    }
}
