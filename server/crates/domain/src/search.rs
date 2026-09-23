//! Search tokenisation, parser, exact verification, and fragment generation.

use std::collections::{BTreeMap, HashSet};

use thiserror::Error;

use sideseat_ports::error::DataError;
use sideseat_ports::traits::SearchIndex;
use sideseat_ports::types::{
    LogSearchSource, NormalizedLog, NormalizedSpan, ProjectId, SEARCH_TERMS_PER_FIELD,
    SearchBackfillDocument, SearchCandidate, SearchDocument, SearchExpr, SearchField,
    SearchFieldTerms, SearchPage, SearchQuery, SearchRecord, SearchSignal, SearchSource,
    SpanSearchSource,
};

const FRAGMENT_CONTEXT_CHARS: usize = 80;
const MAX_FRAGMENTS: usize = 3;
const _: () = assert!(sideseat_ports::types::SEARCH_RECALL_FLOOR >= 0.95);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    False,
    True,
    Unknown,
}

impl Truth {
    fn and(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }

    fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SearchParseError {
    #[error("search query is empty")]
    Empty,
    #[error("unknown search field `{0}`")]
    UnknownField(String),
    #[error("search field `{field}` is not valid for {signal}")]
    WrongSignal { field: String, signal: String },
    #[error("unexpected token at byte {0}")]
    Unexpected(usize),
    #[error("unclosed quoted phrase")]
    UnclosedPhrase,
    #[error("unclosed parenthesis")]
    UnclosedParenthesis,
}

#[derive(Debug)]
pub struct SearchHit {
    pub record: SearchRecord,
    pub indeterminate: bool,
    pub fragments: Vec<String>,
}

#[derive(Debug)]
pub struct SearchResultPage {
    pub hits: Vec<SearchHit>,
    pub next_cursor: Option<sideseat_ports::types::SearchCursor>,
    pub examined: u32,
    pub examination_limit_reached: bool,
    pub arrivals_detected: bool,
    pub index_lag_us: u64,
    pub search_indexing_complete: bool,
}

pub struct SearchService;

impl SearchService {
    pub async fn execute<T: SearchIndex + ?Sized>(
        index: &T,
        query: &SearchQuery,
    ) -> Result<SearchResultPage, DataError> {
        let SearchPage {
            candidates,
            next_cursor,
            examined,
            examination_limit_reached,
            arrivals_detected: _,
            index_lag_us,
            search_indexing_complete,
        } = index.search(query).await?;
        let mut hits = Vec::with_capacity(query.limit as usize);
        let mut indexing_complete = search_indexing_complete;
        let mut actual_cursor = None;
        let mut actual_examined = 0;
        for candidate in candidates {
            let SearchCandidate {
                record,
                document,
                source,
                cursor,
                indeterminate,
            } = candidate;
            actual_examined += 1;
            actual_cursor = Some(cursor);
            let document = materialize_document(document, &source);
            let exact = evaluate(&query.expression, &document, query.signal, true);
            indexing_complete &= document.indexed
                && !document_touched_truncation(&query.expression, &document, query.signal);
            if exact != Truth::False {
                hits.push(SearchHit {
                    fragments: fragments(&document, &query.expression, query.signal),
                    record,
                    indeterminate: indeterminate || exact == Truth::Unknown,
                });
                if hits.len() >= query.limit as usize {
                    break;
                }
            }
        }
        let consumed_all = actual_examined == examined;
        let arrivals_detected = if let Some(cursor) = actual_cursor.as_ref() {
            index.search_arrivals_detected(query, cursor).await?
        } else {
            false
        };
        Ok(SearchResultPage {
            hits,
            next_cursor: actual_cursor.or(next_cursor),
            examined: actual_examined,
            examination_limit_reached: consumed_all && examination_limit_reached,
            arrivals_detected,
            index_lag_us,
            search_indexing_complete: indexing_complete,
        })
    }

