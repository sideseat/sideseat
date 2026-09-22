use chrono::{DateTime, Utc};
use sideseat_ports::clock::Clock;

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
