//! Backend selection and lifecycle owned by the composition root.

mod analytics;
mod transactional;

pub use analytics::AnalyticsService;
pub use transactional::TransactionalService;
