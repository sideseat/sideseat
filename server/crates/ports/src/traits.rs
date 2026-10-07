//! Repository traits for database backends
//!
//! This module defines traits that provide a unified interface for database operations
//! across multiple backends. Each backend (DuckDB, ClickHouse, SQLite, PostgreSQL)
//! implements these traits with its own specific logic.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

use crate::error::DataError;
use crate::types::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, CredentialPermissionRow,
    CredentialRow, FeedMessagesParams, FeedSpansParams, FileRow, LastOwnerResult, ListLogsParams,
    ListMetricsParams, ListSessionsParams, ListSpansParams, ListTracesParams, LogRow,
    MemberWithUser, MembershipRow, MessageQueryParams, MessageQueryResult, MetricAggregateRow,
    MetricRow, NormalizedLog, NormalizedMetric, NormalizedSpan, OrgWithRole, OrganizationRow,
    PressureSpanCandidate, ProjectHold, ProjectId, ProjectRow, ProjectStorageUsage, RawPending,
    RawRecordRow, SearchBackfillDocument, SearchBackfillSource, SearchCursor, SearchPage,
    SearchQuery, SearchSignal, SessionRow, SpanCounts, SpanRow, StagedPayload, StagedSequenceState,
    TraceRow, UserRow,
};

mod analytics;
mod governance;
mod transactional;

pub use analytics::*;
pub use governance::*;
pub use transactional::*;

/// Everything an analytics adapter provides, as one bound.
///
/// **A bundle of the narrow ports, not a trait with its own methods.** It was a single 31-method trait, which
/// meant a caller needing spans depended on session statistics and on every delete - and no consumer could state
/// what it actually used. The narrow ports are the seams; this exists so the composition root can hand out one
/// object, and so `dyn AnalyticsRepository` upcasts to whichever port a caller wants.
///
/// The blanket impl is what keeps it a bundle: implementing the parts *is* implementing this, so no adapter
/// writes an empty impl and nothing can drift between the two.
#[async_trait]
pub trait AnalyticsRepository:
    SpanStore
    + MetricStore
    + LogStore
    + SearchIndex
    + EntityQuery
    + MessageStore
    + AnalyticsMaintenance
    + SurvivorReferences
    + RawStore
{
}

impl<T> AnalyticsRepository for T where
    T: SpanStore
        + MetricStore
        + LogStore
        + SearchIndex
        + EntityQuery
        + MessageStore
        + AnalyticsMaintenance
        + SurvivorReferences
        + RawStore
{
}

/// Everything a transactional adapter provides, as one bound. See [`AnalyticsRepository`] for why this is a
/// bundle rather than a trait of its own: it was 95 methods, and the seven cohesive ports above are what a
/// caller can actually name.
pub trait TransactionalRepository:
    IdentityStore
    + ProjectStore
    + FileMetaStore
    + ApiKeyStore
    + CredentialStore
    + FavoriteStore
    + DeletionJournal
    + StagedPayloadStore
{
}

impl<T> TransactionalRepository for T where
    T: IdentityStore
        + ProjectStore
        + FileMetaStore
        + ApiKeyStore
        + CredentialStore
        + FavoriteStore
        + DeletionJournal
        + StagedPayloadStore
{
}
