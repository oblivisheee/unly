use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// User role in the platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
    ReadOnly,
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserRole::Admin => write!(f, "admin"),
            UserRole::User => write!(f, "user"),
            UserRole::ReadOnly => write!(f, "readonly"),
        }
    }
}

/// Named permissions that can be granted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Use the bot for basic conversations.
    Chat,
    /// Execute tools.
    ExecuteTools,
    /// Execute privileged/destructive tools.
    ExecutePrivilegedTools,
    /// Manage plugins.
    ManagePlugins,
    /// Manage scheduled jobs.
    ManageJobs,
    /// Administer the system.
    Admin,
    /// View audit logs.
    ViewAuditLog,
    /// Manage webhooks.
    ManageWebhooks,
    /// Use subagents.
    UseSubagents,
}

/// A set of permissions for a subject.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionSet {
    pub permissions: HashSet<Permission>,
}

impl PermissionSet {
    pub fn new(permissions: impl IntoIterator<Item = Permission>) -> Self {
        Self {
            permissions: permissions.into_iter().collect(),
        }
    }

    pub fn admin() -> Self {
        Self::new([
            Permission::Chat,
            Permission::ExecuteTools,
            Permission::ExecutePrivilegedTools,
            Permission::ManagePlugins,
            Permission::ManageJobs,
            Permission::Admin,
            Permission::ViewAuditLog,
            Permission::ManageWebhooks,
            Permission::UseSubagents,
        ])
    }

    pub fn basic_user() -> Self {
        Self::new([Permission::Chat])
    }

    pub fn has(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission)
    }

    pub fn check(&self, permission: &Permission) -> crate::Result<()> {
        if self.has(permission) {
            Ok(())
        } else {
            Err(crate::Error::PermissionDenied(format!(
                "permission required: {:?}",
                permission
            )))
        }
    }
}

/// A user record in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: crate::ids::UserId,
    pub telegram_user_id: Option<i64>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub role: UserRole,
    pub permissions: PermissionSet,
    pub is_blocked: bool,
    pub created_at: crate::types::Timestamp,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == UserRole::Admin
    }
}
