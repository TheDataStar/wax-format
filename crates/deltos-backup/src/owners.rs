//! Owners by name, not number.
//!
//! A fresh box makes its system users with `systemd-sysusers`, which picks
//! the numbers itself, so `deltos-identity` may be 998 on one box and 995 on
//! another. A backup records each file's owner and group **by name** and a
//! restore looks the names up on the box it runs on.

use std::ffi::{CStr, CString};

pub fn user_name(uid: u32) -> Option<String> {
    // SAFETY: getpwuid_r with a caller-owned buffer; the result points into it.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        Some(CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned())
    }
}

pub fn group_name(gid: u32) -> Option<String> {
    // SAFETY: as above.
    unsafe {
        let mut gr: libc::group = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::group = std::ptr::null_mut();
        if libc::getgrgid_r(gid, &mut gr, buf.as_mut_ptr(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        Some(CStr::from_ptr(gr.gr_name).to_string_lossy().into_owned())
    }
}

pub fn uid_of(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    // SAFETY: as above.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwnam_r(c.as_ptr(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        Some(pw.pw_uid)
    }
}

pub fn gid_of(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    // SAFETY: as above.
    unsafe {
        let mut gr: libc::group = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::group = std::ptr::null_mut();
        if libc::getgrnam_r(c.as_ptr(), &mut gr, buf.as_mut_ptr(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        Some(gr.gr_gid)
    }
}

pub fn chown(path: &std::path::Path, uid: u32, gid: u32) -> std::io::Result<()> {
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}
