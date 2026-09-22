//! Compatibility facade for the extracted PostgreSQL adapter.

pub use sideseat_adapter_postgres::*;

pub mod migrations {
    pub use sideseat_adapter_postgres::migrations::*;
}

pub mod repositories {
    pub use sideseat_adapter_postgres::repositories::*;
}

pub mod schema {
    pub use sideseat_adapter_postgres::schema::*;
}

#[cfg(test)]
mod parity_tests;
