//! SQL filter builder.
//!
//! Builds SQL WHERE clauses from Filter structs.
//! Includes column whitelists and mapping functions.

use super::sql::SqlParams;
use sideseat_ports::filters::OptionsOp;

/// Build a tags filter.
///
/// `tags` is a VARCHAR holding a JSON array, not a DuckDB list, so it has to be parsed before it
/// can be searched - `array_contains(tags, ?)` raised "No function matches the given name and
/// argument types 'array_contains(VARCHAR, UNKNOWN)'" and the filter never worked at all. The
/// parse mirrors the tag-options query, which reads the same column the same way.
/// The alias parameter is prepended to the column name (e.g., "sp" → "sp.tags").
pub fn build_tags_filter(
    tags: &[String],
    operator: &OptionsOp,
    params: &mut SqlParams,
    alias: &str,
) -> String {
    if tags.is_empty() {
        return "1=1".to_string();
    }

    let col = if alias.is_empty() {
        "tags".to_string()
    } else {
        format!("{}.tags", alias)
    };

    let conditions: Vec<String> = tags
        .iter()
        .map(|t| {
            params.values.push(t.clone());
            // ifnull so a row with no tags is simply not a match, rather than making the whole
            // condition NULL.
            let parsed = format!("from_json(ifnull({}, '[]'), '[\"VARCHAR\"]')", col);
            match operator {
                OptionsOp::AnyOf => format!("list_contains({}, ?)", parsed),
                OptionsOp::NoneOf => format!("NOT list_contains({}, ?)", parsed),
            }
        })
        .collect();

    let join_op = match operator {
        OptionsOp::AnyOf => " OR ",
        OptionsOp::NoneOf => " AND ",
    };

    format!("({})", conditions.join(join_op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_ports::filters::columns;

    #[test]
    fn build_tags_filter_any_of() {
        let tags = vec!["important".to_string(), "urgent".to_string()];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::AnyOf, &mut params, "");

        assert_eq!(
            sql,
            "(list_contains(from_json(ifnull(tags, '[]'), '[\"VARCHAR\"]'), ?) OR \
             list_contains(from_json(ifnull(tags, '[]'), '[\"VARCHAR\"]'), ?))"
        );
        assert_eq!(params.values, vec!["important", "urgent"]);
    }

    #[test]
    fn build_tags_filter_any_of_with_alias() {
        let tags = vec!["important".to_string()];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::AnyOf, &mut params, "sp");

        assert_eq!(
            sql,
            "(list_contains(from_json(ifnull(sp.tags, '[]'), '[\"VARCHAR\"]'), ?))"
        );
        assert_eq!(params.values, vec!["important"]);
    }

    #[test]
    fn build_tags_filter_none_of() {
        let tags = vec!["spam".to_string()];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::NoneOf, &mut params, "");

        assert_eq!(
            sql,
            "(NOT list_contains(from_json(ifnull(tags, '[]'), '[\"VARCHAR\"]'), ?))"
        );
        assert_eq!(params.values, vec!["spam"]);
    }

    #[test]
    fn build_tags_filter_empty() {
        let tags: Vec<String> = vec![];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::AnyOf, &mut params, "");

        assert_eq!(sql, "1=1");
        assert!(params.values.is_empty());
    }

    #[test]
    fn map_trace_columns() {
        assert_eq!(
            columns::map_trace_column_to_spans("input_tokens"),
            "gen_ai_usage_input_tokens"
        );
        assert_eq!(
            columns::map_trace_column_to_spans("total_cost"),
            "gen_ai_cost_total"
        );
        assert_eq!(
            columns::map_trace_column_to_spans("start_time"),
            "timestamp_start"
        );
        assert_eq!(columns::map_trace_column_to_spans("unknown"), "unknown");
    }

    #[test]
    fn map_session_columns() {
        assert_eq!(
            columns::map_session_column_to_spans("start_time"),
            "timestamp_start"
        );
        assert_eq!(
            columns::map_session_column_to_spans("end_time"),
            "timestamp_end"
        );
        assert_eq!(
            columns::map_session_column_to_spans("session_id"),
            "session_id"
        );
    }

    #[test]
    fn map_span_columns() {
        // API aliases should map to DB column names
        assert_eq!(columns::map_span_column("start_time"), "timestamp_start");
        assert_eq!(columns::map_span_column("end_time"), "timestamp_end");
        // Native DB column names should pass through unchanged
        assert_eq!(
            columns::map_span_column("timestamp_start"),
            "timestamp_start"
        );
        assert_eq!(columns::map_span_column("timestamp_end"), "timestamp_end");
        assert_eq!(columns::map_span_column("span_name"), "span_name");
        assert_eq!(columns::map_span_column("duration_ms"), "duration_ms");
    }

    /// Every filter the web UI offers must be accepted by the server.
    ///
    /// The two lists are declared in different languages in different repos-within-a-repo, so
    /// they drift: Provider, Total Tokens and Total Cost appeared in the span filter dropdown
    /// while `SPAN_FILTERABLE` rejected them, so choosing one silently did nothing. Parses the
    /// TypeScript rather than restating it, so the test cannot drift from the UI either.
    #[test]
    fn ui_filter_columns_are_all_server_filterable() {
        let ui = include_str!("../../../../web/src/lib/filters.ts");

        let extract = |config: &str| -> Vec<String> {
            let Some(start) = ui.find(config) else {
                return Vec::new();
            };
            let body = &ui[start..];
            let end = body.find("\n];").unwrap_or(body.len());
            body[..end]
                .split("column:")
                .skip(1)
                .filter_map(|seg| {
                    let seg = seg.trim_start();
                    let seg = seg.strip_prefix('"')?;
                    seg.split('"').next().map(str::to_string)
                })
                .collect()
        };

        for (config, allowed, label) in [
            ("SPAN_FILTER_CONFIGS", columns::SPAN_FILTERABLE, "span"),
            ("TRACE_FILTER_CONFIGS", columns::TRACE_FILTERABLE, "trace"),
            (
                "SESSION_FILTER_CONFIGS",
                columns::SESSION_FILTERABLE,
                "session",
            ),
        ] {
            let ui_columns = extract(config);
            assert!(
                !ui_columns.is_empty(),
                "could not parse {config} from web/src/lib/filters.ts - if it was renamed, update \
                 this test rather than deleting it"
            );
            let missing: Vec<&String> = ui_columns
                .iter()
                .filter(|c| !allowed.contains(&c.as_str()))
                .collect();
            assert!(
                missing.is_empty(),
                "the {label} filter UI offers columns the server rejects: {missing:?}"
            );
        }
    }
}
