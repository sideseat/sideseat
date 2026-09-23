//! Destructive and repair-oriented system command workflows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use sideseat_core::constants::RESTORE_PENDING_MARKER;
use sideseat_core::storage::AppStorage;
use sideseat_domain::restore::{
    AssociationRepairReport, JournalReplayReport, reconcile_restored_associations,
    replay_deletion_journal,
};
use sideseat_domain::storage_governance::RestoreQuotaRepairReport;

use super::CoreApp;

#[derive(Debug, Serialize)]
struct RestoreRepairCommandReport {
    journal: JournalReplayReport,
    retention_completed: bool,
    association_repair_before_quota: AssociationRepairReport,
    quotas: Vec<RestoreQuotaRepairReport>,
    association_repair_after_quota: AssociationRepairReport,
}

fn restore_marker_path() -> PathBuf {
    AppStorage::resolve_data_dir().join(RESTORE_PENDING_MARKER)
}

pub(super) async fn mark_restore_pending() -> Result<()> {
    let marker = restore_marker_path();
    if let Some(parent) = marker.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "Failed to create restore data directory: {}",
                parent.display()
            )
        })?;
    }
    tokio::fs::write(
        &marker,
        b"Restore repair is required before SideSeat may serve this data.\n",
    )
    .await
    .with_context(|| format!("Failed to create restore marker: {}", marker.display()))
}

pub(super) fn refuse_pending_restore() -> Result<()> {
    let marker = restore_marker_path();
    if marker.exists() {
        bail!(
            "restored data is pending repair (marker: {}). Run `sideseat system restore-repair` before starting the server",
            marker.display()
        );
    }
    Ok(())
}

pub(super) async fn run_restore_repair(app: CoreApp, report_path: Option<&Path>) -> Result<()> {
    let journal =
        replay_deletion_journal(&app.database_port, &app.analytics_port, &app.files).await?;

    app.analytics
        .run_retention_to_completion(
            &app.config.otel.retention,
            app.config.files.quota_bytes,
            Arc::clone(&app.files),
            Arc::clone(&app.database),
            Arc::from(app.database.governance_repository()),
        )
        .await
        .context("Restore retention did not complete")?;

    let association_repair_before_quota =
        reconcile_restored_associations(&app.database_port, &app.analytics_port, &app.files)
            .await?;
    let quotas = app
        .storage_governance
        .repair_all_quotas_after_restore()
        .await?;
    let association_repair_after_quota =
        reconcile_restored_associations(&app.database_port, &app.analytics_port, &app.files)
            .await?;

    app.database.checkpoint().await?;
    app.analytics.checkpoint().await?;

    let report = RestoreRepairCommandReport {
        journal,
        retention_completed: true,
        association_repair_before_quota,
        quotas,
        association_repair_after_quota,
    };
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = report_path {
        tokio::fs::write(path, format!("{json}\n"))
            .await
            .with_context(|| format!("Failed to write restore report: {}", path.display()))?;
    } else {
        println!("{json}");
    }

    let marker = app.storage.data_path(RESTORE_PENDING_MARKER);
    match tokio::fs::remove_file(&marker).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Repair succeeded but restore marker remains: {}",
                    marker.display()
                )
            });
        }
    }
    Ok(())
}

pub(super) fn prune_data(skip_confirm: bool) -> Result<()> {
    let data_dir = AppStorage::resolve_data_dir();

    if !data_dir.exists() {
        println!(
            "Nothing to prune. Data directory does not exist: {}",
            data_dir.display()
        );
        return Ok(());
    }

    let data_dir = data_dir.canonicalize().unwrap_or(data_dir);

    println!("This will permanently delete the local data directory:");
    println!("  {}", data_dir.display());
    println!();
    println!(
        "Make sure the server is not running. \
         Deleting data while the server is running will cause data corruption."
    );

    if !skip_confirm {
        print!("\nContinue? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    std::fs::remove_dir_all(&data_dir)
        .with_context(|| format!("Failed to delete data directory: {}", data_dir.display()))?;
    println!("Pruned: {}", data_dir.display());
    Ok(())
}
