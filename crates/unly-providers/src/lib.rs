//! LLM provider abstraction layer for the unly agent platform.
//!
//! Providers:
//! - GitHub Copilot (primary default)
//! - OpenAI-compatible (secondary / alternative)

pub mod copilot;
pub mod error;
pub mod openai_compat;
pub mod registry;

pub use error::ProviderError;
pub use registry::ProviderRegistry;
