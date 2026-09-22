//! Driver-independent search IR, indexed-document shape, and cursor contract.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{LogRow, ProjectId, SpanRow};

pub const SEARCH_TERMS_PER_FIELD: usize = 512;
pub const SEARCH_RECALL_FLOOR: f64 = 0.95;
pub const DEFAULT_SEARCH_MAX_EXAMINED: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSignal {
    Spans,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    Prompt,
    Completion,
    ToolName,
    ToolArgs,
    Error,
    SpanName,
    Body,
    EventName,
    Severity,
    Attributes,
}

impl SearchField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Completion => "completion",
            Self::ToolName => "tool_name",
            Self::ToolArgs => "tool_args",
            Self::Error => "error",
            Self::SpanName => "span_name",
            Self::Body => "body",
            Self::EventName => "event_name",
            Self::Severity => "severity",
            Self::Attributes => "attributes",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "prompt" => Some(Self::Prompt),
            "completion" => Some(Self::Completion),
            "tool_name" => Some(Self::ToolName),
            "tool_args" => Some(Self::ToolArgs),
            "error" => Some(Self::Error),
            "span_name" => Some(Self::SpanName),
            "body" => Some(Self::Body),
            "event_name" => Some(Self::EventName),
            "severity" => Some(Self::Severity),
            "attributes" => Some(Self::Attributes),
            _ => None,
        }
    }

    pub const fn belongs_to(self, signal: SearchSignal) -> bool {
        match signal {
            SearchSignal::Spans => matches!(
                self,
                Self::Prompt
                    | Self::Completion
                    | Self::ToolName
                    | Self::ToolArgs
                    | Self::Error
                    | Self::SpanName
            ),
            SearchSignal::Logs => matches!(
                self,
                Self::Body | Self::EventName | Self::Severity | Self::Attributes
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFieldTerms {
    pub field: SearchField,
    pub terms: Vec<String>,
    pub truncated: bool,
    /// Complete source text used for phrase verification and fragments.
    pub text: String,
}

impl Default for SearchFieldTerms {
    fn default() -> Self {
        Self {
            field: SearchField::Prompt,
            terms: Vec::new(),
            truncated: false,
            text: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchDocument {
    pub fields: Vec<SearchFieldTerms>,
}

impl SearchDocument {
    pub fn field(&self, field: SearchField) -> Option<&SearchFieldTerms> {
        self.fields.iter().find(|entry| entry.field == field)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchExpr {
    MatchAll,
    Term {
        field: Option<SearchField>,
        term: String,
    },
    Phrase {
        field: Option<SearchField>,
        phrase: String,
        terms: Vec<String>,
    },
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchCursor {
    pub timestamp_us: i64,
    pub tie_breaker: String,
    pub ordinal: u32,
    /// Time at which the first page started. Later writes are reported, never called absent.
    pub started_at_us: i64,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub project_id: ProjectId,
    pub signal: SearchSignal,
    pub expression: SearchExpr,
    pub limit: u32,
    pub max_examined: u32,
    pub cursor: Option<SearchCursor>,
    pub from_timestamp: Option<DateTime<Utc>>,
    pub to_timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub enum SearchRecord {
    Span(SpanRow),
    Log(LogRow),
}

#[derive(Debug)]
pub struct SearchCandidate {
    pub record: SearchRecord,
    pub document: SearchDocument,
    pub cursor: SearchCursor,
    /// The term index could not prove true or false because a relevant field hit its cap.
    pub indeterminate: bool,
}

#[derive(Debug)]
pub struct SearchPage {
    pub candidates: Vec<SearchCandidate>,
    /// Cursor of the last candidate examined by the adapter, including rejected phrase candidates.
    pub next_cursor: Option<SearchCursor>,
    pub examined: u32,
    pub examination_limit_reached: bool,
    pub arrivals_detected: bool,
    pub index_lag_us: u64,
}
