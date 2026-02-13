//! SQL type wrappers for DuckDB
//!
//! Shared type wrappers for converting Rust types to DuckDB-compatible SQL values.

use chrono::{DateTime, Utc};
use duckdb::ToSql;
use duckdb::types::{ToSqlOutput, Value, ValueRef};

/// Wrapper for Vec<String> to serialize as JSON array for DuckDB VARCHAR columns
pub struct SqlVec<'a>(pub &'a Vec<String>);

impl ToSql for SqlVec<'_> {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        let json = serde_json::to_string(self.0).unwrap_or_else(|_| "[]".to_string());
        Ok(ToSqlOutput::Owned(Value::Text(json)))
    }
}

/// Wrapper for DateTime<Utc> to implement ToSql for DuckDB TIMESTAMP
pub struct SqlTimestamp(pub DateTime<Utc>);

impl ToSql for SqlTimestamp {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        let ts = self.0.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        Ok(ToSqlOutput::Owned(Value::Text(ts)))
    }
}

/// Wrapper for optional DateTime<Utc>
pub struct SqlOptTimestamp(pub Option<DateTime<Utc>>);

impl ToSql for SqlOptTimestamp {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        match &self.0 {
            Some(dt) => {
                let ts = dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
                Ok(ToSqlOutput::Owned(Value::Text(ts)))
            }
            None => Ok(ToSqlOutput::Borrowed(ValueRef::Null)),
        }
    }
}
