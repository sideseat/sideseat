use super::*;

#[test]
fn test_storage_backend_serde() {
    let json = r#""filesystem""#;
    let backend: StorageBackend = serde_json::from_str(json).unwrap();
    assert_eq!(backend, StorageBackend::Filesystem);

    let json = r#""s3""#;
    let backend: StorageBackend = serde_json::from_str(json).unwrap();
    assert_eq!(backend, StorageBackend::S3);
}

#[test]
fn test_storage_backend_display() {
    assert_eq!(StorageBackend::Filesystem.to_string(), "filesystem");
    assert_eq!(StorageBackend::S3.to_string(), "s3");
}

#[test]
fn test_file_config_parse_full() {
    let json = r#"{
            "server": { "host": "0.0.0.0", "port": 8080 },
            "auth": { "enabled": false }
        }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();

    assert_eq!(
        config.server.as_ref().unwrap().host,
        Some("0.0.0.0".to_string())
    );
    assert_eq!(config.server.as_ref().unwrap().port, Some(8080));
    assert_eq!(config.auth.as_ref().unwrap().enabled, Some(false));
}

#[test]
fn test_file_config_parse_nested_otel() {
    let json = r#"{
            "otel": {
                "grpc": { "enabled": false, "port": 4318 },
                "retention": { "max_age_minutes": 120, "max_spans": 1000000 }
            }
        }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();

    let otel = config.otel.as_ref().unwrap();
    let grpc = otel.grpc.as_ref().unwrap();
    let retention = otel.retention.as_ref().unwrap();

    assert_eq!(grpc.enabled, Some(false));
    assert_eq!(grpc.port, Some(4318));
    assert_eq!(retention.max_age_minutes, Some(120));
    assert_eq!(retention.max_spans, Some(1_000_000));
}

#[test]
fn test_file_config_parse_partial() {
    let json = r#"{ "server": { "port": 9000 } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();

    assert!(config.server.as_ref().unwrap().host.is_none());
    assert_eq!(config.server.as_ref().unwrap().port, Some(9000));
    assert!(config.auth.is_none());
}

#[test]
fn test_file_config_parse_empty() {
    let json = "{}";
    let config: FileConfig = serde_json::from_str(json).unwrap();

    assert!(config.server.is_none());
    assert!(config.auth.is_none());
}

#[test]
fn test_file_config_parse_extra_fields() {
    let json = r#"{ "server": { "host": "localhost" }, "unknown_field": 123 }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();

    assert_eq!(
        config.server.as_ref().unwrap().host,
        Some("localhost".to_string())
    );
    assert_eq!(config.extra.get("unknown_field").unwrap(), 123);
}

#[test]
fn test_file_config_parse_storage_backend() {
    let json = r#"{ "files": { "storage": "s3", "enabled": true } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();

    assert_eq!(
        config.files.as_ref().unwrap().storage,
        Some(StorageBackend::S3)
    );
    assert_eq!(config.files.as_ref().unwrap().enabled, Some(true));
}

