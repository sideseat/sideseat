//! Which OTLP log records carry a message event, compiled from the `log_events` declarations.
//!
//! A log record is recognised here and nothing more: what the event *says* is answered by the same
//! `message_events` declaration and readings a span event of that name gets, so a producer that moves its
//! conversation from span events to log records needs one declaration rather than a second set of readings.

use std::collections::BTreeMap;

use super::diagnostics::ClauseDefect;
use super::expr;
use super::schema::{LogEventPayload, RuleFile};

/// Where a log record states its event name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEventNameSource {
    /// The record's own `event_name` field.
    EventName,
    /// A record attribute, which is where producers that predate the field put the name.
    Attribute(String),
    /// A member of the record's **body**, for a producer whose logging writes one structured event per record
    /// and names the kind inside it. Read from a body that is a map, or text that parses as a JSON object -
    /// the same two shapes `payload: body_members` reads, so a record names its event where it carries it.
    BodyMember(String),
}

impl LogEventNameSource {
    const EVENT_NAME: &'static str = "event_name";
    const ATTRIBUTE_PREFIX: &'static str = "attr:";
    const BODY_PREFIX: &'static str = "body:";

    fn parse(spelling: &str) -> Result<Self, String> {
        if spelling == Self::EVENT_NAME {
            return Ok(Self::EventName);
        }
        for (prefix, build) in [
            (
                Self::ATTRIBUTE_PREFIX,
                (|key: &str| Self::Attribute(key.to_string())) as fn(&str) -> Self,
            ),
            (Self::BODY_PREFIX, |key: &str| {
                Self::BodyMember(key.to_string())
            }),
        ] {
            match spelling.strip_prefix(prefix) {
                Some(key) if !key.is_empty() => return Ok(build(key)),
                Some(_) => {
                    return Err(format!(
                        "`{spelling}` names a member with an empty key, which no record carries"
                    ));
                }
                None => {}
            }
        }
        Err(format!(
            "`{spelling}` is not a name source - expected `{}`, `{}<key>` or `{}<member>`",
            Self::EVENT_NAME,
            Self::ATTRIBUTE_PREFIX,
            Self::BODY_PREFIX
        ))
    }
}

/// One recognised log event, with every declaration that agrees about it.
#[derive(Debug, Clone)]
pub struct DeclaredLogEvent {
    /// The event name, which is also a `message_events` entry.
    pub name: String,
    /// Where a record states the name, in order; the first present source decides.
    pub name_from: Vec<LogEventNameSource>,
    /// Where the event's attributes are on the record.
    pub payload: LogEventPayload,
    /// Every agreeing declaration, so an answer can name the clauses that produced it.
    pub witnesses: expr::EvidenceSet,
}

/// The compiled `log_events` declarations, in event-name order.
#[derive(Debug, Default)]
pub struct LogEventPlan {
    events: Vec<DeclaredLogEvent>,
}

