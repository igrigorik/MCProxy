//! Proxy middleware implementations that operate on aggregated results.

pub mod description_enricher;
pub mod tool_search;

pub use description_enricher::DescriptionEnricherFactory;
pub use tool_search::ToolSearchMiddlewareFactory;