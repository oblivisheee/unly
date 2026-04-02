use unly_core::permissions::PermissionSet;

/// Check whether a Telegram user ID is in the admin list.
pub fn is_admin(telegram_user_id: i64, admin_ids: &[i64]) -> bool {
    admin_ids.contains(&telegram_user_id)
}

/// Check whether a user is allowed to use the bot.
pub fn is_allowed(
    telegram_user_id: i64,
    admin_ids: &[i64],
    allowed_ids: &[i64],
    open_access: bool,
) -> bool {
    if is_admin(telegram_user_id, admin_ids) {
        return true;
    }
    if open_access {
        return true;
    }
    allowed_ids.contains(&telegram_user_id)
}

/// Build a PermissionSet for a Telegram user.
pub fn build_permissions(telegram_user_id: i64, admin_ids: &[i64]) -> PermissionSet {
    if is_admin(telegram_user_id, admin_ids) {
        PermissionSet::admin()
    } else {
        PermissionSet::basic_user()
    }
}
