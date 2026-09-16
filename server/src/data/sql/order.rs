//! Rendering an [`OrderBy`] as SQL.
//!
//! Split from the vocabulary for the same reason `Filter`'s rendering was: a column and a direction are what a
//! caller asks for, and `"col DESC"` is how a store answers. The port keeps the first and this keeps the second.
//!
//! An extension trait, because the orphan rule forbids splitting an inherent `impl` across crates - and because
//! rendering belongs beside the other dialect SQL, where step 5's query builder will absorb it.

use sideseat_ports::types::{OrderBy, OrderDirection};

/// The SQL keyword for a direction.
///
/// `keyword` rather than `as_sql`: clippy reserves `as_*` for methods taking `&self`, and this is a `Copy` enum
/// where by-value is the natural receiver.
trait DirectionSql {
    fn keyword(self) -> &'static str;
}

impl DirectionSql for OrderDirection {
    fn keyword(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// Rendering an order clause.
pub trait OrderSql {
    /// `column DIRECTION`.
    fn to_sql(&self) -> String;

    /// The same, with the column mapped - an API alias to a stored column name.
    fn to_sql_mapped<F>(&self, mapper: F) -> String
    where
        F: Fn(&str) -> &str;
}

impl OrderSql for OrderBy {
    fn to_sql(&self) -> String {
        format!("{} {}", self.column, self.direction.keyword())
    }

    /// Generate SQL with column name mapping (e.g., API aliases to DB columns)
    fn to_sql_mapped<F>(&self, mapper: F) -> String
    where
        F: Fn(&str) -> &str,
    {
        format!("{} {}", mapper(&self.column), self.direction.keyword())
    }
}
