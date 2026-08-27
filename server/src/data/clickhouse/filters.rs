//! Advanced filters in ClickHouse dialect.
//!
//! The list endpoints accept a `filters` array - the UI's filter bar - and the ClickHouse backend
//! ignored it completely: `params.filters` appeared nowhere in its query module, so filtering a
//! trace list by model, token count, cost or error status returned the whole list. The user
//! filtered and got everything back, with nothing to indicate the filter had been dropped.
//!
//! The [`Filter`] type and its DuckDB rendering live under `data/duckdb/filters`. This is the same
//! filter in the other dialect, and it has to mean the same thing, which is not achieved by
//! copying the SQL:
//!
//! - values must be bound with their own types. DuckDB's renderer pushes every value as a string,
//!   which its driver coerces; ClickHouse compares a string literal against a numeric column and
//!   raises "illegal types of arguments" instead.
//! - `LIKE ? ESCAPE '\'` is not accepted. ClickHouse's LIKE always treats backslash as the escape
//!   character, so the clause is dropped and the escaping is unchanged.
//! - a timestamp is bound as microseconds through `fromUnixTimestamp64Micro`, as everywhere else
//!   in this backend, rather than as an RFC 3339 string.
//! - cost columns are `Decimal(18, 6)`, which compares to a bound float only after `toFloat64`.
//!
//! Filters apply to the span rows a list query already scans, at the same point in the WHERE as
//! DuckDB applies them, so a filter on a trace-level column means "some span of this trace
//! matches" on both backends. The parity test asserts that equivalence rather than assuming it.

use crate::data::duckdb::filters::{
    BooleanOp, DatetimeOp, Filter, NullOp, NumberOp, OptionsOp, StringOp,
};
use crate::utils::sql::escape_like_pattern;

use super::repositories::query::QueryParam;

/// Columns holding a `Decimal(18, 6)` cost, which needs `toFloat64` before comparison.
const DECIMAL_COLUMNS: &[&str] = &[
    "gen_ai_cost_input",
    "gen_ai_cost_output",
    "gen_ai_cost_cache_read",
    "gen_ai_cost_cache_write",
    "gen_ai_cost_reasoning",
    "gen_ai_cost_total",
];