impl LogEventPlan {
    /// Compile every asset's `log_events`, refusing a declaration that could not mean what it says.
    ///
    /// `message_events` is the compiled registry: a log event must name one of its entries, because that
    /// entry and its readings are what interpret the event once it is recognised.
    pub fn compile(
        files: &[RuleFile],
        message_events: &BTreeMap<String, super::DeclaredMessageEvent>,
    ) -> Result<Self, super::diagnostics::ClauseDefect> {
        let mut by_name: BTreeMap<String, DeclaredLogEvent> = BTreeMap::new();
        for file in files {
            for event in &file.log_events {
                if event.name.is_empty() {
                    return Err(ClauseDefect::new(
                        &[&event.id],
                        Some(&file.id),
                        format!("`{}`: log event `{}` has no name", file.id, event.id),
                    ));
                }
                if !message_events.contains_key(&event.name) {
                    return Err(ClauseDefect::new(
                        &[&event.id],
                        Some(&file.id),
                        format!(
                            "`{}`: log event `{}` names `{}`, which no asset declares in `message_events` - \
                         nothing would read it once recognised",
                            file.id, event.id, event.name
                        ),
                    ));
                }
                if event.name_from.is_empty() {
                    return Err(ClauseDefect::new(
                        &[&event.id],
                        Some(&file.id),
                        format!(
                            "`{}`: log event `{}` declares no `name_from`, so no record could be recognised as it",
                            file.id, event.id
                        ),
                    ));
                }
                let mut name_from = Vec::with_capacity(event.name_from.len());
                for spelling in &event.name_from {
                    let spelling = &spelling.0;
                    let source = LogEventNameSource::parse(spelling).map_err(|error| {
                        ClauseDefect::new(
                            &[&event.id],
                            Some(&file.id),
                            format!("`{}`: log event `{}`: {error}", file.id, event.id),
                        )
                    })?;
                    if name_from.contains(&source) {
                        return Err(ClauseDefect::new(
                            &[&event.id],
                            Some(&file.id),
                            format!(
                                "`{}`: log event `{}` lists `{spelling}` twice in `name_from`",
                                file.id, event.id
                            ),
                        ));
                    }
                    name_from.push(source);
                }
                let path = expr::ClausePath::root(event.id.clone());
                match by_name.get_mut(&event.name) {
                    None => {
                        by_name.insert(
                            event.name.clone(),
                            DeclaredLogEvent {
                                name: event.name.clone(),
                                name_from,
                                payload: event.payload,
                                witnesses: expr::EvidenceSet::one(path),
                            },
                        );
                    }
                    Some(existing)
                        if existing.payload == event.payload && existing.name_from == name_from =>
                    {
                        let mut paths = existing.witnesses.paths().to_vec();
                        paths.push(path);
                        existing.witnesses = expr::EvidenceSet::of(paths)
                            .expect("a non-empty witness list stays non-empty");
                    }
                    Some(existing) => {
                        return Err(ClauseDefect::new(
                            &[&event.id],
                            Some(&file.id),
                            format!(
                                "log event `{}` is declared twice with different shapes (by {} and `{}`) - which \
                             applies would depend on load order",
                                event.name, existing.witnesses, event.id
                            ),
                        ));
                    }
                }
            }
        }
        Ok(Self {
            events: by_name.into_values().collect(),
        })
    }

