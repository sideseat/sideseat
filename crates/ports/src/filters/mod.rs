//! The filter vocabulary: what a caller may ask for, independent of how any store answers it.
//!
//! **Both analytics adapters need this, so neither may own it.** It lived inside the DuckDB adapter with the
//! ClickHouse one importing it (`crate::data::duckdb::filters`), which the plan names as a defect and is one: a
//! shared type owned by one implementation, so the two adapters could not be separate crates and the
//! dependency direction between them was backwards.
//!
//! What stayed behind is the *rendering*. `Filter::to_sql` and its variants are DuckDB SQL, so they moved to
//! `duckdb::filters::sql` as an extension trait — the orphan rule forbids splitting an inherent `impl` across
//! crates, and rendering is dialect-specific anyway, which is why ClickHouse has its own renderer rather than
//! sharing one. Step 5 folds both into a single dialect seam.

pub mod columns;
mod parser;
mod types;

pub use parser::parse_filters;
pub use types::{
    BooleanOp, DatetimeOp, Filter, FilterError, NullOp, NumberOp, OptionsOp, StringOp,
};
