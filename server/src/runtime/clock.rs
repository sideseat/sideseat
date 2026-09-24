//! Production time source for services that depend on the clock port.

use chrono::{DateTime, Utc};
use sideseat_ports::clock::Clock;

/// Clock backed by the system's current UTC time.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