#[test]
fn test_file_config_merge() {
    let mut base = FileConfig {
        server: Some(ServerFileConfig {
            host: Some("base.host".to_string()),
            port: Some(1000),
            mcp: None,
        }),
        auth: Some(AuthFileConfig {
            enabled: Some(true),
        }),
        otel: Some(OtelFileConfig {
            grpc: Some(GrpcFileConfig {
                enabled: Some(true),
                port: Some(4317),
            }),
            retention: Some(RetentionFileConfig {
                max_age_minutes: Some(60),
                max_spans: None,
            }),
            auth: None,
            staging_redrive_cap: None,
        }),
        pricing: Some(PricingFileConfig {
            sync_hours: Some(4),
        }),
        files: None,
        rate_limit: None,
        update: None,
        database: None,
        secrets: None,
        credentials: None,
        debug: Some(false),
        extra: serde_json::Value::Null,
    };

    let overlay = FileConfig {
        server: Some(ServerFileConfig {
            host: None,
            port: Some(2000),
            mcp: None,
        }),
        auth: Some(AuthFileConfig {
            enabled: Some(false),
        }),
        otel: Some(OtelFileConfig {
            grpc: Some(GrpcFileConfig {
                enabled: Some(false),
                port: None,
            }),
            retention: Some(RetentionFileConfig {
                max_age_minutes: None,
                max_spans: Some(1_000_000),
            }),
            auth: None,
            staging_redrive_cap: None,
        }),
        pricing: Some(PricingFileConfig {
            sync_hours: Some(8),
        }),
        files: None,
        rate_limit: None,
        update: None,
        database: None,
        secrets: None,
        credentials: None,
        debug: Some(true),
        extra: serde_json::Value::Null,
    };

    base.merge(overlay);

    assert_eq!(
        base.server.as_ref().unwrap().host,
        Some("base.host".to_string())
    );
    assert_eq!(base.server.as_ref().unwrap().port, Some(2000));
    assert_eq!(base.auth.as_ref().unwrap().enabled, Some(false));

    let otel = base.otel.as_ref().unwrap();
    assert_eq!(otel.grpc.as_ref().unwrap().enabled, Some(false));
    assert_eq!(otel.grpc.as_ref().unwrap().port, Some(4317));
    assert_eq!(otel.retention.as_ref().unwrap().max_age_minutes, Some(60));
    assert_eq!(otel.retention.as_ref().unwrap().max_spans, Some(1_000_000));

    assert_eq!(base.pricing.as_ref().unwrap().sync_hours, Some(8));
    assert_eq!(base.debug, Some(true));
}

#[test]
fn test_app_config_defaults() {
    let cli = CliConfig::default();
    let config = AppConfig::load(&cli).unwrap();

    assert_eq!(config.server.host, DEFAULT_HOST);
    assert_eq!(config.server.port, DEFAULT_PORT);
    assert!(config.auth.enabled);
    assert!(!config.debug);
    assert_eq!(config.files.storage, StorageBackend::Filesystem);
    assert_eq!(config.database.queue, QueueBackendType::Memory);
}

#[test]
fn queue_backend_is_independent_from_cache_backend() {
    let cli = CliConfig {
        cache_backend: Some(CacheBackendType::Redis),
        cache_redis_url: Some("redis://127.0.0.1:6379".to_string()),
        queue_backend: Some(QueueBackendType::Redpanda),
        redpanda_brokers: Some("redpanda.internal:9092".to_string()),
        ..CliConfig::default()
    };

    let config = AppConfig::load(&cli).unwrap();

    assert_eq!(config.database.cache, CacheBackendType::Redis);
    assert_eq!(config.database.queue, QueueBackendType::Redpanda);
    assert_eq!(
        config.database.redpanda.as_ref().unwrap().brokers,
        "redpanda.internal:9092"
    );
    assert_eq!(
        config.database.cache_config().backend,
        CacheBackendType::Redis
    );
    assert_eq!(
        config.database.queue_config().backend,
        QueueBackendType::Redpanda
    );
}

#[test]
fn test_app_config_cli_override() {
    let cli = CliConfig {
        host: Some("cli.host".to_string()),
        port: Some(3000),
        no_auth: true,
        debug: true,
        config: None,
        otel_grpc: Some(false),
        otel_grpc_port: Some(4318),
        otel_retention_max_age: Some(120),
        otel_retention_max_spans: Some(1_000_000),
        otel_auth_required: None,
        pricing_sync_hours: Some(12),
        no_update_check: true,
        files_enabled: Some(false),
        files_storage: None,
        files_quota_bytes: Some(500_000_000),
        files_s3_bucket: None,
        files_s3_prefix: None,
        files_s3_region: None,
        files_s3_endpoint: None,
        cache_backend: None,
        cache_max_entries: None,
        cache_eviction_policy: None,
        cache_redis_url: None,
        queue_backend: None,
        redpanda_brokers: None,
        rate_limit_enabled: None,
        rate_limit_per_ip: None,
        rate_limit_api_rpm: None,
        rate_limit_ingestion_rpm: None,
        rate_limit_auth_rpm: None,
        rate_limit_files_rpm: None,
        rate_limit_bypass_header: None,
        secrets_backend: None,
        mcp: None,
        transactional_backend: None,
        analytics_backend: None,
        postgres_url: None,
        clickhouse_url: None,
        credentials_scan_env: None,
    };
    let config = AppConfig::load(&cli).unwrap();

    assert_eq!(config.server.host, "cli.host");
    assert_eq!(config.server.port, 3000);
    assert!(!config.auth.enabled);
    assert!(config.debug);
    assert!(!config.otel.grpc_enabled);
    assert_eq!(config.otel.grpc_port, 4318);
    assert_eq!(config.otel.retention.max_age_minutes, Some(120));
    assert_eq!(config.otel.retention.max_spans, Some(1_000_000));
    assert_eq!(config.pricing.sync_hours, 12);
    assert!(!config.files.enabled);
    assert_eq!(config.files.quota_bytes, 500_000_000);
}

