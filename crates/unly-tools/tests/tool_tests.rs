//! Tests for the tool execution framework.

use serde_json::json;
use unly_core::tool::{ToolContext, ToolRisk};
use unly_tools::ToolRegistry;
use unly_tools::policy::ExecutionPolicy;

fn default_policy() -> ExecutionPolicy {
    ExecutionPolicy {
        require_approval_for_privileged: true,
        require_approval_for_dangerous: true,
        max_execution_seconds: 10,
        max_concurrent: 4,
        shell_allowlist: vec![],
    }
}

fn make_ctx() -> ToolContext {
    ToolContext {
        tool_call_id: "test-call-1".to_string(),
        user_id: None,
        chat_id: None,
        agent_id: None,
        subagent_depth: 0,
    }
}

#[tokio::test]
async fn fs_read_blocks_path_traversal() {
    let mut registry = ToolRegistry::new(default_policy(), vec![], vec![]);
    registry.register(unly_tools::builtin::FsReadTool);

    let result = registry
        .execute(
            "fs_read",
            json!({"path": "../../etc/passwd"}),
            make_ctx(),
            false,
            false,
        )
        .await
        .expect("execute should not fail at policy level");

    assert!(
        result.is_error,
        "path traversal should produce an error result"
    );
    assert!(
        result.stderr.contains("traversal"),
        "error should mention traversal: {}",
        result.stderr
    );
}

#[tokio::test]
async fn privileged_tool_requires_approval() {
    let mut registry = ToolRegistry::new(default_policy(), vec![], vec![]);
    registry.register(unly_tools::builtin::HttpPostTool::new());

    let result = registry
        .execute(
            "http_post",
            json!({"url": "https://example.com", "body": {}}),
            make_ctx(),
            false, // not approved
            false,
        )
        .await;

    assert!(
        matches!(result, Err(unly_core::Error::ToolDenied { .. })),
        "privileged tool without approval should be denied"
    );
}

#[tokio::test]
async fn privileged_tool_runs_when_approved() {
    let mut registry = ToolRegistry::new(default_policy(), vec![], vec![]);
    registry.register(unly_tools::builtin::HttpGetTool::new());

    // http_get is Safe, not Privileged — should run without approval
    let result = registry
        .execute(
            "http_get",
            json!({"url": "https://httpbin.org/status/200"}),
            make_ctx(),
            false,
            false,
        )
        .await;

    // Result may succeed or fail (network), but should not be denied
    assert!(
        !matches!(result, Err(unly_core::Error::ToolDenied { .. })),
        "safe tool should not be denied: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn tool_not_found_returns_error() {
    let registry = ToolRegistry::new(default_policy(), vec![], vec![]);

    let result = registry
        .execute("nonexistent_tool", json!({}), make_ctx(), false, false)
        .await;

    assert!(
        matches!(result, Err(unly_core::Error::ToolNotFound(_))),
        "unknown tool should return ToolNotFound error"
    );
}

#[tokio::test]
async fn disabled_tool_returns_not_found() {
    let mut registry = ToolRegistry::new(
        default_policy(),
        vec![],
        vec!["fs_read".to_string()], // explicitly disabled
    );
    registry.register(unly_tools::builtin::FsReadTool);

    let result = registry
        .execute("fs_read", json!({"path": "/tmp"}), make_ctx(), false, false)
        .await;

    assert!(
        matches!(result, Err(unly_core::Error::ToolNotFound(_))),
        "disabled tool should appear not found"
    );
}

#[test]
fn execution_policy_needs_approval() {
    let policy = ExecutionPolicy {
        require_approval_for_privileged: true,
        require_approval_for_dangerous: true,
        ..Default::default()
    };
    assert!(!policy.needs_approval(&ToolRisk::Safe));
    assert!(policy.needs_approval(&ToolRisk::Privileged));
    assert!(policy.needs_approval(&ToolRisk::Dangerous));
}

#[test]
fn shell_allowlist_enforcement() {
    let policy = ExecutionPolicy {
        shell_allowlist: vec!["^ls ".to_string(), "^echo ".to_string()],
        ..Default::default()
    };
    assert!(policy.is_shell_allowed("ls -la /tmp"));
    assert!(policy.is_shell_allowed("echo hello world"));
    assert!(!policy.is_shell_allowed("rm -rf /"));
    assert!(!policy.is_shell_allowed("curl https://evil.com"));
}

#[test]
fn empty_shell_allowlist_denies_all() {
    let policy = ExecutionPolicy {
        shell_allowlist: vec![],
        ..Default::default()
    };
    assert!(!policy.is_shell_allowed("ls"));
    assert!(!policy.is_shell_allowed("echo hi"));
}

#[tokio::test]
async fn approved_bash_bypasses_allowlist() {
    let policy = ExecutionPolicy {
        require_approval_for_privileged: true,
        require_approval_for_dangerous: true,
        max_execution_seconds: 10,
        max_concurrent: 4,
        shell_allowlist: vec!["^ls(\\s|$)".to_string()],
    };
    let mut registry = ToolRegistry::new(policy, vec![], vec![]);
    registry.register(unly_tools::builtin::BashTool::new(
        vec!["^ls(\\s|$)".to_string()],
        None,
        true,
    ));

    let result = registry
        .execute(
            "bash",
            json!({"command": "echo approved-run"}),
            make_ctx(),
            true,
            false,
        )
        .await
        .expect("approved execution should not be denied");

    assert!(
        !result.is_error,
        "approved bash should run: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("approved-run"),
        "unexpected stdout: {}",
        result.stdout
    );
}
