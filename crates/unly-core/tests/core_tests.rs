//! Integration tests for unly-core domain types.

use unly_core::{
    ids::ChatId,
    permissions::{Permission, PermissionSet, UserRole},
};

#[test]
fn ids_are_unique() {
    let a = ChatId::new();
    let b = ChatId::new();
    assert_ne!(a, b);
}

#[test]
fn admin_permissions_have_all_capabilities() {
    let perms = PermissionSet::admin();
    assert!(perms.has(&Permission::Chat));
    assert!(perms.has(&Permission::ExecuteTools));
    assert!(perms.has(&Permission::ExecutePrivilegedTools));
    assert!(perms.has(&Permission::ViewAuditLog));
    assert!(perms.has(&Permission::ManageJobs));
    assert!(perms.has(&Permission::ManagePlugins));
}

#[test]
fn basic_user_permissions_are_restricted() {
    let perms = PermissionSet::basic_user();
    assert!(perms.has(&Permission::Chat));
    assert!(!perms.has(&Permission::ExecuteTools));
    assert!(!perms.has(&Permission::ViewAuditLog));
    assert!(!perms.has(&Permission::ManageJobs));
    assert!(!perms.has(&Permission::ManagePlugins));
}

#[test]
fn user_role_display() {
    assert_eq!(UserRole::Admin.to_string(), "admin");
    assert_eq!(UserRole::User.to_string(), "user");
}
