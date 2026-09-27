//! NFR-14 on Windows: replace the credential file's inherited DACL with one
//! explicit entry granting only the current user full control, so the file is
//! owner-only regardless of the parent directory's permissions.
//!
//! `unsafe` is required because the Win32 security API is FFI. Every handle
//! and buffer is released on every path, and the SID buffer outlives the ACE
//! that points into it. This module is the credential path's documented
//! exemption from the crate's `deny(unsafe_code)` (the pty module is the
//! other).

use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE,
    PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Apply an owner-only DACL to `path`.
pub fn set_owner_only(path: &std::path::Path) -> Result<(), String> {
    // SAFETY: the calls below are the documented Win32 sequence for
    // setting a file DACL. `token` is closed and `acl` is freed on every
    // exit; `buffer` (which owns the SID the ACE points at) lives until
    // the end of the block, after `SetEntriesInAclW` has copied the SID.
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(format!(
                "OpenProcessToken failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
        if len == 0 {
            CloseHandle(token);
            return Err(format!(
                "GetTokenInformation size failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut buffer = vec![0u64; len.div_ceil(8) as usize];
        if GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len) == 0 {
            CloseHandle(token);
            return Err(format!(
                "GetTokenInformation failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let sid = (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid;

        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: sid.cast::<u16>(),
            },
        };
        let mut acl: *mut ACL = ptr::null_mut();
        let entries = SetEntriesInAclW(1, &entry, ptr::null(), &mut acl);
        CloseHandle(token);
        if entries != 0 {
            return Err(format!("SetEntriesInAclW failed: error {entries}"));
        }

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let result = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        );
        LocalFree(acl.cast());
        if result != 0 {
            return Err(format!("SetNamedSecurityInfoW failed: error {result}"));
        }
    }
    Ok(())
}

/// Whether `path`'s DACL is protected (inheritance disabled). The write
/// sets `PROTECTED_DACL_SECURITY_INFORMATION`, so this is the cheap,
/// verifiable half of "the file no longer inherits the directory ACL".
#[cfg(test)]
pub fn dacl_is_protected(path: &std::path::Path) -> Result<bool, String> {
    use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
    use windows_sys::Win32::Security::{SE_DACL_PROTECTED, SECURITY_DESCRIPTOR};

    // SAFETY: GetNamedSecurityInfoW allocates the descriptor with
    // LocalAlloc; LocalFree releases it, and the control field is read
    // before the free.
    unsafe {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sd: *mut core::ffi::c_void = ptr::null_mut();
        let result = GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        );
        if result != 0 {
            return Err(format!("GetNamedSecurityInfoW failed: error {result}"));
        }
        let control = (*sd.cast::<SECURITY_DESCRIPTOR>()).Control;
        LocalFree(sd.cast());
        Ok(control & SE_DACL_PROTECTED != 0)
    }
}
