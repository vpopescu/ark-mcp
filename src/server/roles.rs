//! Role and group mapping for authentication providers.

use serde::{Deserialize, Serialize};

/// Application-level roles used by the server's authorization checks.
///
/// The list is intentionally small and can be extended to include more
/// fine-grained roles as the authorization model grows. Use `UserRoles` to
/// convert provider-specific group information into a vector of these roles.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Administrative user. Has elevated privileges and can perform
    /// actions on all loaded plugins.
    Admin,
    /// Regular authenticated user. This is the default role granted to any
    /// authenticated identity that does not match an admin group. A user
    /// can only see and access plugins they own.
    User,
    // Add more roles as needed
}
