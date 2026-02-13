---
title: Storage Manager
description: Cross-platform storage management for SideSeat application data, configuration, cache, and logs.
---

The Storage Manager provides a unified API for managing application storage across Windows, macOS, and Linux. It handles platform-specific directory conventions, ensures directories exist and are accessible, and provides graceful fallback when standard paths are unavailable.

## Platform Paths

SideSeat stores data in platform-appropriate locations following OS conventions:

| Type            | Windows                          | macOS                                     | Linux                        |
| --------------- | -------------------------------- | ----------------------------------------- | ---------------------------- |
| **User Config** | `%USERPROFILE%\SideSeat\`        | `~/.sideseat/`                            | `~/.sideseat/`               |
| **Config**      | `%APPDATA%\SideSeat\config\`     | `~/Library/Application Support/SideSeat/` | `$XDG_CONFIG_HOME/sideseat/` |
| **Data**        | `%APPDATA%\SideSeat\data\`       | `~/Library/Application Support/SideSeat/` | `$XDG_DATA_HOME/sideseat/`   |
| **Cache**       | `%LOCALAPPDATA%\SideSeat\cache\` | `~/Library/Caches/SideSeat/`              | `$XDG_CACHE_HOME/sideseat/`  |
| **Logs**        | `%LOCALAPPDATA%\SideSeat\logs\`  | `~/Library/Logs/SideSeat/`                | `$XDG_STATE_HOME/sideseat/`  |
| **Temp**        | `%TEMP%\sideseat\`               | `$TMPDIR/sideseat/`                       | `/tmp/sideseat/`             |

### Linux XDG Defaults

On Linux, if XDG environment variables are not set, the following defaults are used:

- `$XDG_CONFIG_HOME` defaults to `~/.config/`
- `$XDG_DATA_HOME` defaults to `~/.local/share/`
- `$XDG_CACHE_HOME` defaults to `~/.cache/`
- `$XDG_STATE_HOME` defaults to `~/.local/state/`

## Directory Structure

```
data/
├── db/           # Database files
└── uploads/      # User uploaded content

cache/            # Regeneratable cached data
config/           # Application configuration
logs/             # Application logs
temp/             # Temporary processing files
```

## Environment Variable Overrides

You can override storage locations using environment variables:

| Variable              | Description               |
| --------------------- | ------------------------- |
| `SIDESEAT_CONFIG_DIR` | Override config directory |
| `SIDESEAT_DATA_DIR`   | Override data directory   |
| `SIDESEAT_CACHE_DIR`  | Override cache directory  |

```bash
# Example: Use custom data directory
export SIDESEAT_DATA_DIR=/mnt/storage/sideseat/data
```

## User Config Directory

The user config directory (`~/.sideseat` on Unix, `%USERPROFILE%\SideSeat` on Windows) is a special location where users can manually place configuration files. Unlike other directories, this is **not automatically created** by SideSeat.

To use custom configuration:

1. Create the directory manually:

   ```bash
   # Unix/macOS
   mkdir -p ~/.sideseat

   # Windows (PowerShell)
   mkdir $env:USERPROFILE\SideSeat
   ```

2. Place configuration files (e.g., `config.json`)

3. SideSeat will detect and load these files on startup

## Initialization

On startup, SideSeat:

1. Attempts to use platform-specific paths
2. Creates all required directories if they don't exist
3. Verifies each directory is accessible (read/write test)
4. Falls back to `.sideseat/` in the current directory if platform paths fail

### Initialization Errors

If initialization fails, you may see errors like:

- `"Failed to create {directory}"` - Permission denied or disk full
- `"{path} exists but is not a directory"` - A file exists at the expected directory path
- `"{directory} is not writable"` - Cannot write to the directory
- `"{directory} is not readable"` - Cannot read from the directory

## Fallback Mode

When standard platform paths are unavailable (e.g., no home directory, permission issues), SideSeat falls back to using `.sideseat/` in the current working directory. A warning is logged when fallback mode is active.

## API Reference

### StorageType Enum

```rust
pub enum StorageType {
    Config,  // Configuration files
    Data,    // Persistent application data
    Cache,   // Regeneratable cached data
    Logs,    // Application logs
    Temp,    // Temporary files
}
```

### DataSubdir Enum

```rust
pub enum DataSubdir {
    Uploads,     // data/uploads/
}
```

### StorageManager Methods

| Method                       | Description                                           |
| ---------------------------- | ----------------------------------------------------- |
| `init()`                     | Initialize storage manager and create directories     |
| `work_dir()`                 | Get current working directory (where app was started) |
| `user_config_dir()`          | Get user config directory path                        |
| `config_dir()`               | Get config directory path                             |
| `data_dir()`                 | Get data directory path                               |
| `cache_dir()`                | Get cache directory path                              |
| `logs_dir()`                 | Get logs directory path                               |
| `temp_dir()`                 | Get temp directory path                               |
| `using_fallback()`           | Check if using fallback storage                       |
| `data_subdir(subdir)`        | Get path to a data subdirectory                       |
| `get_path(type, filename)`   | Get full path for a file                              |
| `exists(path)`               | Check if a path exists                                |
| `is_writable(type)`          | Check if a storage location is writable               |
| `user_config_exists()`       | Check if user config directory exists                 |
| `user_config_path(filename)` | Get path to a user config file                        |
| `read_user_config(filename)` | Read a user config file                               |
| `list_user_config_files()`   | List files in user config directory                   |

## Usage Examples

```rust
use sideseat_server::core::{StorageManager, StorageType, DataSubdir};

// Initialize storage
let storage = StorageManager::init().await?;

// Get current working directory
let cwd = storage.work_dir();
println!("Started from: {:?}", cwd);

// Get data subdirectory path
let db_path = storage.data_subdir(DataSubdir::Database).join("app.db");

// Get config file path
let config_path = storage.get_path(StorageType::Config, "settings.json");

// Get cache directory for custom use
let cache_path = storage.cache_dir().join("my_cache_file");

// Check if using fallback location
if storage.using_fallback() {
    println!("Warning: Using fallback storage");
}

// Read user config if it exists
if let Some(content) = storage.read_user_config("config.json").await {
    println!("Found user config: {}", content);
}

// List all user config files
for file in storage.list_user_config_files().await {
    println!("User config file: {:?}", file);
}
```

## Best Practices

1. **Don't hardcode paths** - Always use `StorageManager` methods to get paths
2. **Check fallback mode** - Log a warning if `using_fallback()` returns true
3. **Handle missing user config** - User config directory may not exist
4. **Use appropriate storage types**:
   - `Config` for settings that should persist
   - `Cache` for data that can be regenerated
   - `Temp` for short-lived files
   - `Data` for user data and databases
