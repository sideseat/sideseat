//! Event filter matching for SSE subscriptions

use super::events::{SpanEvent, TraceEvent};
use crate::otel::query::TraceFilter;

/// Matches events against subscription filters
pub struct EventMatcher {
    filter: TraceFilter,
}

impl EventMatcher {
    /// Create a new matcher with the given filter
    pub fn new(filter: TraceFilter) -> Self {
        Self { filter }
    }

    /// Check if an event matches the filter
    pub fn matches(&self, event: &TraceEvent) -> bool {
        match event {
            TraceEvent::NewSpan(span) | TraceEvent::SpanUpdated(span) => self.matches_span(span),
            TraceEvent::TraceCompleted(trace) => {
                // Match service filter
                if let Some(ref service) = self.filter.service_name
                    && &trace.service_name != service
                {
                    return false;
                }

                // Match error filter
                if let Some(has_errors) = self.filter.has_errors
                    && trace.has_errors != has_errors
                {
                    return false;
                }

                true
            }
            TraceEvent::HealthUpdate(_) => {
                // Health updates always match (they're system-wide)
                true
            }
        }
    }

    /// Check if a span matches the filter
    fn matches_span(&self, span: &SpanEvent) -> bool {
        // Service name filter
        if let Some(ref service) = self.filter.service_name
            && &span.service_name != service
        {
            return false;
        }

        // Framework filter
        if let Some(ref framework) = self.filter.framework
            && &span.detected_framework != framework
        {
            return false;
        }

        // Agent name filter
        if let Some(ref agent) = self.filter.agent_name
            && span.gen_ai_agent_name.as_ref() != Some(agent)
        {
            return false;
        }

        // Time range filter
        if let Some(start) = self.filter.start_time_ns
            && span.start_time_ns < start
        {
            return false;
        }

        if let Some(end) = self.filter.end_time_ns
            && span.start_time_ns > end
        {
            return false;
        }

        // Duration filter
        if let Some(min_duration) = self.filter.min_duration_ns
            && let Some(duration) = span.duration_ns
            && duration < min_duration
        {
            return false;
        }

        // Error filter (status_code != 0 means error)
        if self.filter.has_errors == Some(true) && span.status_code == 0 {
            return false;
        }

        // Search filter
        if let Some(ref search) = self.filter.search {
            let search_lower = search.to_lowercase();
            let matches = span.span_name.to_lowercase().contains(&search_lower)
                || span.service_name.to_lowercase().contains(&search_lower)
                || span.trace_id.contains(&search_lower);
            if !matches {
                return false;
            }
        }

        true
    }

    /// Create a matcher that matches everything
    pub fn match_all() -> Self {
        Self { filter: TraceFilter::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otel::realtime::events::{HealthEvent, TraceCompletedEvent};

    fn create_test_span_event(
        service_name: &str,
        framework: &str,
        agent_name: Option<&str>,
        status_code: i8,
    ) -> SpanEvent {
        SpanEvent {
            trace_id: "trace123".to_string(),
            span_id: "span456".to_string(),
            parent_span_id: None,
            span_name: "test-span".to_string(),
            service_name: service_name.to_string(),
            detected_framework: framework.to_string(),
            detected_category: Some("llm".to_string()),
            start_time_ns: 1000000000,
            end_time_ns: Some(2000000000),
            duration_ns: Some(1000000000),
            status_code,
            gen_ai_agent_name: agent_name.map(String::from),
            gen_ai_tool_name: None,
            gen_ai_request_model: None,
            usage_input_tokens: None,
            usage_output_tokens: None,
        }
    }

    #[test]
    fn test_match_all_matches_everything() {
        let matcher = EventMatcher::match_all();

        let span_event = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));
        assert!(matcher.matches(&span_event));

        let health_event = TraceEvent::HealthUpdate(HealthEvent {
            status: "healthy".to_string(),
            disk_usage_percent: 50,
            pending_spans: 0,
            total_traces: 100,
        });
        assert!(matcher.matches(&health_event));
    }

    #[test]
    fn test_filter_by_service_name() {
        let filter =
            TraceFilter { service_name: Some("my-service".to_string()), ..Default::default() };
        let matcher = EventMatcher::new(filter);

        let matching =
            TraceEvent::NewSpan(create_test_span_event("my-service", "langchain", None, 0));
        let non_matching =
            TraceEvent::NewSpan(create_test_span_event("other-service", "langchain", None, 0));

        assert!(matcher.matches(&matching));
        assert!(!matcher.matches(&non_matching));
    }

