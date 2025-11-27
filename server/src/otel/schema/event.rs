//! Span event schema for Arrow/Parquet

use arrow::datatypes::{DataType, Field, Schema};
use std::sync::Arc;

/// Arrow/Parquet schema for span events
pub struct EventSchema;

impl EventSchema {
    /// Get Arrow schema for span events
    pub fn arrow_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("span_id", DataType::Utf8, false),
            Field::new("trace_id", DataType::Utf8, false),
            Field::new("event_time_ns", DataType::Int64, false),
            Field::new("event_name", DataType::Utf8, false),
            Field::new("attributes_json", DataType::Utf8, false),
        ]))
    }
}