    /// Process one marker-checkpointed historical-index page.
    pub async fn backfill_project_page<T: SearchIndex + ?Sized>(
        index: &T,
        project_id: &ProjectId,
        signal: SearchSignal,
        limit: usize,
    ) -> Result<usize, DataError> {
        let sources = index
            .search_backfill_page(project_id, signal, limit)
            .await?;
        if sources.is_empty() {
            return Ok(0);
        }
        let documents = sources
            .into_iter()
            .map(|source| SearchBackfillDocument {
                id: source.id,
                expected_content_digest: source.expected_content_digest,
                document: document_for_source(&source.source),
            })
            .collect::<Vec<_>>();
        index
            .write_search_backfill(project_id, signal, &documents)
            .await?;
        Ok(documents.len())
    }
}

/// ClickHouse `splitByNonAlpha` contract owned by the domain: Unicode alphanumeric runs,
/// lower-cased without diacritic folding. Empty runs are discarded.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut token = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            token.extend(character.to_lowercase());
        } else if !token.is_empty() {
            result.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        result.push(token);
    }
    result
}

fn field_terms(field: SearchField, text: String) -> SearchFieldTerms {
    field_terms_with_limit(field, text, SEARCH_TERMS_PER_FIELD)
}

fn field_terms_with_limit(field: SearchField, text: String, limit: usize) -> SearchFieldTerms {
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    let mut truncated = false;
    for term in tokenize(&text) {
        if seen.insert(term.clone()) {
            if terms.len() == limit {
                truncated = true;
                break;
            }
            terms.push(term);
        }
    }
    SearchFieldTerms {
        field,
        terms,
        truncated,
        text,
    }
}

pub fn index_spans(spans: &mut [NormalizedSpan]) {
    for span in spans {
        span.search = span_document(span);
    }
}

pub fn index_logs(logs: &mut [NormalizedLog]) {
    for log in logs {
        log.search = log_document(log);
    }
}

pub fn span_document(span: &NormalizedSpan) -> SearchDocument {
    document_from_texts(span_source_texts(&SpanSearchSource {
        messages: span.messages.clone(),
        tool_definitions: span.tool_definitions.clone(),
        tool_names: span.tool_names.clone(),
        input_preview: span.input_preview.clone(),
        output_preview: span.output_preview.clone(),
        gen_ai_tool_name: span.gen_ai_tool_name.clone(),
        status_message: span.status_message.clone(),
        exception_type: span.exception_type.clone(),
        exception_message: span.exception_message.clone(),
        exception_stacktrace: span.exception_stacktrace.clone(),
        span_name: Some(span.span_name.clone()),
    }))
}

pub fn log_document(log: &NormalizedLog) -> SearchDocument {
    document_from_texts(log_source_texts(&LogSearchSource {
        body_text: log.body_text.clone(),
        body: serde_json::to_string(&log.body).ok(),
        event_name: log.event_name.clone(),
        severity_text: log.severity_text.clone(),
        attributes: serde_json::to_string(&log.attributes).ok(),
    }))
}

fn document_from_texts(texts: BTreeMap<SearchField, String>) -> SearchDocument {
    SearchDocument {
        indexed: true,
        fields: texts
            .into_iter()
            .map(|(field, text)| field_terms(field, text))
            .collect(),
    }
}

fn materialize_document(indexed: SearchDocument, source: &SearchSource) -> SearchDocument {
    let texts = match source {
        SearchSource::Span(source) => span_source_texts(source),
        SearchSource::Log(source) => log_source_texts(source),
    };
    SearchDocument {
        indexed: indexed.indexed,
        fields: texts
            .into_iter()
            .map(|(field, text)| {
                field_terms_with_limit(
                    field,
                    text,
                    if indexed.indexed {
                        SEARCH_TERMS_PER_FIELD
                    } else {
                        usize::MAX
                    },
                )
            })
            .collect(),
    }
}

pub fn document_for_source(source: &SearchSource) -> SearchDocument {
    match source {
        SearchSource::Span(source) => document_from_texts(span_source_texts(source)),
        SearchSource::Log(source) => document_from_texts(log_source_texts(source)),
    }
}

