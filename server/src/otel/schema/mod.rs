//! Arrow/Parquet schema definitions

mod attributes;
mod event;
mod semconv;
pub mod span;

pub use attributes::AttributeValue;
pub use event::EventSchema;
pub use semconv::{UnknownFields, is_known_resource_attr, is_known_span_attr};
pub use span::{SpanSchema, to_record_batch};
