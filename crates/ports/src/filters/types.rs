//! Filter type definitions
//!
//! Defines the filter types and operators used for querying OTEL data.

use serde::Deserialize;

/// Why a filter could not be accepted.
///
/// The storage layer's own error, deliberately **not** `ApiError`. Filter parsing and validation used to
/// return one, which made the analytics adapters depend on the HTTP layer for their own vocabulary - the
/// wrong direction, and the thing that stops these modules moving into a crate that cannot see `api` at
/// all. The API converts it at the boundary (`From<FilterError> for ApiError`), so every route keeps
/// using `?` exactly as before.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FilterError {
    /// A column that is not on the caller's allowlist.
    #[error("Cannot filter by column: {column}")]
    UnknownColumn { column: String },
    /// The filter payload was not the shape a filter takes.
    #[error("{message}")]
    Malformed { message: String },
    /// More filters than the endpoint accepts.
    #[error("{message}")]
    TooMany { message: String },
    /// The payload is larger than the endpoint reads.
    ///
    /// Distinct from `TooMany`: a first draft folded both into it, which changed the machine-readable code
    /// an oversized payload returns from `FILTER_JSON_TOO_LARGE` to `TOO_MANY_FILTERS` - two different
    /// remedies (send less text, send fewer clauses) behind one code.
    #[error("{message}")]
    TooLarge { message: String },
}

impl FilterError {
    /// The stable machine-readable code, so the mapping to a response body carries no guesswork.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownColumn { .. } => "INVALID_FILTER_COLUMN",
            Self::Malformed { .. } => "INVALID_FILTER_JSON",
            Self::TooMany { .. } => "TOO_MANY_FILTERS",
            Self::TooLarge { .. } => "FILTER_JSON_TOO_LARGE",
        }
    }
}

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

impl Filter {
    /// Validate filter column against whitelist
    pub fn validate(&self, allowed_columns: &[&str]) -> Result<(), FilterError> {
        let column = self.column();
        if !allowed_columns.contains(&column.as_str()) {
            return Err(FilterError::UnknownColumn {
                column: column.to_owned(),
            });
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
}