fn span_source_texts(source: &SpanSearchSource) -> BTreeMap<SearchField, String> {
    let mut text: BTreeMap<SearchField, Vec<String>> = [
        SearchField::Prompt,
        SearchField::Completion,
        SearchField::ToolName,
        SearchField::ToolArgs,
        SearchField::Error,
        SearchField::SpanName,
    ]
    .into_iter()
    .map(|field| (field, Vec::new()))
    .collect();
    push(
        &mut text,
        SearchField::Prompt,
        source.input_preview.as_deref(),
    );
    push(
        &mut text,
        SearchField::Completion,
        source.output_preview.as_deref(),
    );
    push(
        &mut text,
        SearchField::ToolName,
        source.gen_ai_tool_name.as_deref(),
    );
    push(
        &mut text,
        SearchField::ToolName,
        source.tool_names.as_deref(),
    );
    push(
        &mut text,
        SearchField::ToolArgs,
        source.tool_definitions.as_deref(),
    );
    for value in [
        source.status_message.as_deref(),
        source.exception_type.as_deref(),
        source.exception_message.as_deref(),
        source.exception_stacktrace.as_deref(),
    ] {
        push(&mut text, SearchField::Error, value);
    }
    push(
        &mut text,
        SearchField::SpanName,
        source.span_name.as_deref(),
    );
    if let Some(messages) = source.messages.as_deref()
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(messages)
    {
        collect_message_text(&value, None, &mut text);
    }
    text.into_iter()
        .map(|(field, values)| (field, values.join("\n")))
        .collect()
}

fn log_source_texts(source: &LogSearchSource) -> BTreeMap<SearchField, String> {
    [
        (
            SearchField::Body,
            source
                .body_text
                .clone()
                .or_else(|| source.body.clone())
                .unwrap_or_default(),
        ),
        (
            SearchField::EventName,
            source.event_name.clone().unwrap_or_default(),
        ),
        (
            SearchField::Severity,
            source.severity_text.clone().unwrap_or_default(),
        ),
        (
            SearchField::Attributes,
            source.attributes.clone().unwrap_or_default(),
        ),
    ]
    .into_iter()
    .collect()
}

fn push(map: &mut BTreeMap<SearchField, Vec<String>>, field: SearchField, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        map.entry(field).or_default().push(value.to_string());
    }
}

fn collect_message_text(
    value: &serde_json::Value,
    inherited_role: Option<&str>,
    fields: &mut BTreeMap<SearchField, Vec<String>>,
) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_message_text(value, inherited_role, fields);
            }
        }
        serde_json::Value::Object(object) => {
            let role = object
                .get("role")
                .and_then(serde_json::Value::as_str)
                .or(inherited_role);
            for (key, value) in object {
                if key != "role" {
                    collect_message_text(value, role, fields);
                }
            }
        }
        serde_json::Value::String(value) => {
            let field = match inherited_role.unwrap_or_default() {
                "assistant" | "model" => SearchField::Completion,
                "tool" | "function" => SearchField::ToolArgs,
                _ => SearchField::Prompt,
            };
            push(fields, field, Some(value));
        }
        _ => {}
    }
}

pub fn parse(
    input: &str,
    signal: sideseat_ports::types::SearchSignal,
) -> Result<SearchExpr, SearchParseError> {
    let tokens = lex(input)?;
    if tokens.is_empty() {
        return Err(SearchParseError::Empty);
    }
    let mut parser = Parser {
        tokens: &tokens,
        position: 0,
        signal,
    };
    let expression = parser.parse_or(None)?;
    if parser.position != tokens.len() {
        return Err(SearchParseError::Unexpected(
            tokens[parser.position].position,
        ));
    }
    Ok(expression)
}

#[derive(Debug)]
enum TokenKind {
    Word(String),
    Phrase(String),
    Colon,
    Open,
    Close,
    And,
    Or,
    Not,
}

#[derive(Debug)]
struct Token {
    kind: TokenKind,
    position: usize,
}

