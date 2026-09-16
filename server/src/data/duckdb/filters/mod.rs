//! DuckDB's filter rendering.
//!
//! The vocabulary - `Filter`, its operators, the column allowlists and the parser - lives in
//! `crate::data::filters`, shared by both analytics adapters. What is here is this dialect's SQL.

mod builder;
mod sql;

pub use builder::build_tags_filter;
pub use sql::{FilterSql, SqlParams};
