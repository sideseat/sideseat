//! Filter type definitions
//!
//! Defines the filter types and operators used for querying OTEL data.

use serde::Deserialize;

use crate::api::types::ApiError;
use crate::utils::sql::escape_like_pattern;

/// Filter types for advanced queries
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    Datetime {
        column: String,
        operator: DatetimeOp,
        value: String,
    },
    String {
        column: String,
        operator: StringOp,
        value: String,
    },
    Number {
        column: String,
        operator: NumberOp,
        value: f64,
    },
    StringOptions {
        column: String,
        operator: OptionsOp,
        value: Vec<String>,
    },
    Boolean {
        column: String,
        operator: BooleanOp,
        value: bool,
    },
    Null {
        column: String,
        operator: NullOp,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub enum DatetimeOp {
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "<=")]
    Lte,
}

#[derive(Debug, Clone, Deserialize)]
pub enum StringOp {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "contains")]
    Contains,
    #[serde(rename = "starts_with")]
    StartsWith,
    #[serde(rename = "ends_with")]
    EndsWith,
}

#[derive(Debug, Clone, Deserialize)]
pub enum NumberOp {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "<=")]
    Lte,
}

#[derive(Debug, Clone, Deserialize)]
pub enum OptionsOp {
    #[serde(rename = "any of")]
    AnyOf,
    #[serde(rename = "none of")]
    NoneOf,
}

#[derive(Debug, Clone, Deserialize)]
pub enum BooleanOp {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "<>")]
    Ne,
}

#[derive(Debug, Clone, Deserialize)]
pub enum NullOp {
    #[serde(rename = "is null")]
    IsNull,
    #[serde(rename = "is not null")]
    IsNotNull,
}

/// Collects SQL parameters during query building (maintains insertion order)
#[derive(Debug, Default)]
pub struct SqlParams {
    pub values: Vec<String>,
}

impl Filter {
    /// Validate filter column against whitelist
    pub fn validate(&self, allowed_columns: &[&str]) -> Result<(), ApiError> {
        let column = self.column();
        if !allowed_columns.contains(&column.as_str()) {
            return Err(ApiError::bad_request(
                "INVALID_FILTER_COLUMN",
                format!("Cannot filter by column: {}", column),
            ));
        }
        Ok(())
    }

    fn column(&self) -> &String {
        match self {
            Self::Datetime { column, .. } => column,
            Self::String { column, .. } => column,
            Self::Number { column, .. } => column,
            Self::StringOptions { column, .. } => column,
            Self::Boolean { column, .. } => column,
            Self::Null { column, .. } => column,
        }
    }

    /// Generate SQL WHERE clause fragment
    /// Returns the SQL clause with ? placeholders and updates params
    pub fn to_sql(&self, params: &mut SqlParams) -> String {
        self.to_sql_aliased(params, |col| col, "")
    }

