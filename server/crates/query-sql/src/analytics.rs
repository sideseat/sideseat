//! Typed statements shared by the analytical adapters.
//!
//! The public constructors describe operations in storage-neutral terms. Rendering is the only
//! place where DuckDB's window deduplication and ClickHouse's `FINAL` relation are selected.

use crate::Backend;
use sideseat_core::constants::{QUERY_MAX_FILTER_SUGGESTIONS, QUERY_MAX_SPANS_PER_TRACE};
use sideseat_core::utils::sql::{escape_like_pattern, is_plain_identifier};
use sideseat_ports::filters::{
    BooleanOp, DatetimeOp, Filter, NullOp, NumberOp, OptionsOp, StringOp, columns,
};
use sideseat_ports::types::{
    FeedSpansParams, ListSessionsParams, ListSpansParams, ListTracesParams, OrderDirection,
    SESSION_FILTER_OPTION_COLUMNS, SPAN_FILTER_OPTION_COLUMNS, TRACE_FILTER_OPTION_COLUMNS,
};

mod membership;
mod operation;
mod render;
mod sessions;
mod spans;
mod traces;

pub use membership::*;
pub use operation::*;
pub use sessions::*;
pub use spans::*;

/// Driver-neutral parameter values. Adapters only translate these values into their driver's
/// binding API; they do not decide SQL shape or parameter order.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryValue {
    String(String),
    Int64(i64),
    Float64(f64),
}

/// The semantic value occupying a placeholder, in statement order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    ProjectId,
    TraceId,
    SpanId,
}

/// SQL lowered from a typed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedQuery {
    sql: String,
    bindings: Vec<Binding>,
}

/// One executable statement with its ordered values.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterizedQuery {
    sql: String,
    params: Vec<QueryValue>,
}

impl ParameterizedQuery {
    pub(crate) fn new(sql: String, params: Vec<QueryValue>) -> Self {
        Self { sql, params }
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn params(&self) -> &[QueryValue] {
        &self.params
    }
}

/// Count and row statements for an offset-paginated read.
#[derive(Debug, Clone, PartialEq)]
pub struct PageQuery {
    pub count: ParameterizedQuery,
    pub rows: ParameterizedQuery,
}

/// One validated filter-option column and its executable statement.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOptionQuery {
    pub column: String,
    pub query: ParameterizedQuery,
}

/// Statements needed to verify that one project's analytical data is gone.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRowCountPlan {
    pub spans: ParameterizedQuery,
    pub metrics_table_exists: Option<ParameterizedQuery>,
    pub metrics: ParameterizedQuery,
    pub logs: ParameterizedQuery,
}

/// Per-signal logical-byte totals for one project.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectLogicalBytesPlan {
    pub spans: ParameterizedQuery,
    pub metrics: ParameterizedQuery,
    pub logs: ParameterizedQuery,
}

impl RenderedQuery {
    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Projection {
    SpanDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    Spans,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Project,
    Trace,
    Span,
}

impl Column {
    const fn name(self) -> &'static str {
        match self {
            Self::Project => "project_id",
            Self::Trace => "trace_id",
            Self::Span => "span_id",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Equality {
    column: Column,
    binding: Binding,
}

/// A typed `SELECT` statement. Its fields are deliberately closed: callers choose an operation,
/// not fragments of SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectStatement {
    operation: QueryOperation,
    projection: Projection,
    relation: Relation,
    predicates: Vec<Equality>,
    limit: Option<u32>,
}

#[cfg(test)]
#[path = "analytics_tests.rs"]
mod tests;
