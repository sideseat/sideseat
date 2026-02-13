//! Query filter system
//!
//! Provides reusable filter types and SQL generation for OTEL queries.
//! Filters support datetime, string, number, options, boolean, and null operations.
//!
//! ## Usage
//!
//! ```no_run
//! use sideseat_server::data::duckdb::filters::{Filter, parse_filters, columns, SqlParams};
//!
//! let json_str = r#"[{"field": "status", "op": "eq", "value": "ok"}]"#;
//! let filters = parse_filters(json_str, columns::TRACE_FILTERABLE).unwrap();
//! let mut params = SqlParams::default();
//! for filter in &filters {
//!     let sql = filter.to_sql(&mut params);
//! }
//! ```

mod builder;
mod parser;
mod types;

pub use builder::{build_tags_filter, columns};
pub use parser::parse_filters;
pub use types::{BooleanOp, DatetimeOp, Filter, NullOp, NumberOp, OptionsOp, SqlParams, StringOp};
