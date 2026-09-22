//! Compatibility facade for the extracted ClickHouse adapter.

pub use sideseat_adapter_clickhouse::*;

pub mod consistency {
    pub use sideseat_adapter_clickhouse::consistency::*;
}

pub mod repositories {
    pub use sideseat_adapter_clickhouse::repositories::*;
}

pub mod schema {
    pub use sideseat_adapter_clickhouse::schema::*;
}

#[cfg(test)]
mod released_v2;

#[cfg(test)]
mod parity_tests;
