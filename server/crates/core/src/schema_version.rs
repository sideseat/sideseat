//! One schema version per store, created from scratch and never upgraded.
//!
//! Every backend - SQLite, DuckDB, PostgreSQL, ClickHouse - is created at its current schema version in one
//! step. A store at any other version is refused at startup rather than migrated: SideSeat keeps no backward
//! compatibility for stored data, so there is no upgrade path to get wrong, and nothing is ever deleted
//! without the operator asking for it.

use std::fmt;

/// What opening a store found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaCheck {
    /// No schema yet: create the current one.
    Create,
    /// Already at the supported version.
    Current,
}

/// A store whose schema this build does not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedSchema {
    pub found: i32,
    pub supported: i32,
}

impl fmt::Display for UnsupportedSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the store is at schema version {found}, and this build reads and writes only version \
             {supported}; it does not upgrade other versions. To start fresh, remove the store explicitly - \
             `sideseat system prune` deletes the embedded data directory, and for PostgreSQL or ClickHouse \
             drop the configured database. Nothing is deleted automatically.",
            found = self.found,
            supported = self.supported,
        )
    }
}

impl std::error::Error for UnsupportedSchema {}

/// A store at the supported version whose tables, columns or indexes are not the ones this build creates: an
/// earlier layout of the same version, which this build can neither read nor write correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutMismatch {
    pub version: i32,
    /// The first differences, `missing ...` or `unexpected ...`.
    pub differences: Vec<String>,
}

impl fmt::Display for LayoutMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the store is at schema version {version} but was created with an earlier layout of it ({}); this \
             build does not upgrade stores. To start fresh, remove the store explicitly - `sideseat system prune` \
             deletes the embedded data directory. Nothing is deleted automatically.",
            self.differences.join("; "),
            version = self.version,
        )
    }
}

impl std::error::Error for LayoutMismatch {}

/// Decide what to do with a store whose recorded version is `found` (`None` when it has no schema).
pub fn check(found: Option<i32>, supported: i32) -> Result<SchemaCheck, UnsupportedSchema> {
    match found {
        None => Ok(SchemaCheck::Create),
        Some(version) if version == supported => Ok(SchemaCheck::Current),
        Some(version) => Err(UnsupportedSchema {
            found: version,
            supported,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_is_created_kept_or_refused_never_upgraded() {
        assert_eq!(check(None, 2), Ok(SchemaCheck::Create));
        assert_eq!(check(Some(2), 2), Ok(SchemaCheck::Current));
        let refused = check(Some(1), 2).unwrap_err();
        assert_eq!(
            refused,
            UnsupportedSchema {
                found: 1,
                supported: 2
            }
        );
        let message = refused.to_string();
        assert!(message.contains("version 1") && message.contains("only version 2"));
        assert!(message.contains("sideseat system prune"));
        assert!(check(Some(3), 2).is_err());
    }
}
