//! Agent runtime for the unly agent platform.
//!
//! Implements the core agentic loop:
//! 1. Receive user message
//! 2. Load conversation context + relevant memories
//! 3. Send to LLM provider
//! 4. Handle tool calls (with policy enforcement)
//! 5. Continue loop until done
//! 6. Store assistant response + update memory

pub mod context;
pub mod error;
pub mod runtime;
pub mod subagent;

pub use context::AgentContext;
pub use error::AgentError;
pub use runtime::{AgentResponse, AgentRuntime, AgentRuntimeConfig};
pub use subagent::{SubagentHandle, SubagentRequest};
