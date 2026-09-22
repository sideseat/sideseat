//! Driver-independent schema migration planning.
//!
//! Adapters keep their SQL and transaction mechanics. This module owns the state machine that
//! decides whether to initialize, reject, stay current, or walk an exact contiguous migration path.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStep {
    pub version: i32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationRun {
    Initialize { target: i32 },
    UpToDate { version: i32 },
    Apply(Vec<MigrationStep>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationPlanError {
    InvalidTarget(i32),
    InvalidMinimum {
        minimum: i32,
        target: i32,
    },
    DuplicateVersion(i32),
    UnexpectedVersion {
        version: i32,
        minimum: i32,
        target: i32,
    },
    MissingVersion(i32),
    DatabaseTooOld {
        current: i32,
        minimum: i32,
    },
    DatabaseTooNew {
        current: i32,
        target: i32,
    },
}

impl MigrationPlanError {
    pub const fn version(&self) -> i32 {
        match *self {
            Self::InvalidTarget(version)
            | Self::DuplicateVersion(version)
            | Self::MissingVersion(version)
            | Self::UnexpectedVersion { version, .. } => version,
            Self::InvalidMinimum { minimum, .. } => minimum,
            Self::DatabaseTooOld { current, .. } | Self::DatabaseTooNew { current, .. } => current,
        }
    }
}

impl fmt::Display for MigrationPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(target) => {
                write!(formatter, "schema target {target} is not positive")
            }
            Self::InvalidMinimum { minimum, target } => write!(
                formatter,
                "minimum upgradable schema {minimum} exceeds target {target}"
            ),
            Self::DuplicateVersion(version) => {
                write!(
                    formatter,
                    "migration version {version} is declared more than once"
                )
            }
            Self::UnexpectedVersion {
                version,
                minimum,
                target,
            } => write!(
                formatter,
                "migration version {version} lies outside the required range {}..={target}",
                minimum + 1
            ),
            Self::MissingVersion(version) => {
                write!(
                    formatter,
                    "no migration is declared for schema version {version}"
                )
            }
            Self::DatabaseTooOld { current, minimum } => write!(
                formatter,
                "database schema version {current} is older than the minimum upgradable version {minimum}"
            ),
            Self::DatabaseTooNew { current, target } => write!(
                formatter,
                "database schema version {current} is newer than application version {target}; upgrade the application"
            ),
        }
    }
}

impl std::error::Error for MigrationPlanError {}

/// Plan one migration run from the adapter's current version and data table.
///
/// `None` means no version row and selects fresh initialization. Every declared table is validated
/// as a complete, unique sequence from `minimum + 1` through `target`, even when this particular
/// database is already current, so a version bump without a migration fails before deployment.
pub fn plan_migrations<'a>(
    current: Option<i32>,
    target: i32,
    minimum: i32,
    migrations: impl IntoIterator<Item = (i32, &'a str)>,
) -> Result<MigrationRun, MigrationPlanError> {
    if target <= 0 {
        return Err(MigrationPlanError::InvalidTarget(target));
    }
    if minimum > target {
        return Err(MigrationPlanError::InvalidMinimum { minimum, target });
    }

    let mut declared = migrations
        .into_iter()
        .map(|(version, name)| MigrationStep {
            version,
            name: name.to_string(),
        })
        .collect::<Vec<_>>();
    declared.sort_by_key(|migration| migration.version);

    for pair in declared.windows(2) {
        if pair[0].version == pair[1].version {
            return Err(MigrationPlanError::DuplicateVersion(pair[0].version));
        }
    }
    for migration in &declared {
        if migration.version <= minimum || migration.version > target {
            return Err(MigrationPlanError::UnexpectedVersion {
                version: migration.version,
                minimum,
                target,
            });
        }
    }
    for version in (minimum + 1)..=target {
        if !declared
            .iter()
            .any(|migration| migration.version == version)
        {
            return Err(MigrationPlanError::MissingVersion(version));
        }
    }

    let Some(current) = current else {
        return Ok(MigrationRun::Initialize { target });
    };
    if current < minimum {
        return Err(MigrationPlanError::DatabaseTooOld { current, minimum });
    }
    if current > target {
        return Err(MigrationPlanError::DatabaseTooNew { current, target });
    }
    if current == target {
        return Ok(MigrationRun::UpToDate { version: current });
    }

    Ok(MigrationRun::Apply(
        declared
            .into_iter()
            .filter(|migration| migration.version > current)
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIGRATIONS: &[(i32, &str)] = &[(2, "two"), (3, "three")];

    #[test]
    fn fresh_current_and_incremental_paths_share_one_state_machine() {
        assert_eq!(
            plan_migrations(None, 3, 1, MIGRATIONS.iter().copied()).unwrap(),
            MigrationRun::Initialize { target: 3 }
        );
        assert_eq!(
            plan_migrations(Some(3), 3, 1, MIGRATIONS.iter().copied()).unwrap(),
            MigrationRun::UpToDate { version: 3 }
        );
        assert_eq!(
            plan_migrations(Some(1), 3, 1, MIGRATIONS.iter().copied()).unwrap(),
            MigrationRun::Apply(vec![
                MigrationStep {
                    version: 2,
                    name: "two".to_string(),
                },
                MigrationStep {
                    version: 3,
                    name: "three".to_string(),
                },
            ])
        );
    }

    #[test]
    fn gaps_duplicates_and_unknown_versions_are_rejected() {
        assert_eq!(
            plan_migrations(Some(1), 3, 1, [(3, "three")]),
            Err(MigrationPlanError::MissingVersion(2))
        );
        assert_eq!(
            plan_migrations(Some(1), 2, 1, [(2, "a"), (2, "b")]),
            Err(MigrationPlanError::DuplicateVersion(2))
        );
        assert!(matches!(
            plan_migrations(Some(1), 2, 1, [(3, "future")]),
            Err(MigrationPlanError::UnexpectedVersion { version: 3, .. })
        ));
    }

    #[test]
    fn incompatible_database_versions_are_never_silently_accepted() {
        assert!(matches!(
            plan_migrations(Some(0), 3, 1, MIGRATIONS.iter().copied()),
            Err(MigrationPlanError::DatabaseTooOld { .. })
        ));
        assert!(matches!(
            plan_migrations(Some(4), 3, 1, MIGRATIONS.iter().copied()),
            Err(MigrationPlanError::DatabaseTooNew { .. })
        ));
    }
}
