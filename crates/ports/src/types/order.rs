//! Result ordering, as the storage layer expresses it.
//!
//! `OrderBy` lived in `api::types` while three analytics DTOs used it as a field type, which made the
//! storage layer depend on the HTTP layer for a column name and a direction. That is the wrong way round
//! and it is what stops these modules moving into a crate that cannot see `api` at all.
//!
//! The split follows what each half actually is: a column plus a direction, and the SQL they render, are
//! storage concerns and live here. Turning a `?order_by=name:asc` **query parameter** into one - with the
//! allowlist check and the 400 that a bad value earns - is an HTTP concern and stays in `api::types`
//! (`parse_order_by`).

use serde::Serialize;
use utoipa::ToSchema;

/// A column and the direction to sort it in.
#[derive(Debug, Clone)]
pub struct OrderBy {
    pub column: String,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone, Copy, Default, Serialize, ToSchema)]
pub enum OrderDirection {
    #[default]
    Desc,
    Asc,
}

impl OrderDirection {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

impl OrderBy {
    /// Build one directly, for a caller that is not parsing a query parameter.
    pub fn new(column: impl Into<String>, direction: OrderDirection) -> Self {
        Self {
            column: column.into(),
            direction,
        }
    }

    pub fn to_sql(&self) -> String {
        format!("{} {}", self.column, self.direction.as_sql())
    }

    /// Generate SQL with column name mapping (e.g., API aliases to DB columns)
    pub fn to_sql_mapped<F>(&self, mapper: F) -> String
    where
        F: Fn(&str) -> &str,
    {
        format!("{} {}", mapper(&self.column), self.direction.as_sql())
    }
}