fn lex(input: &str) -> Result<Vec<Token>, SearchParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();
    while let Some((position, character)) = chars.next() {
        if character.is_whitespace() {
            continue;
        }
        let kind = match character {
            ':' => TokenKind::Colon,
            '(' => TokenKind::Open,
            ')' => TokenKind::Close,
            '"' => {
                let mut phrase = String::new();
                let mut closed = false;
                for (_, character) in chars.by_ref() {
                    if character == '"' {
                        closed = true;
                        break;
                    }
                    phrase.push(character);
                }
                if !closed {
                    return Err(SearchParseError::UnclosedPhrase);
                }
                TokenKind::Phrase(phrase)
            }
            _ => {
                let mut word = String::from(character);
                while let Some((_, next)) = chars.peek() {
                    if next.is_whitespace() || matches!(next, ':' | '(' | ')' | '"') {
                        break;
                    }
                    word.push(*next);
                    chars.next();
                }
                match word.to_ascii_uppercase().as_str() {
                    "AND" => TokenKind::And,
                    "OR" => TokenKind::Or,
                    "NOT" => TokenKind::Not,
                    _ => TokenKind::Word(word),
                }
            }
        };
        tokens.push(Token { kind, position });
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    signal: sideseat_ports::types::SearchSignal,
}

impl Parser<'_> {
    fn parse_or(&mut self, field: Option<SearchField>) -> Result<SearchExpr, SearchParseError> {
        let mut expression = self.parse_and(field)?;
        while self.take(|kind| matches!(kind, TokenKind::Or)) {
            expression = SearchExpr::Or(Box::new(expression), Box::new(self.parse_and(field)?));
        }
        Ok(expression)
    }

    fn parse_and(&mut self, field: Option<SearchField>) -> Result<SearchExpr, SearchParseError> {
        let mut expression = self.parse_unary(field)?;
        loop {
            let explicit = self.take(|kind| matches!(kind, TokenKind::And));
            let implicit = self.peek().is_some_and(|kind| {
                matches!(
                    kind,
                    TokenKind::Word(_) | TokenKind::Phrase(_) | TokenKind::Open | TokenKind::Not
                )
            });
            if explicit || implicit {
                expression =
                    SearchExpr::And(Box::new(expression), Box::new(self.parse_unary(field)?));
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn parse_unary(
        &mut self,
        inherited: Option<SearchField>,
    ) -> Result<SearchExpr, SearchParseError> {
        if self.take(|kind| matches!(kind, TokenKind::Not)) {
            return Ok(SearchExpr::Not(Box::new(self.parse_unary(inherited)?)));
        }
        if let Some(TokenKind::Word(name)) = self.peek()
            && self
                .tokens
                .get(self.position + 1)
                .is_some_and(|token| matches!(token.kind, TokenKind::Colon))
        {
            let name = name.clone();
            self.position += 2;
            let field = SearchField::parse(&name)
                .ok_or_else(|| SearchParseError::UnknownField(name.clone()))?;
            if !field.belongs_to(self.signal) {
                return Err(SearchParseError::WrongSignal {
                    field: name,
                    signal: format!("{:?}", self.signal).to_lowercase(),
                });
            }
            return self.parse_unary(Some(field));
        }
        self.parse_primary(inherited)
    }

    fn parse_primary(
        &mut self,
        field: Option<SearchField>,
    ) -> Result<SearchExpr, SearchParseError> {
        let Some(token) = self.tokens.get(self.position) else {
            return Err(SearchParseError::Unexpected(
                self.tokens.last().map_or(0, |token| token.position + 1),
            ));
        };
        self.position += 1;
        match &token.kind {
            TokenKind::Word(word) if word == "*" => Ok(SearchExpr::MatchAll),
            TokenKind::Word(word) => {
                let terms = tokenize(word);
                if terms.is_empty() {
                    return Err(SearchParseError::Unexpected(token.position));
                }
                let mut terms = terms.into_iter();
                let first = SearchExpr::Term {
                    field,
                    term: terms.next().expect("non-empty"),
                };
                Ok(terms.fold(first, |left, term| {
                    SearchExpr::And(Box::new(left), Box::new(SearchExpr::Term { field, term }))
                }))
            }
            TokenKind::Phrase(phrase) => {
                let terms = tokenize(phrase);
                if terms.is_empty() {
                    return Err(SearchParseError::Unexpected(token.position));
                }
                Ok(SearchExpr::Phrase {
                    field,
                    phrase: phrase.clone(),
                    terms,
                })
            }
            TokenKind::Open => {
                let expression = self.parse_or(field)?;
                if !self.take(|kind| matches!(kind, TokenKind::Close)) {
                    return Err(SearchParseError::UnclosedParenthesis);
                }
                Ok(expression)
            }
            _ => Err(SearchParseError::Unexpected(token.position)),
        }
    }

    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.position).map(|token| &token.kind)
    }

    fn take(&mut self, predicate: impl FnOnce(&TokenKind) -> bool) -> bool {
        if self.peek().is_some_and(predicate) {
            self.position += 1;
            true
        } else {
            false
        }
    }
}

