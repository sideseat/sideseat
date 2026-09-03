//! Filter type definitions
//!
//! Defines the filter types and operators used for querying OTEL data.

use serde::Deserialize;

use crate::api::types::ApiError;
use crate::utils::sql::{escape_like_pattern, is_plain_identifier};

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

    /// Whether this filter states nothing, so it must contribute no condition at all.
    ///
    /// An empty option list is "the user has not chosen a value", not "match nothing" - which is why the
    /// renderers answer `1=1` for it. But a condition of `1=1` is only neutral where it sits in the query's
    /// own WHERE: wrapped in a subquery over a *narrower* relation it silently becomes that relation's
    /// membership test. `session_id any of []` became `trace_id IN (traces that have a session)`, so every
    /// sessionless trace vanished from a list the user had not filtered. Skipping the filter outright is the
    /// only form that is neutral wherever it is used.
    pub fn is_vacuous(&self) -> bool {
        match self {
            Self::StringOptions { value, .. } => value.is_empty(),
            _ => false,
        }
    }

    /// The column this filter names, before any view-to-span mapping.
    pub fn column(&self) -> &String {
        match self {
            Self::Datetime { column, .. } => column,
            Self::String { column, .. } => column,
            Self::Number { column, .. } => column,
            Self::StringOptions { column, .. } => column,
            Self::Boolean { column, .. } => column,
            Self::Null { column, .. } => column,
        }
    }

    /// The positive form of a negated filter, for callers that express "none" by excluding the
    /// matches of "any".
    ///
    /// A filter on a span column is asked of a whole entity: a trace has many spans, so "not this
    /// model" has to mean *no* span used it. Rendered as written, inside a `trace_id IN (...)`
    /// subquery, it meant "some span was something else" - so a trace that used the excluded model
    /// in one call and another model in the next came back from the filter that excluded it. The
    /// caller renders this twin and negates the subquery instead.
    ///
    /// `None` for an operator that is already positive.
    pub fn positive_twin(&self) -> Option<Filter> {
        match self {
            // An empty list is not a filter at all. Its twin renders as `1 = 1`, and negating the
            // subquery around that excluded every row - "none of nothing" returned nothing instead
            // of everything.
            Self::StringOptions {
                column,
                operator: OptionsOp::NoneOf,
                value,
            } if !value.is_empty() => Some(Self::StringOptions {
                column: column.clone(),
                operator: OptionsOp::AnyOf,
                value: value.clone(),
            }),
            Self::Null {
                column,
                operator: NullOp::IsNull,
            } => Some(Self::Null {
                column: column.clone(),
                operator: NullOp::IsNotNull,
            }),
            Self::Boolean {
                column,
                operator: BooleanOp::Ne,
                value,
            } => Some(Self::Boolean {
                column: column.clone(),
                operator: BooleanOp::Eq,
                value: *value,
            }),
            _ => None,
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
    /// Render this filter against an arbitrary SQL expression instead of a column.
    ///
    /// For a value that is computed rather than stored - the trace name a list row displays is an
    /// aggregate over the trace's spans - so the filter can be evaluated where that value exists,
    /// in a HAVING clause. The expression comes from this crate, never from a request, which is why
    /// the bare-identifier guard does not apply to it.
    pub fn to_sql_against(&self, params: &mut SqlParams, expression: &str) -> String {
        match self {
            Self::String {
                operator, value, ..
            } => match operator {
                StringOp::Eq => {
                    params.values.push(value.clone());
                    format!("{expression} = ?")
                }
                StringOp::Contains => {
                    params
                        .values
                        .push(format!("%{}%", escape_like_pattern(value)));
                    format!("{expression} LIKE ? ESCAPE '\\'")
                }
                StringOp::StartsWith => {
                    params
                        .values
                        .push(format!("{}%", escape_like_pattern(value)));
                    format!("{expression} LIKE ? ESCAPE '\\'")
                }
                StringOp::EndsWith => {
                    params
                        .values
                        .push(format!("%{}", escape_like_pattern(value)));
                    format!("{expression} LIKE ? ESCAPE '\\'")
                }
            },
            Self::StringOptions {
                operator, value, ..
            } => {
                if value.is_empty() {
                    return "1=1".to_string();
                }
                let placeholders: Vec<&str> = value.iter().map(|_| "?").collect();
                params.values.extend(value.iter().cloned());
                match operator {
                    OptionsOp::AnyOf => format!("{expression} IN ({})", placeholders.join(", ")),
                    OptionsOp::NoneOf => {
                        format!("{expression} NOT IN ({})", placeholders.join(", "))
                    }
                }
            }
            Self::Null { operator, .. } => match operator {
                NullOp::IsNull => format!("{expression} IS NULL"),
                NullOp::IsNotNull => format!("{expression} IS NOT NULL"),
            },
            // Numbers and timestamps, because most of what a trace row displays is an aggregate:
            // its tokens and cost are sums over its spans and its duration spans all of them. A
            // filter on those has to be evaluated where the aggregate exists, so this renderer
            // carries every operator the trace filter bar offers, not only the string ones a name
            // needs.
            Self::Number {
                operator, value, ..
            } => {
                params.values.push(value.to_string());
                let op = match operator {
                    NumberOp::Eq => "=",
                    NumberOp::Gt => ">",
                    NumberOp::Lt => "<",
                    NumberOp::Gte => ">=",
                    NumberOp::Lte => "<=",
                };
                format!("{expression} {op} ?")
            }
            Self::Datetime {
                operator, value, ..
            } => {
                params.values.push(value.clone());
                let op = match operator {
                    DatetimeOp::Gt => ">",
                    DatetimeOp::Lt => "<",
                    DatetimeOp::Gte => ">=",
                    DatetimeOp::Lte => "<=",
                };
                format!("{expression} {op} ?")
            }
            Self::Boolean {
                operator, value, ..
            } => {
                let literal = if *value { "true" } else { "false" };
                match operator {
                    BooleanOp::Eq => format!("{expression} = {literal}"),
                    BooleanOp::Ne => format!("{expression} != {literal}"),
                }
            }
        }
    }

    pub fn to_sql_aliased<'a, F>(&'a self, params: &mut SqlParams, mapper: F, alias: &str) -> String
    where
        F: Fn(&'a str) -> &'a str,
    {
        // See the same guard in the ClickHouse renderer: the allowlist runs when the request is
        // parsed, and this is the check where the SQL is assembled, because a mapper passes an
        // unknown column straight through. Matching nothing is the safe failure.
        if !is_plain_identifier(self.column()) {
            return "1 = 0".to_string();
        }

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

    /// The mirror of the ClickHouse renderer's guard: an unmapped column reaches the SQL text, so
    /// anything that is not a bare identifier must produce a condition that matches nothing.
    #[test]
    fn a_hostile_column_name_matches_nothing() {
        for hostile in [
            "trace_id; DROP TABLE otel_spans",
            "trace_id' OR '1'='1",
            "count(*)",
            "",
        ] {
            let filter = Filter::String {
                column: hostile.to_string(),
                operator: StringOp::Eq,
                value: "x".to_string(),
            };
            let mut params = SqlParams::default();
            let sql = filter.to_sql_aliased(&mut params, |c| c, "");
            assert_eq!(sql, "1 = 0", "{hostile:?} produced SQL: {sql}");
            assert!(params.values.is_empty(), "{hostile:?} bound a parameter");
        }
    }

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