    /// Generate SQL WHERE clause fragment with column name mapping and table alias
    /// Returns the SQL clause with ? placeholders and updates params
    ///
    /// The alias is prepended to column names (e.g., "sp" → "sp.column_name").
    /// Pass empty string for no alias.
    pub fn to_sql_aliased<'a, F>(&'a self, params: &mut SqlParams, mapper: F, alias: &str) -> String
    where
        F: Fn(&'a str) -> &'a str,
    {
        // Helper to format column with optional alias
        let format_col = |col: &str| -> String {
            if alias.is_empty() {
                col.to_string()
            } else {
                format!("{}.{}", alias, col)
            }
        };

        match self {
            Self::Datetime {
                column,
                operator,
                value,
            } => {
                let col = format_col(mapper(column));
                params.values.push(value.clone());
                let op = match operator {
                    DatetimeOp::Gt => ">",
                    DatetimeOp::Lt => "<",
                    DatetimeOp::Gte => ">=",
                    DatetimeOp::Lte => "<=",
                };
                format!("{} {} ?", col, op)
            }
            Self::String {
                column,
                operator,
                value,
            } => {
                let col = format_col(mapper(column));
                match operator {
                    StringOp::Eq => {
                        params.values.push(value.clone());
                        format!("{} = ?", col)
                    }
                    StringOp::Contains => {
                        let escaped = escape_like_pattern(value);
                        params.values.push(format!("%{}%", escaped));
                        format!("{} LIKE ? ESCAPE '\\'", col)
                    }
                    StringOp::StartsWith => {
                        let escaped = escape_like_pattern(value);
                        params.values.push(format!("{}%", escaped));
                        format!("{} LIKE ? ESCAPE '\\'", col)
                    }
                    StringOp::EndsWith => {
                        let escaped = escape_like_pattern(value);
                        params.values.push(format!("%{}", escaped));
                        format!("{} LIKE ? ESCAPE '\\'", col)
                    }
                }
            }
            Self::Number {
                column,
                operator,
                value,
            } => {
                let col = format_col(mapper(column));
                params.values.push(value.to_string());
                let op = match operator {
                    NumberOp::Eq => "=",
                    NumberOp::Gt => ">",
                    NumberOp::Lt => "<",
                    NumberOp::Gte => ">=",
                    NumberOp::Lte => "<=",
                };
                format!("{} {} ?", col, op)
            }
            Self::StringOptions {
                column,
                operator,
                value,
            } => {
                let mapped = mapper(column);
                let col = format_col(mapped);
                if value.is_empty() {
                    return "1=1".to_string();
                }

                // Use array-specific filtering for tags column
                if mapped == "tags" {
                    return super::builder::build_tags_filter(value, operator, params, alias);
                }

                let placeholders: Vec<&str> = value.iter().map(|_| "?").collect();
                params.values.extend(value.iter().cloned());

                match operator {
                    OptionsOp::AnyOf => {
                        format!("{} IN ({})", col, placeholders.join(", "))
                    }
                    OptionsOp::NoneOf => {
                        format!("{} NOT IN ({})", col, placeholders.join(", "))
                    }
                }
            }
            Self::Boolean {
                column,
                operator,
                value,
            } => {
                let col = format_col(mapper(column));
                let sql_bool = if *value { "TRUE" } else { "FALSE" };
                match operator {
                    BooleanOp::Eq => format!("{} = {}", col, sql_bool),
                    BooleanOp::Ne => format!("{} <> {}", col, sql_bool),
                }
            }
            Self::Null { column, operator } => {
                let col = format_col(mapper(column));
                match operator {
                    NullOp::IsNull => format!("{} IS NULL", col),
                    NullOp::IsNotNull => format!("{} IS NOT NULL", col),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_filter_gt() {
        let filter = Filter::Datetime {
            column: "start_time".to_string(),
            operator: DatetimeOp::Gt,
            value: "2024-01-01T00:00:00Z".to_string(),
        };
        let mut params = SqlParams::default();
        let sql = filter.to_sql(&mut params);

        assert_eq!(sql, "start_time > ?");
        assert_eq!(params.values, vec!["2024-01-01T00:00:00Z"]);
    }

    #[test]
    fn string_filter_contains() {
        let filter = Filter::String {
            column: "span_name".to_string(),
            operator: StringOp::Contains,
            value: "error".to_string(),
        };
        let mut params = SqlParams::default();
        let sql = filter.to_sql(&mut params);

        assert_eq!(sql, r"span_name LIKE ? ESCAPE '\'");
        assert_eq!(params.values, vec!["%error%"]);
    }

    #[test]
    fn number_filter_all_operators() {
        let operators = [
            (NumberOp::Eq, "="),
            (NumberOp::Gt, ">"),
            (NumberOp::Lt, "<"),
            (NumberOp::Gte, ">="),
            (NumberOp::Lte, "<="),
        ];

        for (op, expected_op) in operators {
            let filter = Filter::Number {
                column: "duration_ms".to_string(),
                operator: op,
                value: 100.5,
            };
            let mut params = SqlParams::default();
            let sql = filter.to_sql(&mut params);

            assert_eq!(sql, format!("duration_ms {} ?", expected_op));
            assert_eq!(params.values, vec!["100.5"]);
        }
    }

    #[test]
    fn string_options_any_of() {
        let filter = Filter::StringOptions {
            column: "environment".to_string(),
            operator: OptionsOp::AnyOf,
            value: vec!["prod".to_string(), "staging".to_string()],
        };
        let mut params = SqlParams::default();
        let sql = filter.to_sql(&mut params);

        assert_eq!(sql, "environment IN (?, ?)");
        assert_eq!(params.values, vec!["prod", "staging"]);
    }

    #[test]
    fn boolean_filter_eq() {
        let filter = Filter::Boolean {
            column: "is_root".to_string(),
            operator: BooleanOp::Eq,
            value: true,
        };
        let mut params = SqlParams::default();
        let sql = filter.to_sql(&mut params);

        assert_eq!(sql, "is_root = TRUE");
        assert!(params.values.is_empty());
    }

    #[test]
    fn null_filter_is_null() {
        let filter = Filter::Null {
            column: "parent_span_id".to_string(),
            operator: NullOp::IsNull,
        };
        let mut params = SqlParams::default();
        let sql = filter.to_sql(&mut params);

        assert_eq!(sql, "parent_span_id IS NULL");
        assert!(params.values.is_empty());
    }

    #[test]
    fn string_options_filter_with_alias() {
        let filter = Filter::StringOptions {
            column: "trace_id".to_string(),
            operator: OptionsOp::AnyOf,
            value: vec!["abc".to_string(), "def".to_string()],
        };
        let mut params = SqlParams::default();
        let sql = filter.to_sql_aliased(&mut params, |c| c, "sp");

        assert_eq!(sql, "sp.trace_id IN (?, ?)");
        assert_eq!(params.values, vec!["abc", "def"]);
    }

    #[test]
    fn number_filter_with_alias_and_mapper() {
        let filter = Filter::Number {
            column: "total_cost".to_string(),
            operator: NumberOp::Gte,
            value: 0.01,
        };
        let mut params = SqlParams::default();
        // Simple mapper that converts total_cost to gen_ai_cost_total
        fn mapper(col: &str) -> &str {
            if col == "total_cost" {
                "gen_ai_cost_total"
            } else {
                col
            }
        }
        let sql = filter.to_sql_aliased(&mut params, mapper, "g");

        assert_eq!(sql, "g.gen_ai_cost_total >= ?");
        assert_eq!(params.values, vec!["0.01"]);
    }
}