pub fn evaluate(
    expression: &SearchExpr,
    document: &SearchDocument,
    signal: sideseat_ports::types::SearchSignal,
    verify_phrases: bool,
) -> Truth {
    match expression {
        SearchExpr::MatchAll => Truth::True,
        SearchExpr::Term { field, term } => evaluate_leaf(
            document,
            signal,
            *field,
            &[term.as_str()],
            None,
            verify_phrases,
        ),
        SearchExpr::Phrase {
            field,
            phrase: _,
            terms,
        } => {
            let borrowed = terms.iter().map(String::as_str).collect::<Vec<_>>();
            evaluate_leaf(
                document,
                signal,
                *field,
                &borrowed,
                Some(terms),
                verify_phrases,
            )
        }
        SearchExpr::And(left, right) => evaluate(left, document, signal, verify_phrases)
            .and(evaluate(right, document, signal, verify_phrases)),
        SearchExpr::Or(left, right) => evaluate(left, document, signal, verify_phrases)
            .or(evaluate(right, document, signal, verify_phrases)),
        SearchExpr::Not(inner) => evaluate(inner, document, signal, verify_phrases).not(),
    }
}

fn evaluate_leaf(
    document: &SearchDocument,
    signal: sideseat_ports::types::SearchSignal,
    restricted: Option<SearchField>,
    terms: &[&str],
    phrase_terms: Option<&Vec<String>>,
    verify_phrases: bool,
) -> Truth {
    let fields = document.fields.iter().filter(|entry| {
        entry.field.belongs_to(signal) && restricted.is_none_or(|field| field == entry.field)
    });
    let mut result = Truth::False;
    for field in fields {
        let all_present = terms
            .iter()
            .all(|term| field.terms.iter().any(|candidate| candidate == term));
        let value = if all_present {
            if verify_phrases && let Some(phrase_terms) = phrase_terms {
                if field.text.is_empty() {
                    Truth::Unknown
                } else if contains_phrase(&field.text, phrase_terms) {
                    Truth::True
                } else {
                    Truth::False
                }
            } else {
                Truth::True
            }
        } else if field.truncated {
            Truth::Unknown
        } else {
            Truth::False
        };
        result = result.or(value);
    }
    result
}

fn contains_phrase(text: &str, phrase: &[String]) -> bool {
    let tokens = tokenize(text);
    tokens.windows(phrase.len()).any(|window| window == phrase)
}

fn document_touched_truncation(
    expression: &SearchExpr,
    document: &SearchDocument,
    signal: sideseat_ports::types::SearchSignal,
) -> bool {
    match expression {
        SearchExpr::MatchAll => false,
        SearchExpr::Term { field, .. } | SearchExpr::Phrase { field, .. } => {
            document.fields.iter().any(|entry| {
                entry.truncated
                    && entry.field.belongs_to(signal)
                    && field.is_none_or(|field| field == entry.field)
            })
        }
        SearchExpr::And(left, right) | SearchExpr::Or(left, right) => {
            document_touched_truncation(left, document, signal)
                || document_touched_truncation(right, document, signal)
        }
        SearchExpr::Not(inner) => document_touched_truncation(inner, document, signal),
    }
}