#[test]
fn test_file_config_parse_pricing() {
    let json = r#"{ "pricing": { "sync_hours": 12 } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.pricing.as_ref().unwrap().sync_hours, Some(12));
}

#[test]
fn test_app_config_pricing_defaults() {
    let cli = CliConfig::default();
    let config = AppConfig::load(&cli).unwrap();
    assert_eq!(config.pricing.sync_hours, PRICING_SYNC_INTERVAL_SECS / 3600);
}

#[test]
fn test_app_config_pricing_disabled() {
    let cli = CliConfig {
        pricing_sync_hours: Some(0),
        ..Default::default()
    };
    let config = AppConfig::load(&cli).unwrap();
    assert_eq!(config.pricing.sync_hours, 0);
}

#[test]
fn test_file_config_parse_update() {
    let json = r#"{ "update": { "enabled": false } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.update.as_ref().unwrap().enabled, Some(false));
}

#[test]
fn test_app_config_update_defaults() {
    let cli = CliConfig::default();
    let config = AppConfig::load(&cli).unwrap();
    assert!(config.update.enabled);
}

#[test]
fn test_app_config_update_cli_override() {
    let cli = CliConfig {
        no_update_check: true,
        ..Default::default()
    };
    let config = AppConfig::load(&cli).unwrap();
    assert!(!config.update.enabled);
}

#[test]
fn test_app_config_validation_port_collision() {
    let cli = CliConfig {
        port: Some(4317),
        otel_grpc_port: Some(4317),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("cannot be the same")
    );
}

#[test]
fn test_app_config_validation_s3_bucket_required() {
    let cli = CliConfig {
        files_storage: Some(StorageBackend::S3),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("files.s3.bucket is required")
    );
}

#[test]
fn test_app_config_validation_port_collision_disabled_grpc() {
    // Should NOT error if gRPC is disabled
    let cli = CliConfig {
        port: Some(4317),
        otel_grpc: Some(false),
        otel_grpc_port: Some(4317),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_ok());
}

#[test]
fn test_app_config_validation_server_port_zero() {
    let cli = CliConfig {
        port: Some(0),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("server.port must be greater than 0")
    );
}

#[test]
fn test_app_config_validation_grpc_port_zero() {
    let cli = CliConfig {
        otel_grpc: Some(true),
        otel_grpc_port: Some(0),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("otel.grpc.port must be greater than 0")
    );
}

#[test]
fn test_app_config_validation_grpc_port_zero_disabled() {
    // Port 0 should be OK if gRPC is disabled
    let cli = CliConfig {
        otel_grpc: Some(false),
        otel_grpc_port: Some(0),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_ok());
}

#[test]
fn test_app_config_validation_empty_host() {
    let cli = CliConfig {
        host: Some(String::new()),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("server.host must not be empty")
    );
}

#[test]
fn test_app_config_s3_empty_bucket_rejected() {
    // Test that empty bucket string in config file is rejected
    use std::io::Write;

    let json = r#"{
            "files": {
                "storage": "s3",
                "s3": { "bucket": "" }
            }
        }"#;

    // Create temp config file
    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();

    // Load config pointing to temp file
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        ..Default::default()
    };

    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("files.s3.bucket is required")
    );
}

