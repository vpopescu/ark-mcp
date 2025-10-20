//! Utilities for platform-specific secure filesystem operations.
//!
//! The helpers in this module ensure that files and directories are restricted
//! to the current user (owner) only. On Unix this uses POSIX file modes; on
//! Windows it uses ACLs to grant the current user full control and protect the
//! DACL from inheritance.
//!
//! These functions are intentionally conservative: they operate on existing
//! paths and return an error when the target does not exist. Callers that need
//! to create files or directories should create them first and then call these
//! helpers to harden permissions.

use anyhow::{Context, Result};
use std::path::Path;

#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

/// Ensure the directory at `dir_path` is accessible only by the current user.
///
/// On Unix this sets the mode to 0o700 (rwx------). On Windows this replaces
/// the DACL with an ACL that grants the current user full access and protects
/// the DACL from inheritance so parent ACLs do not re-introduce access for
/// other principals.
///
/// Returns an error if the path does not exist or if the platform-specific
/// permission changes fail.
pub fn set_secure_dir_permissions(dir_path: &Path) -> Result<()> {
    tracing::debug!(
        "Setting secure directory permissions for: {}",
        dir_path.display()
    );

    if !dir_path.exists() {
        return Err(anyhow::anyhow!(
            "directory does not exist: {}",
            dir_path.display()
        ));
    }

    #[cfg(unix)]
    {
        // On Unix: set permissions to 0700 (rwx------)
        let metadata = fs::metadata(dir_path)
            .with_context(|| format!("reading metadata for {}", dir_path.display()))?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(dir_path, permissions)
            .with_context(|| format!("setting unix permissions on {}", dir_path.display()))?;
        tracing::debug!(
            "Set Unix directory permissions to 0700 (rwx------) for: {}",
            dir_path.display()
        );
    }

    #[cfg(windows)]
    {
        let p = dir_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid path string: {}", dir_path.display()))?;
        restrict_to_owner_readwrite(p)
            .with_context(|| format!("setting windows ACL on {}", dir_path.display()))?;
    }

    Ok(())
}

/// Ensure the file at `file_path` is readable and writable only by the current
/// user (owner) and not accessible to other principals.
///
/// On Unix this sets the mode to 0o600 (rw-------). On Windows this replaces
/// the DACL with an ACL that grants the current user full access (including
/// delete) and protects the DACL from inheritance.
///
/// Returns an error if the file does not exist or if the permission changes
/// fail.
pub fn set_secure_file_permissions(file_path: &Path) -> Result<()> {
    tracing::debug!(
        "Setting secure file permissions for: {}",
        file_path.display()
    );

    if !file_path.exists() {
        return Err(anyhow::anyhow!(
            "file does not exist: {}",
            file_path.display()
        ));
    }

    #[cfg(unix)]
    {
        // On Unix: set permissions to 0600 (rw-------)
        let metadata = fs::metadata(file_path)
            .with_context(|| format!("reading metadata for {}", file_path.display()))?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(file_path, permissions)
            .with_context(|| format!("setting unix permissions on {}", file_path.display()))?;
        tracing::debug!(
            "Set Unix file permissions to 0600 (rw-------) for: {}",
            file_path.display()
        );
    }

    #[cfg(windows)]
    {
        let p = file_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid path string: {}", file_path.display()))?;
        restrict_to_owner_readwrite(p)
            .with_context(|| format!("setting windows ACL on {}", file_path.display()))?;
    }

    Ok(())
}