pub fn fragments(
    document: &SearchDocument,
    expression: &SearchExpr,
    signal: sideseat_ports::types::SearchSignal,
) -> Vec<String> {
    let mut needles = Vec::new();
    collect_positive_terms(expression, false, &mut needles);
    let mut fragments = Vec::new();
    for field in document
        .fields
        .iter()
        .filter(|field| field.field.belongs_to(signal))
    {
        let lower = field.text.to_lowercase();
        for needle in &needles {
            if let Some(byte) = lower.find(needle) {
                let start = previous_char_boundary(
                    &field.text,
                    byte.saturating_sub(FRAGMENT_CONTEXT_CHARS),
                );
                let end = next_char_boundary(
                    &field.text,
                    (byte + needle.len() + FRAGMENT_CONTEXT_CHARS).min(field.text.len()),
                );
                fragments.push(field.text[start..end].to_string());
                if fragments.len() == MAX_FRAGMENTS {
                    return fragments;
                }
            }
        }
    }
    fragments
}

fn collect_positive_terms(expression: &SearchExpr, negated: bool, output: &mut Vec<String>) {
    match expression {
        SearchExpr::Term { term, .. } if !negated => output.push(term.clone()),
        SearchExpr::Phrase { phrase, .. } if !negated => output.push(phrase.to_lowercase()),
        SearchExpr::And(left, right) | SearchExpr::Or(left, right) => {
            collect_positive_terms(left, negated, output);
            collect_positive_terms(right, negated, output);
        }
        SearchExpr::Not(inner) => collect_positive_terms(inner, !negated, output),
        _ => {}
    }
}

fn previous_char_boundary(value: &str, mut position: usize) -> usize {
    while position > 0 && !value.is_char_boundary(position) {
        position -= 1;
    }
    position
}

