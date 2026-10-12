//! What a section's conditions may read.

/// What a section's conditions may read, beyond the span's name and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Readable {
    pub span_name: bool,
    pub attributes: bool,
    pub scope: bool,
    pub scope_version: bool,
    pub resource: bool,
    /// The marks a stored row carries. A section that can read the span's attributes asks its question directly,
    /// so this is given only where the attributes are gone.
    pub marks: bool,
}

impl Readable {
    /// The span's attributes only: span facts.
    pub const ATTRIBUTES: Self = Self {
        span_name: false,
        attributes: true,
        scope: false,
        scope_version: false,
        resource: false,
        marks: false,
    };
    /// The span's name and attributes: classification and span-field sources.
    pub const SPAN: Self = Self {
        span_name: true,
        attributes: true,
        scope: false,
        scope_version: false,
        resource: false,
        marks: false,
    };
    /// And the instrumentation scope: message-rule gates.
    pub const SPAN_AND_SCOPE: Self = Self {
        span_name: true,
        attributes: true,
        scope: true,
        scope_version: false,
        resource: false,
        marks: false,
    };
    /// Everything: detection.
    pub const ALL: Self = Self {
        span_name: true,
        attributes: true,
        scope: true,
        scope_version: false,
        resource: true,
        marks: false,
    };
    /// What ingest hands a span mark (`span_marks::SpanMarks::marks_of`): the span's name, attributes and
    /// instrumentation scope, version included - and no resource, which the mark is decided without.
    pub const MARKS: Self = Self {
        span_name: true,
        attributes: true,
        scope: true,
        scope_version: true,
        resource: false,
        marks: false,
    };
    /// A block being read: its span's name and instrumentation scope, version included - what every block of a
    /// read carries - and neither attributes nor marks.
    pub const BLOCK: Self = Self {
        span_name: true,
        attributes: false,
        scope: true,
        scope_version: true,
        resource: false,
        marks: false,
    };
    /// A stored row being projected: its span name and instrumentation scope, version included, and none of
    /// its attributes - projection reads the row, not the span.
    pub const PROJECTION: Self = Self {
        span_name: true,
        attributes: false,
        scope: true,
        scope_version: true,
        resource: false,
        marks: true,
    };
}
