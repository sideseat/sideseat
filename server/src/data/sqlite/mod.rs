//! Compatibility facade for the extracted SQLite adapter.

pub use sideseat_adapter_sqlite::*;

pub mod migrations {
    pub use sideseat_adapter_sqlite::migrations::*;
}

pub mod repositories {
    pub use sideseat_adapter_sqlite::repositories::*;
}

pub mod schema {
    pub use sideseat_adapter_sqlite::schema::*;
}
