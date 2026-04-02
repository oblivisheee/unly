//! Agent runtime for the unly agent platform.
//!
//! Implements the core agentic loop:
//! 1. Receive user message
//! 2. Load conversation context + relevant memories
//! 3. Send to LLM provider
//! 4. Handle tool calls (with policy enforcement) — "thinking phase"
//! 5. Continue loop until done
//! 6. Stream the final response — "response phase"
//! 7. Store assistant response + update memory

pub mod context;
pub mod error;
pub mod runtime;
pub mod subagent;

pub use context::{AgentContext, ThinkingStep};
pub use error::AgentError;
pub use runtime::{AgentResponse, AgentRuntime, AgentRuntimeConfig, StreamEvent};
pub use subagent::{SubagentHandle, SubagentRequest};
