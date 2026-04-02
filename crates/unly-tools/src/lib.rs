//! Secure tool execution framework for the unly agent platform.
//!
//! Implements a typed tool registry with:
//! - Risk classification (Safe, Privileged, Dangerous)
//! - Allowlist/denylist enforcement
//! - Execution policy (approval gates for privileged/dangerous tools)
//! - Timeout enforcement
//! - Structured result capture
//! - Audit logging for every invocation

pub mod builtin;
pub mod error;
pub mod policy;
pub mod registry;

pub use error::ToolError;
pub use policy::{ExecutionPolicy, ToolPolicy};
pub use registry::ToolRegistry;