#[test]
fn test_is_all_interfaces() {
    // Should match all-interfaces bindings
    assert!(is_all_interfaces("0.0.0.0"));
    assert!(is_all_interfaces("::"));
    assert!(is_all_interfaces("[::]"));

    // Should not match localhost or specific IPs
    assert!(!is_all_interfaces("127.0.0.1"));
    assert!(!is_all_interfaces("localhost"));
    assert!(!is_all_interfaces("::1"));
    assert!(!is_all_interfaces("192.168.1.1"));
}

#[test]
fn test_app_config_s3_valid_bucket() {
    // Test that valid S3 config with non-empty bucket loads successfully
    use std::io::Write;

    let json = r#"{
            "files": {
                "storage": "s3",
                "s3": {
                    "bucket": "my-bucket",
                    "prefix": "custom/prefix",
                    "region": "us-west-2"
                }
            }
        }"#;

    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();

    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        ..Default::default()
    };

    let config = AppConfig::load(&cli).unwrap();
    assert_eq!(config.files.storage, StorageBackend::S3);
    assert!(config.files.s3.is_some());

    let s3 = config.files.s3.unwrap();
    assert_eq!(s3.bucket, "my-bucket");
    assert_eq!(s3.prefix, "custom/prefix");
    assert_eq!(s3.region, Some("us-west-2".to_string()));
    assert!(s3.endpoint.is_none());
}

#[test]
fn test_file_config_parse_mcp_under_server() {
    let json = r#"{ "server": { "host": "0.0.0.0", "mcp": { "enabled": false } } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();
    let server = config.server.unwrap();
    assert_eq!(server.host, Some("0.0.0.0".to_string()));
    assert_eq!(server.mcp.unwrap().enabled, Some(false));
}

#[test]
fn test_file_config_parse_mcp_absent() {
    let json = r#"{ "server": { "port": 9000 } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();
    assert!(config.server.unwrap().mcp.is_none());
}

#[test]
fn test_file_config_merge_mcp() {
    let mut base = FileConfig {
        server: Some(ServerFileConfig {
            host: Some("localhost".to_string()),
            port: None,
            mcp: Some(McpFileConfig {
                enabled: Some(true),
            }),
        }),
        ..Default::default()
    };
    let overlay = FileConfig {
        server: Some(ServerFileConfig {
            host: None,
            port: None,
            mcp: Some(McpFileConfig {
                enabled: Some(false),
            }),
        }),
        ..Default::default()
    };
    base.merge(overlay);
    let mcp = base.server.unwrap().mcp.unwrap();
    assert_eq!(mcp.enabled, Some(false));
}

#[test]
fn test_file_config_merge_mcp_partial() {
    let mut base = FileConfig {
        server: Some(ServerFileConfig {
            host: None,
            port: None,
            mcp: Some(McpFileConfig {
                enabled: Some(true),
            }),
        }),
        ..Default::default()
    };
    // Overlay has server but no mcp — base mcp preserved
    let overlay = FileConfig {
        server: Some(ServerFileConfig {
            host: Some("new-host".to_string()),
            port: None,
            mcp: None,
        }),
        ..Default::default()
    };
    base.merge(overlay);
    let server = base.server.unwrap();
    assert_eq!(server.host, Some("new-host".to_string()));
    assert_eq!(server.mcp.unwrap().enabled, Some(true));
}

#[test]
fn test_app_config_mcp_enabled_by_default() {
    let cli = CliConfig::default();
    let config = AppConfig::load(&cli).unwrap();
    assert!(config.mcp.enabled);
}

#[test]
fn test_app_config_mcp_cli_override() {
    let cli = CliConfig {
        mcp: Some(false),
        ..Default::default()
    };
    let config = AppConfig::load(&cli).unwrap();
    assert!(!config.mcp.enabled);
}

