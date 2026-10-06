//! Equivalence tests for [`CleanupError`](super::CleanupError)'s wording.

use super::*;

/// Every message `anyhow` used to format, now rendered by the variant that replaced it.
///
/// These are the sentences an operator reads: the API answers a failed deletion with
/// `error.to_string()` in the response body, and the sweep logs it as a `tracing` field. Each one
/// also now carries the store's own words, which `anyhow`'s `{}` dropped.
#[test]
fn every_variant_says_what_it_said_before_plus_the_cause() {
    let cause = || DataError::Conflict("busy".to_string());
    let project_id = ProjectId::from("p-1");

    let cases: Vec<(CleanupError, &str)> = vec![
        (
            CleanupError::ClaimOrganization { source: cause() },
            "Failed to claim and journal the organization deletion: Conflict: busy",
        ),
        (
            CleanupError::ListProjects { source: cause() },
            "Failed to list an organization's projects: Conflict: busy",
        ),
        (
            CleanupError::FenceProject {
                project_id: project_id.clone(),
                org_id: "o-1".to_string(),
                source: cause(),
            },
            "Failed to fence and journal project p-1 of organization o-1: Conflict: busy",
        ),
        (
            CleanupError::CountRemainingProjects { source: cause() },
            "Failed to count an organization's remaining projects: Conflict: busy",
        ),
        (
            CleanupError::DeleteOrganizationRow { source: cause() },
            "Failed to delete organization row: Conflict: busy",
        ),
        (
            CleanupError::ClaimProject { source: cause() },
            "Failed to claim and journal the project deletion: Conflict: busy",
        ),
        (
            CleanupError::RecordProjectSweep { source: cause() },
            "Failed to record a project cleanup sweep: Conflict: busy",
        ),
        (
            CleanupError::ListTombstonedProjects { source: cause() },
            "Failed to look for tombstoned projects: Conflict: busy",
        ),
        (
            CleanupError::ListTombstonedOrganizations { source: cause() },
            "Failed to look for tombstoned organizations: Conflict: busy",
        ),
        (
            CleanupError::OrganizationIncomplete {
                org_id: "o-1".to_string(),
                details: "project p-1: boom".to_string(),
            },
            "Organization o-1 cleanup is not finished; it stays fenced for retry: \
             project p-1: boom",
        ),
        (
            CleanupError::ProjectIncomplete {
                project_id,
                failures: 2,
                details: "Analytics delete failed: boom; File delete failed: boom".to_string(),
            },
            "Project p-1 cleanup failed with 2 errors, leaving it claimed for retry: \
             Analytics delete failed: boom; File delete failed: boom",
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

/// The store's error stays reachable through the chain, which `anyhow`'s `.context` also gave.
#[test]
fn a_store_failure_keeps_its_cause_in_the_chain() {
    let error = CleanupError::ClaimProject {
        source: DataError::Conflict("busy".to_string()),
    };

    assert!(matches!(
        std::error::Error::source(&error).and_then(|s| s.downcast_ref::<DataError>()),
        Some(DataError::Conflict(_))
    ));
}