    #[test]
    fn test_filter_by_framework() {
        let filter = TraceFilter { framework: Some("langchain".to_string()), ..Default::default() };
        let matcher = EventMatcher::new(filter);

        let matching = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));
        let non_matching =
            TraceEvent::NewSpan(create_test_span_event("svc", "llamaindex", None, 0));

        assert!(matcher.matches(&matching));
        assert!(!matcher.matches(&non_matching));
    }

    #[test]
    fn test_filter_by_agent_name() {
        let filter = TraceFilter { agent_name: Some("my-agent".to_string()), ..Default::default() };
        let matcher = EventMatcher::new(filter);

        let matching =
            TraceEvent::NewSpan(create_test_span_event("svc", "langchain", Some("my-agent"), 0));
        let non_matching = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));

        assert!(matcher.matches(&matching));
        assert!(!matcher.matches(&non_matching));
    }

    #[test]
    fn test_filter_by_errors() {
        let filter = TraceFilter { has_errors: Some(true), ..Default::default() };
        let matcher = EventMatcher::new(filter);

        let with_error = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 2)); // status_code != 0
        let without_error =
            TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));

        assert!(matcher.matches(&with_error));
        assert!(!matcher.matches(&without_error));
    }

    #[test]
    fn test_filter_by_search() {
        let filter = TraceFilter { search: Some("test".to_string()), ..Default::default() };
        let matcher = EventMatcher::new(filter);

        // span_name contains "test"
        let matching = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));
        assert!(matcher.matches(&matching));
    }

    #[test]
    fn test_search_case_insensitive() {
        let filter = TraceFilter { search: Some("TEST".to_string()), ..Default::default() };
        let matcher = EventMatcher::new(filter);

        let matching = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));
        assert!(matcher.matches(&matching)); // "test-span" should match "TEST"
    }

    #[test]
    fn test_trace_completed_event() {
        let filter = TraceFilter {
            service_name: Some("my-service".to_string()),
            has_errors: Some(true),
            ..Default::default()
        };
        let matcher = EventMatcher::new(filter);

        let matching = TraceEvent::TraceCompleted(TraceCompletedEvent {
            trace_id: "trace123".to_string(),
            service_name: "my-service".to_string(),
            span_count: 5,
            total_duration_ns: Some(1000000),
            total_input_tokens: Some(100),
            total_output_tokens: Some(50),
            has_errors: true,
        });

        let wrong_service = TraceEvent::TraceCompleted(TraceCompletedEvent {
            trace_id: "trace123".to_string(),
            service_name: "other-service".to_string(),
            span_count: 5,
            total_duration_ns: Some(1000000),
            total_input_tokens: Some(100),
            total_output_tokens: Some(50),
            has_errors: true,
        });

        assert!(matcher.matches(&matching));
        assert!(!matcher.matches(&wrong_service));
    }

    #[test]
    fn test_health_update_always_matches() {
        let filter = TraceFilter {
            service_name: Some("specific-service".to_string()),
            ..Default::default()
        };
        let matcher = EventMatcher::new(filter);

        let health = TraceEvent::HealthUpdate(HealthEvent {
            status: "healthy".to_string(),
            disk_usage_percent: 50,
            pending_spans: 10,
            total_traces: 1000,
        });

        // Health updates should always match regardless of filter
        assert!(matcher.matches(&health));
    }

    #[test]
    fn test_combined_filters() {
        let filter = TraceFilter {
            service_name: Some("my-service".to_string()),
            framework: Some("langchain".to_string()),
            has_errors: Some(true),
            ..Default::default()
        };
        let matcher = EventMatcher::new(filter);

        // All conditions match
        let matching =
            TraceEvent::NewSpan(create_test_span_event("my-service", "langchain", None, 2));
        assert!(matcher.matches(&matching));

        // Wrong service
        let wrong_service =
            TraceEvent::NewSpan(create_test_span_event("other", "langchain", None, 2));
        assert!(!matcher.matches(&wrong_service));

        // Wrong framework
        let wrong_framework =
            TraceEvent::NewSpan(create_test_span_event("my-service", "other", None, 2));
        assert!(!matcher.matches(&wrong_framework));

        // No error
        let no_error =
            TraceEvent::NewSpan(create_test_span_event("my-service", "langchain", None, 0));
        assert!(!matcher.matches(&no_error));
    }

    #[test]
    fn test_time_range_filter() {
        let filter = TraceFilter {
            start_time_ns: Some(500000000),
            end_time_ns: Some(1500000000),
            ..Default::default()
        };
        let matcher = EventMatcher::new(filter);

        // start_time_ns is 1000000000, within range
        let in_range = TraceEvent::NewSpan(create_test_span_event("svc", "langchain", None, 0));
        assert!(matcher.matches(&in_range));
    }
}