#[test]
fn test_app_config_mcp_file_override() {
    use std::io::Write;
    let json = r#"{ "server": { "mcp": { "enabled": false } } }"#;
    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        ..Default::default()
    };
    let config = AppConfig::load(&cli).unwrap();
    assert!(!config.mcp.enabled);
}

#[test]
fn test_secrets_aws_recovery_window_days_from_json() {
    let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 14 } } }"#;
    let config: FileConfig = serde_json::from_str(json).unwrap();
    let aws = config.secrets.unwrap().aws.unwrap();
    assert_eq!(aws.recovery_window_days, Some(14));
}

#[test]
fn test_secrets_aws_recovery_window_days_validation_too_low() {
    use std::io::Write;
    let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 5 } } }"#;
    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        secrets_backend: Some(SecretsBackend::Aws),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("between 7 and 30"));
}

#[test]
fn test_secrets_aws_recovery_window_days_validation_too_high() {
    use std::io::Write;
    let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 50 } } }"#;
    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        secrets_backend: Some(SecretsBackend::Aws),
        ..Default::default()
    };
    let result = AppConfig::load(&cli);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("between 7 and 30"));
}

#[test]
fn test_secrets_aws_recovery_window_days_valid() {
    use std::io::Write;
    let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 7 } } }"#;
    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        secrets_backend: Some(SecretsBackend::Aws),
        ..Default::default()
    };
    let config = AppConfig::load(&cli).unwrap();
    let aws = config.secrets.aws.unwrap();
    assert_eq!(aws.recovery_window_days, Some(7));
}

#[test]
fn test_secrets_aws_recovery_window_days_omitted() {
    use std::io::Write;
    let json = r#"{ "secrets": { "backend": "aws", "aws": { "region": "us-east-1" } } }"#;
    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(json.as_bytes()).unwrap();
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        secrets_backend: Some(SecretsBackend::Aws),
        ..Default::default()
    };
    let config = AppConfig::load(&cli).unwrap();
    let aws = config.secrets.aws.unwrap();
    assert!(aws.recovery_window_days.is_none());
}

/// The three file-level failures keep the wording `anyhow`'s context produced, and the two that have a
/// cause keep it reachable - the `io::Error` says *why* the read failed, and `serde_json`'s error says
/// which line of JSON is wrong, which the message deliberately does not repeat.
#[test]
fn a_missing_config_file_is_named_by_its_own_variant() {
    let missing = std::path::PathBuf::from("/nonexistent/sideseat-does-not-exist.json");
    let cli = CliConfig {
        config: Some(missing.clone()),
        ..Default::default()
    };

    let error = AppConfig::load(&cli).unwrap_err();
    assert!(matches!(error, ConfigError::NotFound { .. }));
    assert_eq!(
        error.to_string(),
        format!("Config file not found: {}", missing.display())
    );
}

#[test]
fn an_unparseable_config_file_keeps_the_path_and_the_serde_cause() {
    use std::io::Write;

    let mut temp_file = tempfile::NamedTempFile::new().unwrap();
    temp_file.write_all(b"{ not json").unwrap();
    let cli = CliConfig {
        config: Some(temp_file.path().to_path_buf()),
        ..Default::default()
    };

    let error = AppConfig::load(&cli).unwrap_err();
    assert!(matches!(error, ConfigError::Parse { .. }));
    assert_eq!(
        error.to_string(),
        format!(
            "Failed to parse config file: {}",
            temp_file.path().display()
        )
    );
    assert!(std::error::Error::source(&error).is_some());
}

/// Every refusal still reads "Configuration error: ...", which is what the messages the operator-facing
/// docs quote begin with, and what the rest of this file asserts substrings of.
#[test]
fn a_rejected_setting_keeps_the_configuration_error_prefix() {
    let cli = CliConfig {
        host: Some(String::new()),
        ..Default::default()
    };

    let error = AppConfig::load(&cli).unwrap_err();
    assert!(matches!(error, ConfigError::Invalid(_)));
    assert_eq!(
        error.to_string(),
        "Configuration error: server.host must not be empty"
    );
}