fn next_char_boundary(value: &str, mut position: usize) -> usize {
    while position < value.len() && !value.is_char_boundary(position) {
        position += 1;
    }
    position
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use chrono::{DateTime, Utc};
    use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
    use sideseat_core::storage::AppStorage;
    use sideseat_ports::clock::Clock;
    use sideseat_ports::traits::SpanStore;
    use sideseat_ports::types::{ProjectId, SearchQuery, SearchSignal};
    use tempfile::TempDir;

    #[derive(Debug)]
    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            DateTime::from_timestamp(1_700_000_100, 0).unwrap()
        }
    }

    #[test]
    fn tokenizer_contract_covers_unicode_code_json_cjk_and_diacritics() {
        assert_eq!(
            tokenize("Crème brûlée, HTTP_2 foo.bar 中文 JSON:{\"a\":1}"),
            [
                "crème", "brûlée", "http", "2", "foo", "bar", "中文", "json", "a", "1"
            ]
        );
    }

    #[test]
    fn parser_preserves_field_boolean_and_exact_phrase_structure() {
        let parsed = parse(
            r#"prompt:"hello world" AND NOT (error:timeout OR tool_name:delete)"#,
            SearchSignal::Spans,
        )
        .unwrap();
        assert!(matches!(parsed, SearchExpr::And(_, _)));
    }

    #[test]
    fn truncated_negation_is_unknown_under_nesting() {
        let document = SearchDocument {
            indexed: true,
            fields: vec![SearchFieldTerms {
                field: SearchField::Prompt,
                terms: vec!["a".into()],
                truncated: true,
                text: "a z".into(),
            }],
        };
        let expression = SearchExpr::Not(Box::new(SearchExpr::Or(
            Box::new(SearchExpr::Term {
                field: Some(SearchField::Prompt),
                term: "missing".into(),
            }),
            Box::new(SearchExpr::Term {
                field: Some(SearchField::Prompt),
                term: "also_missing".into(),
            }),
        )));
        assert_eq!(
            evaluate(&expression, &document, SearchSignal::Spans, true),
            Truth::Unknown
        );
    }

    #[test]
    fn phrase_is_verified_consecutively() {
        let document = SearchDocument {
            indexed: true,
            fields: vec![field_terms(SearchField::Prompt, "foo x bar foo bar".into())],
        };
        let expression = SearchExpr::Phrase {
            field: Some(SearchField::Prompt),
            phrase: "foo bar".into(),
            terms: vec!["foo".into(), "bar".into()],
        };
        assert_eq!(
            evaluate(&expression, &document, SearchSignal::Spans, true),
            Truth::True
        );
    }

    #[test]
    fn cap_and_recall_floor_are_explicit() {
        let source = (0..SEARCH_TERMS_PER_FIELD + 10)
            .map(|index| format!("term{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let indexed = field_terms(SearchField::Prompt, source);
        assert!(indexed.truncated);
        assert_eq!(indexed.terms.len(), SEARCH_TERMS_PER_FIELD);
    }

    #[tokio::test]
    async fn duckdb_search_replaces_terms_and_cursor_advances_over_empty_pages() {
        let directory = TempDir::new().unwrap();
        let storage = AppStorage::init_for_test(directory.path().to_path_buf());
        let service = Arc::new(
            DuckdbService::init(&storage, Arc::new(FixedClock))
                .await
                .unwrap(),
        );
        let repository = DuckdbRepository(service);
        let timestamp = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut spans = vec![
            NormalizedSpan {
                project_id: Some("search-project".into()),
                trace_id: "trace-a".into(),
                span_id: "span-a".into(),
                span_name: "first".into(),
                timestamp_start: timestamp,
                input_preview: Some("alpha x beta".into()),
                ingested_at: Some(timestamp),
                ..Default::default()
            },
            NormalizedSpan {
                project_id: Some("search-project".into()),
                trace_id: "trace-b".into(),
                span_id: "span-b".into(),
                span_name: "second".into(),
                timestamp_start: timestamp,
                input_preview: Some("alpha beta".into()),
                ingested_at: Some(timestamp),
                ..Default::default()
            },
        ];
        index_spans(&mut spans);
        repository.insert_spans(spans).await.unwrap();

        let expression = parse(r#"prompt:"alpha beta""#, SearchSignal::Spans).unwrap();
        let first = SearchService::execute(
            &repository,
            &SearchQuery {
                project_id: ProjectId::from("search-project"),
                signal: SearchSignal::Spans,
                expression: expression.clone(),
                limit: 1,
                max_examined: 1,
                cursor: None,
                from_timestamp: None,
                to_timestamp: None,
            },
        )
        .await
        .unwrap();
        assert!(first.hits.is_empty());
        assert_eq!(first.examined, 1);
        let cursor = first.next_cursor.expect("empty page still advances");

        let second = SearchService::execute(
            &repository,
            &SearchQuery {
                project_id: ProjectId::from("search-project"),
                signal: SearchSignal::Spans,
                expression,
                limit: 1,
                max_examined: 1,
                cursor: Some(cursor),
                from_timestamp: None,
                to_timestamp: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(second.hits.len(), 1);
        assert!(matches!(
            &second.hits[0].record,
            SearchRecord::Span(row) if row.trace_id == "trace-b"
        ));

        let mut correction = NormalizedSpan {
            project_id: Some("search-project".into()),
            trace_id: "trace-b".into(),
            span_id: "span-b".into(),
            span_name: "second".into(),
            timestamp_start: timestamp,
            input_preview: Some("gamma delta".into()),
            ingested_at: DateTime::from_timestamp(1_700_000_001, 0),
            ..Default::default()
        };
        index_spans(std::slice::from_mut(&mut correction));
        repository.insert_spans(vec![correction]).await.unwrap();

        let old = SearchService::execute(
            &repository,
            &SearchQuery {
                project_id: ProjectId::from("search-project"),
                signal: SearchSignal::Spans,
                expression: parse("prompt:beta", SearchSignal::Spans).unwrap(),
                limit: 10,
                max_examined: 10,
                cursor: None,
                from_timestamp: None,
                to_timestamp: None,
            },
        )
        .await
        .unwrap();
        assert!(old.hits.iter().all(|hit| {
            !matches!(
                &hit.record,
                SearchRecord::Span(row) if row.trace_id == "trace-b"
            )
        }));
        let corrected = SearchService::execute(
            &repository,
            &SearchQuery {
                project_id: ProjectId::from("search-project"),
                signal: SearchSignal::Spans,
                expression: parse("prompt:gamma", SearchSignal::Spans).unwrap(),
                limit: 10,
                max_examined: 10,
                cursor: None,
                from_timestamp: None,
                to_timestamp: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(corrected.hits.len(), 1);
    }
}