/// Render one filter as a ClickHouse condition, appending its bind parameters.
///
/// `mapper` translates a view column name to the underlying span column, exactly as the DuckDB
/// renderer does, so both backends filter on the same column. `alias` qualifies the column when
/// the query names its table.
pub(super) fn to_clickhouse_sql<'a, F>(
    filter: &'a Filter,
    params: &mut Vec<QueryParam>,
    mapper: F,
    alias: &str,
) -> String
where
    F: Fn(&'a str) -> &'a str,
{
    let qualify = |column: &str| -> String {
        if alias.is_empty() {
            column.to_string()
        } else {
            format!("{alias}.{column}")
        }
    };
    // A Decimal column compares to a bound float only after conversion; a Nullable one yields
    // NULL for a missing value, which fails the comparison, which is the intent.
    let numeric = |column: &str| -> String {
        let qualified = qualify(column);
        if DECIMAL_COLUMNS.contains(&column) {
            format!("toFloat64({qualified})")
        } else {
            qualified
        }
    };

    match filter {
        Filter::Datetime {
            column,
            operator,
            value,
        } => {
            let column = qualify(mapper(column));
            let op = match operator {
                DatetimeOp::Gt => ">",
                DatetimeOp::Lt => "<",
                DatetimeOp::Gte => ">=",
                DatetimeOp::Lte => "<=",
            };
            // An unparseable timestamp must exclude everything rather than match everything: a
            // malformed filter that widens the result set is worse than one that returns nothing,
            // because it looks like data.
            match chrono::DateTime::parse_from_rfc3339(value) {
                Ok(parsed) => {
                    params.push(QueryParam::Int64(parsed.timestamp_micros()));
                    format!("{column} {op} fromUnixTimestamp64Micro(?)")
                }
                Err(_) => "1 = 0".to_string(),
            }
        }
        Filter::String {
            column,
            operator,
            value,
        } => {
            let column = qualify(mapper(column));
            let (pattern, sql) = match operator {
                StringOp::Eq => (value.clone(), format!("{column} = ?")),
                StringOp::Contains => (
                    format!("%{}%", escape_like_pattern(value)),
                    format!("{column} LIKE ?"),
                ),
                StringOp::StartsWith => (
                    format!("{}%", escape_like_pattern(value)),
                    format!("{column} LIKE ?"),
                ),
                StringOp::EndsWith => (
                    format!("%{}", escape_like_pattern(value)),
                    format!("{column} LIKE ?"),
                ),
            };
            params.push(QueryParam::String(pattern));
            sql
        }
        Filter::Number {
            column,
            operator,
            value,
        } => {
            let column = numeric(mapper(column));
            let op = match operator {
                NumberOp::Eq => "=",
                NumberOp::Gt => ">",
                NumberOp::Lt => "<",
                NumberOp::Gte => ">=",
                NumberOp::Lte => "<=",
            };
            params.push(QueryParam::Float64(*value));
            format!("{column} {op} ?")
        }
        Filter::StringOptions {
            column,
            operator,
            value,
        } => {
            let mapped = mapper(column);
            if value.is_empty() {
                return "1 = 1".to_string();
            }
            if mapped == "tags" {
                return tags_condition(value, operator, params, alias);
            }
            let column = qualify(mapped);
            let placeholders: Vec<&str> = value.iter().map(|_| "?").collect();
            params.extend(value.iter().cloned().map(QueryParam::String));
            match operator {
                OptionsOp::AnyOf => format!("{column} IN ({})", placeholders.join(", ")),
                // NOT IN is false for a NULL column in both dialects, so a row with no value is
                // excluded by "none of" - matching DuckDB rather than being more helpful here.
                OptionsOp::NoneOf => format!("{column} NOT IN ({})", placeholders.join(", ")),
            }
        }
        Filter::Boolean {
            column,
            operator,
            value,
        } => {
            let column = qualify(mapper(column));
            let literal = if *value { "true" } else { "false" };
            match operator {
                BooleanOp::Eq => format!("{column} = {literal}"),
                BooleanOp::Ne => format!("{column} != {literal}"),
            }
        }
        Filter::Null { column, operator } => {
            let column = qualify(mapper(column));
            match operator {
                NullOp::IsNull => format!("{column} IS NULL"),
                NullOp::IsNotNull => format!("{column} IS NOT NULL"),
            }
        }
    }
}

/// Tags are stored as a JSON array string, so membership is an array operation rather than a
/// comparison. `hasAny` mirrors DuckDB's list overlap; ifNull keeps a row with no tags out of
/// "any of" and in "none of", which is what an absent tag means.
fn tags_condition(
    values: &[String],
    operator: &OptionsOp,
    params: &mut Vec<QueryParam>,
    alias: &str,
) -> String {
    let column = if alias.is_empty() {
        "tags".to_string()
    } else {
        format!("{alias}.tags")
    };
    let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
    params.extend(values.iter().cloned().map(QueryParam::String));
    let extracted = format!("JSONExtract(ifNull({column}, '[]'), 'Array(String)')");
    let condition = format!("hasAny({extracted}, [{}])", placeholders.join(", "));
    match operator {
        OptionsOp::AnyOf => condition,
        OptionsOp::NoneOf => format!("NOT {condition}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::duckdb::filters::columns;

    fn render(filter: &Filter) -> (String, Vec<QueryParam>) {
        let mut params = Vec::new();
        let sql = to_clickhouse_sql(filter, &mut params, columns::map_trace_column_to_spans, "s");
        (sql, params)
    }

    fn describe(params: &[QueryParam]) -> Vec<String> {
        params
            .iter()
            .map(|p| match p {
                QueryParam::String(s) => format!("str:{s}"),
                QueryParam::Int64(i) => format!("i64:{i}"),
                QueryParam::Float64(f) => format!("f64:{f}"),
            })
            .collect()
    }

    #[test]
    fn a_number_binds_as_a_number() {
        let (sql, params) = render(&Filter::Number {
            column: "total_tokens".to_string(),
            operator: NumberOp::Gt,
            value: 100.0,
        });
        assert_eq!(sql, "s.gen_ai_usage_total_tokens > ?");
        assert_eq!(
            describe(&params),
            vec!["f64:100"],
            "a numeric column compared against a bound string raises in ClickHouse"
        );
    }

    #[test]
    fn a_cost_column_is_converted_before_comparison() {
        let (sql, _) = render(&Filter::Number {
            column: "total_cost".to_string(),
            operator: NumberOp::Lt,
            value: 0.5,
        });
        assert_eq!(
            sql, "toFloat64(s.gen_ai_cost_total) < ?",
            "Decimal(18, 6) does not compare to a float directly"
        );
    }

    #[test]
    fn like_carries_no_escape_clause() {
        let (sql, params) = render(&Filter::String {
            column: "trace_name".to_string(),
            operator: StringOp::Contains,
            value: "a_b%c".to_string(),
        });
        assert_eq!(
            sql, "s.span_name LIKE ?",
            "ClickHouse rejects an ESCAPE clause; backslash is already its escape character"
        );
        // The wildcards in the user's value stay escaped, so they match literally.
        assert_eq!(describe(&params), vec![r"str:%a\_b\%c%"]);
    }

    #[test]
    fn a_timestamp_binds_as_microseconds() {
        let (sql, params) = render(&Filter::Datetime {
            column: "start_time".to_string(),
            operator: DatetimeOp::Gte,
            value: "2025-06-15T12:00:00Z".to_string(),
        });
        assert_eq!(sql, "s.timestamp_start >= fromUnixTimestamp64Micro(?)");
        assert_eq!(describe(&params), vec!["i64:1749988800000000"]);
    }

    #[test]
    fn an_unparseable_timestamp_matches_nothing() {
        let (sql, params) = render(&Filter::Datetime {
            column: "start_time".to_string(),
            operator: DatetimeOp::Gte,
            value: "not a timestamp".to_string(),
        });
        assert_eq!(
            sql, "1 = 0",
            "a malformed filter must not widen the result set into looking like data"
        );
        assert!(params.is_empty(), "a dropped condition must bind nothing");
    }

    #[test]
    fn options_and_tags_differ() {
        let (sql, params) = render(&Filter::StringOptions {
            column: "environment".to_string(),
            operator: OptionsOp::AnyOf,
            value: vec!["prod".to_string(), "staging".to_string()],
        });
        assert_eq!(sql, "s.environment IN (?, ?)");
        assert_eq!(describe(&params), vec!["str:prod", "str:staging"]);

        let (sql, params) = render(&Filter::StringOptions {
            column: "tags".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["alpha".to_string()],
        });
        assert_eq!(
            sql, "NOT hasAny(JSONExtract(ifNull(s.tags, '[]'), 'Array(String)'), [?])",
            "tags are a JSON array, so membership is an array operation"
        );
        assert_eq!(describe(&params), vec!["str:alpha"]);

        // An empty option list is not a filter at all, and must not exclude everything.
        let (sql, params) = render(&Filter::StringOptions {
            column: "environment".to_string(),
            operator: OptionsOp::AnyOf,
            value: vec![],
        });
        assert_eq!(sql, "1 = 1");
        assert!(params.is_empty());
    }

    #[test]
    fn booleans_and_nulls_need_no_parameters() {
        let (sql, params) = render(&Filter::Boolean {
            column: "has_error".to_string(),
            operator: BooleanOp::Ne,
            value: true,
        });
        assert_eq!(sql, "s.has_error != true");
        assert!(params.is_empty());

        let (sql, _) = render(&Filter::Null {
            column: "session_id".to_string(),
            operator: NullOp::IsNotNull,
        });
        assert_eq!(sql, "s.session_id IS NOT NULL");
    }
}
