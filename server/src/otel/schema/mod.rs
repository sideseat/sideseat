//! Arrow/Parquet schema definitions

mod attributes;
mod event;
pub mod span;
mod unknown;

pub use attributes::AttributeValue;
pub use event::EventSchema;
pub use span::{SpanSchema, to_record_batch};
pub use unknown::{UnknownFields, is_known_resource_attr, is_known_span_attr};