/// Replace the DACL on `path` with an owner-only ACL granting full control.
///
/// Platform: Windows only (`#[cfg(windows)]`).
///
/// This function replaces the DACL for the given file or directory with a
/// new ACL that grants the current process user full control (read, write,
/// modify and delete). The DACL is marked as protected to prevent inheritance
/// of ACEs from parent objects which could re-introduce access for other
/// principals. The function performs a best-effort check for an existing ACE
/// that already grants full access to the current user and will skip modifying
/// the DACL if it finds one.
///
/// Notes:
/// - The caller must ensure the target path exists; this function returns an
///   error if it does not.
/// - Replacing a DACL is a sensitive operation and may require elevated
///   privileges in restricted environments. Failure returns an error that
///   includes the underlying Win32 error code when available.
/// - Memory allocated by Windows APIs is freed before returning.
#[cfg(windows)]
fn restrict_to_owner_readwrite(path: &str) -> anyhow::Result<()> {
    tracing::debug!(
        "Setting Windows ACL permissions for owner full access: {}",
        path
    );

    use windows::{
        Win32::Foundation::{HANDLE, HLOCAL, LocalFree},
        Win32::Security::Authorization::*,
        Win32::Security::*,
        Win32::Storage::FileSystem::*,
        Win32::System::Threading::*,
        core::*,
    };

    unsafe {
        // Get current user SID from the process token.
        let mut token = HANDLE(std::ptr::null_mut());
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| anyhow::anyhow!("OpenProcessToken failed: {}", e))?;

        let mut token_info = vec![0u8; 1024];
        let mut ret_len = 0u32;
        GetTokenInformation(
            token,
            TokenUser,
            Some(token_info.as_mut_ptr() as _),
            token_info.len() as u32,
            &mut ret_len,
        )
        .map_err(|e| anyhow::anyhow!("GetTokenInformation failed: {}", e))?;
        let user_sid = (*(token_info.as_ptr() as *const TOKEN_USER)).User.Sid;

        // Prepare the path and attempt to read the existing DACL. If that fails
        // we will still attempt to create and apply an owner-only ACL.
        let wide_path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut p_dacl = std::ptr::null_mut();
        let p_sd = std::ptr::null_mut();
        let status = GetNamedSecurityInfoW(
            PCWSTR(wide_path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut p_dacl),
            None,
            p_sd,
        );
        if status.0 != 0 {
            tracing::warn!(
                "GetNamedSecurityInfoW failed for {} code={} - will replace DACL",
                path,
                status.0
            );
        } else if !p_dacl.is_null() {
            // Check whether an ACE already grants full access to the current
            // user; if so, we are done.
            let mut access_granted = false;
            let ace_count = (*p_dacl).AceCount;
            for i in 0..ace_count {
                let mut p_ace: *mut std::ffi::c_void = std::ptr::null_mut();
                if GetAce(p_dacl, i as u32, &mut p_ace).is_ok() {
                    let ace = *(p_ace as *const ACCESS_ALLOWED_ACE);
                    let sid = PSID(&ace.SidStart as *const u32 as *mut std::ffi::c_void);
                    if EqualSid(sid, user_sid).is_ok() {
                        let mask = ace.Mask;
                        if mask & FILE_ALL_ACCESS.0 == FILE_ALL_ACCESS.0 {
                            access_granted = true;
                            break;
                        }
                    }
                }
            }

            if access_granted {
                tracing::debug!(
                    "Windows ACL already grants full access to owner for {}",
                    path
                );
                return Ok(());
            }
        }

        // Build an EXPLICIT_ACCESS entry that grants the current user full
        // control, and then create a fresh DACL with only that entry.
        let allow_owner_ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS.0,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: PWSTR(user_sid.0 as _),
            },
        };

        let entries = [allow_owner_ea];
        let mut new_dacl = std::ptr::null_mut();
        let create_rc = SetEntriesInAclW(Some(&entries), None, &mut new_dacl);
        if create_rc.0 != 0 {
            return Err(anyhow::anyhow!(
                "SetEntriesInAclW failed code={}",
                create_rc.0
            ));
        }

        // Apply the ACL and mark it protected so parent ACLs are not inherited.
        let result = SetNamedSecurityInfoW(
            PCWSTR(wide_path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(new_dacl),
            None,
        );
        if result.0 != 0 {
            // Best-effort cleanup of the allocated ACL before returning.
            if !new_dacl.is_null() {
                // new_dacl is allocated by SetEntriesInAclW - free it with LocalFree
                let h = HLOCAL(new_dacl as *mut core::ffi::c_void);
                let _ = LocalFree(Some(h));
            }
            return Err(anyhow::anyhow!(
                "SetNamedSecurityInfoW failed code={}",
                result.0
            ));
        }

        // Free the ACL allocated by SetEntriesInAclW - it is no longer needed.
        if !new_dacl.is_null() {
            let h = HLOCAL(new_dacl as *mut core::ffi::c_void);
            let _ = LocalFree(Some(h));
        }

        tracing::debug!("Successfully updated Windows ACL for owner: {}", path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[cfg(unix)]
    #[test]
    fn unix_set_secure_file_permissions_makes_owner_only() -> Result<()> {
        let td = tempfile::tempdir()?;
        let file_path = td.path().join("secret.db");
        std::fs::File::create(&file_path)?;

        // Apply secure permissions
        set_secure_file_permissions(&file_path)?;

        let md = fs::metadata(&file_path)?;
        let mode = md.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file mode should be 0600");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_set_secure_dir_permissions_makes_owner_only() -> Result<()> {
        let td = tempfile::tempdir()?;
        let dir_path = td.path().join("ark-dir");
        fs::create_dir_all(&dir_path)?;

        set_secure_dir_permissions(&dir_path)?;

        let md = fs::metadata(&dir_path)?;
        let mode = md.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "dir mode should be 0700");
        Ok(())
    }

    #[test]
    fn missing_paths_return_error() {
        let td = tempfile::tempdir().expect("tmpdir");
        let missing = td.path().join("nope");
        assert!(set_secure_file_permissions(&missing).is_err());
        assert!(set_secure_dir_permissions(&missing).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_set_secure_file_permissions_is_idempotent() -> Result<()> {
        let td = tempfile::tempdir()?;
        let file_path = td.path().join("secret.db");
        std::fs::File::create(&file_path)?;

        // First application
        set_secure_file_permissions(&file_path)?;
        // Second application should not error and should be idempotent
        set_secure_file_permissions(&file_path)?;

        // Also call the lower-level helper directly
        let p = file_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid path"))?;
        restrict_to_owner_readwrite(p)?;
        Ok(())
    }
}
