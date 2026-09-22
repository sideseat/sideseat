//! Compatibility facade for shared SQL rendering.

pub use sideseat_query_sql::*;

pub mod display {
    pub use sideseat_query_sql::display::*;
}

pub mod order {
    pub use sideseat_query_sql::order::*;
}