    /// The declaration a log record matches, given its `event_name` field and a lookup of its attributes.
    ///
    /// Each declaration resolves the record's name from its own `name_from`, first present source first, so a
    /// record carrying both a field and a legacy attribute is named by whichever its declaration ranks higher.
    pub fn recognise<'a>(
        &self,
        event_name: Option<&'a str>,
        attribute: impl Fn(&str) -> Option<&'a str>,
        body_member: impl Fn(&str) -> Option<&'a str>,
    ) -> Option<&DeclaredLogEvent> {
        self.events.iter().find(|event| {
            event
                .name_from
                .iter()
                .find_map(|source| {
                    match source {
                        LogEventNameSource::EventName => event_name,
                        LogEventNameSource::Attribute(key) => attribute(key),
                        LogEventNameSource::BodyMember(member) => body_member(member),
                    }
                    .filter(|name| !name.is_empty())
                })
                .is_some_and(|name| name == event.name)
        })
    }

    /// Every compiled declaration, in event-name order.
    pub fn events(&self) -> &[DeclaredLogEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(value: serde_json::Value) -> RuleFile {
        serde_json::from_value(value).expect("a well-formed rule file")
    }

    fn with_message_event(log_events: serde_json::Value) -> Vec<RuleFile> {
        vec![file(json!({
            "id": "test",
            "message_events": [
                {"id": "test.event.user", "name": "test.user.message"},
                {"id": "test.event.details", "name": "inference.details", "raw": "replace"}
            ],
            "log_events": log_events
        }))]
    }

    fn compile(
        files: &[RuleFile],
    ) -> Result<LogEventPlan, super::super::diagnostics::ClauseDefect> {
        let events = super::super::compile_message_events(files)?;
        LogEventPlan::compile(files, &events)
    }

    #[test]
    fn a_log_event_must_name_a_message_event() {
        let error = compile(&with_message_event(json!([{
            "id": "test.log.unknown",
            "name": "test.nobody.message",
            "name_from": "event_name",
            "payload": "body_members"
        }])))
        .unwrap_err();
        assert!(error.contains("message_events"), "{error}");
    }

    #[test]
    fn name_sources_are_refused_when_absent_unknown_empty_or_repeated() {
        // An empty list is not writable: `name_from` is one source or a `first_of` of two or more.
        for (name_from, wanted) in [
            (json!("body"), "not a name source"),
            (json!("attr:"), "empty key"),
            (
                json!({"first_of": ["event_name", "event_name"], "mode": "usable"}),
                "twice",
            ),
        ] {
            let error = compile(&with_message_event(json!([{
                "id": "test.log.user",
                "name": "test.user.message",
                "name_from": name_from,
                "payload": "body_members"
            }])))
            .unwrap_err();
            assert!(error.contains(wanted), "{wanted}: {error}");
        }
    }

    #[test]
    fn two_declarations_of_one_event_must_agree() {
        let mut files = with_message_event(json!([{
            "id": "test.log.user",
            "name": "test.user.message",
            "name_from": "event_name",
            "payload": "body_members"
        }]));
        files.push(file(json!({
            "id": "other",
            "log_events": [
                {
                    "id": "other.log.user",
                    "name": "test.user.message",
                    "name_from": "event_name",
                    "payload": "attributes"
                }
            ]
        })));
        let error = compile(&files).unwrap_err();
        assert!(error.contains("load order"), "{error}");

        files[1].log_events[0].payload = LogEventPayload::BodyMembers;
        let plan = compile(&files).expect("agreeing declarations compile");
        assert_eq!(plan.events().len(), 1);
        assert_eq!(plan.events()[0].witnesses.paths().len(), 2);
    }

    #[test]
    fn a_record_is_named_by_the_first_present_source() {
        let plan = compile(&with_message_event(json!([
            {
                "id": "test.log.user",
                "name": "test.user.message",
                "name_from": {"first_of":["event_name", "attr:legacy.name"],"mode":"usable"},
                "payload": "body_members"
            },
            {
                "id": "test.log.details",
                "name": "inference.details",
                "name_from": {"first_of":["event_name", "attr:legacy.name"],"mode":"usable"},
                "payload": "attributes"
            }
        ])))
        .expect("compiles");

        let legacy = |key: &str| (key == "legacy.name").then_some("test.user.message");
        assert_eq!(
            plan.recognise(None, legacy, |_| None)
                .map(|event| event.name.as_str()),
            Some("test.user.message"),
            "a record without the field is named by its attribute"
        );
        assert_eq!(
            plan.recognise(Some(""), legacy, |_| None)
                .map(|event| event.name.as_str()),
            Some("test.user.message"),
            "an empty field is absent, not a name"
        );
        assert_eq!(
            plan.recognise(Some("inference.details"), legacy, |_| None)
                .map(|event| (event.name.as_str(), event.payload)),
            Some(("inference.details", LogEventPayload::Attributes)),
            "the field outranks the attribute when both are present"
        );
        assert!(
            plan.recognise(Some("app.audit"), |_| None, |_| None)
                .is_none()
        );
        assert!(plan.recognise(None, |_| None, |_| None).is_none());
    }

    /// **A record may name its event from a body member.** A producer whose logging writes one structured event
    /// per record names the kind inside it, where neither the record's field nor an attribute carries it.
    #[test]
    fn a_record_may_be_named_by_a_body_member() {
        let plan = compile(&with_message_event(json!([{
            "id": "test.log.body",
            "name": "test.user.message",
            "name_from": "body:kind",
            "payload": "body_members"
        }])))
        .expect("a body-member name source compiles");
        let body = |member: &str| (member == "kind").then_some("test.user.message");
        assert_eq!(
            plan.recognise(None, |_| None, body)
                .map(|event| event.name.as_str()),
            Some("test.user.message")
        );
        assert!(
            plan.recognise(Some("test.user.message"), |_| None, |_| None)
                .is_none(),
            "the declaration reads the body alone, so the record's own field does not name it"
        );
        assert!(
            plan.recognise(None, |_| None, |member| (member == "kind").then_some(""))
                .is_none(),
            "an empty member is absent, not a name"
        );
        // An empty member key names nothing a record carries, exactly as an empty attribute key does.
        for spelling in ["body:", "body"] {
            assert!(
                compile(&with_message_event(json!([{
                    "id": "test.log.body",
                    "name": "test.user.message",
                    "name_from": spelling,
                    "payload": "body_members"
                }])))
                .is_err(),
                "`{spelling}` must be refused"
            );
        }
    }
}
