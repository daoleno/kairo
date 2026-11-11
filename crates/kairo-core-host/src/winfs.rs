//! Windows file system rules of the SYSTEM side: directories created locked,
//! owners checked, reparse points (junctions, symbolic links) refused.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_NONE, FILE_GENERIC_WRITE,
};
use windows::core::{PCWSTR, PWSTR};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Whether `path` is a reparse point (a junction or a symbolic link), judged
/// without following it. A path that does not exist is not one.
pub fn is_reparse(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

/// The SID string of `path`'s owner.
pub fn owner_sid(path: &Path) -> Result<String, String> {
    unsafe {
        let mut owner = PSID::default();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let w = wide(path);
        let rc = GetNamedSecurityInfoW(
            PCWSTR(w.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut descriptor,
        );
        if rc.0 != 0 {
            return Err(format!("owner of {}: error {}", path.display(), rc.0));
        }
        let mut s = PWSTR::null();
        let r = ConvertSidToStringSidW(owner, &mut s);
        let text = r.map(|_| s.to_string().unwrap_or_default());
        if !s.is_null() {
            let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        }
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        text.map_err(|e| e.to_string())
    }
}

/// A directory only SYSTEM or Administrators own, that is not a reparse point.
pub fn is_trusted_dir(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    let a = meta.file_attributes();
    if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 || a & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return false;
    }
    matches!(owner_sid(path).as_deref(), Ok("S-1-5-18") | Ok("S-1-5-32-544"))
}

/// A regular file (no link) that only SYSTEM or Administrators own.
pub fn is_trusted_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    let a = meta.file_attributes();
    if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 || a & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return false;
    }
    matches!(owner_sid(path).as_deref(), Ok("S-1-5-18") | Ok("S-1-5-32-544"))
}

/// The first thing in `dir` that is not one of `allowed` names or is not a
/// trusted file, if any (the loader looks in a program's own folder first).
pub fn stray_in(dir: &Path, allowed: impl Fn(&str) -> bool) -> Option<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Some("(cannot list the folder)".into()) };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !allowed(&name) || !is_trusted_file(&e.path()) {
            return Some(name);
        }
    }
    None
}

/// Creates `path` (its parent must exist and be trusted) with this descriptor
/// in the same call, so the directory is never visible with another one.
pub fn create_dir_with(path: &Path, sddl: &str) -> Result<(), String> {
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let s: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(s.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(|e| format!("descriptor {sddl}: {e}"))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let w = wide(path);
        let r = CreateDirectoryW(PCWSTR(w.as_ptr()), Some(&attributes));
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        r.map_err(|e| format!("create {}: {e}", path.display()))
    }
}

/// Creates the empty file `path` (it must not exist) with this descriptor in
/// the same call.
pub fn create_file_with(path: &Path, sddl: &str) -> Result<(), String> {
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let s: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(s.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(|e| format!("descriptor {sddl}: {e}"))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let w = wide(path);
        let h = CreateFileW(
            PCWSTR(w.as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            Some(&attributes),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            None,
        );
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        let h = h.map_err(|e| format!("create {}: {e}", path.display()))?;
        let _ = CloseHandle(h);
        Ok(())
    }
}

/// Removes whatever is at `path`, a link included, without following links.
pub fn remove_any(path: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok(()) };
    let attrs = meta.file_attributes();
    let r = if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            std::fs::remove_dir(path) // the link only
        } else {
            std::fs::remove_dir_all(path) // std does not follow links below
        }
    } else {
        std::fs::remove_file(path)
    };
    r.map_err(|e| format!("remove {}: {e}", path.display()))
}

/// A directory of ours: when `path` is there and trusted it stays, otherwise it
/// is removed (a squat: Users may create folders in `%ProgramData%`) and created.
pub fn ensure_trusted_dir(path: &Path, sddl: &str) -> Result<(), String> {
    if is_trusted_dir(path) {
        return Ok(());
    }
    remove_any(path)?;
    create_dir_with(path, sddl)
}

/// Reads a file only a regular, non-link file may be read from, at most `max`
/// bytes. The path is opened without following a link, and what was opened must
/// be the path asked for.
pub fn read_plain_file(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
        return Err(format!("{} is not a plain file", path.display()));
    }
    if meta.len() > max {
        return Err(format!("{} is larger than {max} bytes", path.display()));
    }
    let mut buf = Vec::new();
    file.take(max + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > max {
        return Err(format!("{} is larger than {max} bytes", path.display()));
    }
    Ok(buf)
}

/// The descriptor of `path` as an SDDL string (for the acceptance checks).
pub fn dacl_of(path: &Path) -> Result<String, String> {
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let w = wide(path);
        let rc = GetNamedSecurityInfoW(
            PCWSTR(w.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | windows::Win32::Security::DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            &mut descriptor,
        );
        if rc.0 != 0 {
            return Err(format!("{}: error {}", path.display(), rc.0));
        }
        let mut s = PWSTR::null();
        let r = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | windows::Win32::Security::DACL_SECURITY_INFORMATION,
            &mut s,
            None,
        );
        let text = r.map(|_| s.to_string().unwrap_or_default());
        if !s.is_null() {
            let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        }
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        text.map_err(|e| e.to_string())
    }
}

/// Returns `path` back (used to keep imports honest in tests).
pub fn same(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| PathBuf::from(p.to_string_lossy().to_ascii_lowercase());
    n(a) == n(b)
}
