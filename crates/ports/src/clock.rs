use std::fmt::Debug;

use chrono::{DateTime, Utc};

/// Source of wall-clock time for domain and application decisions.
pub trait Clock: Debug + Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
