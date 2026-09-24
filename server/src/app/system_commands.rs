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

    let data_dir = validate_prune_target(&data_dir)?;

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

fn validate_prune_target(data_dir: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(data_dir)
        .with_context(|| format!("Failed to inspect data directory: {}", data_dir.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "Refusing to prune symbolic-link data directory: {}",
            data_dir.display()
        );
    }
    if !metadata.is_dir() {
        bail!(
            "Refusing to prune data path that is not a directory: {}",
            data_dir.display()
        );
    }

    let target = data_dir
        .canonicalize()
        .with_context(|| format!("Failed to resolve data directory: {}", data_dir.display()))?;
    if target.parent().is_none() {
        bail!(
            "Refusing to prune filesystem root configured as the data directory: {}",
            target.display()
        );
    }

    let current_dir = std::env::current_dir()
        .context("Failed to resolve the current working directory")?
        .canonicalize()
        .context("Failed to canonicalize the current working directory")?;
    if current_dir.starts_with(&target) {
        bail!(
            "Refusing to prune data directory containing the current working directory: {}",
            target.display()
        );
    }

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok());
    if home.as_ref().is_some_and(|home| home.starts_with(&target)) {
        bail!(
            "Refusing to prune data directory containing the home directory: {}",
            target.display()
        );
    }

    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::validate_prune_target;

    #[test]
    fn prune_target_accepts_a_dedicated_directory() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();

        assert_eq!(
            validate_prune_target(&data).unwrap(),
            data.canonicalize().unwrap()
        );
    }

    #[test]
    fn prune_target_rejects_filesystem_root_and_working_directory() {
        let current = std::env::current_dir().unwrap().canonicalize().unwrap();
        let root = current.ancestors().last().unwrap();

        assert!(validate_prune_target(root).is_err());
        assert!(validate_prune_target(&current).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn prune_target_rejects_a_symbolic_link() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let link = root.path().join("data-link");
        std::fs::create_dir(&data).unwrap();
        std::os::unix::fs::symlink(&data, &link).unwrap();

        assert!(validate_prune_target(&link).is_err());
    }
}
