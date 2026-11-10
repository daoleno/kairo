//! The few Win32 calls Kairo needs, wrapped so the rest of the engine stays
//! plain Rust: the registry (system proxy, start at login), the process list
//! and the TCP table (bypass detection, who holds a port), and process
//! lifetime (stop the core, wait for the old Kairo).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPEN_CREATE_OPTIONS, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// `HKEY_CURRENT_USER` subkeys Kairo touches.
pub const INTERNET_SETTINGS: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn err(what: &str, code: windows::Win32::Foundation::WIN32_ERROR) -> String {
    format!("{what}: Windows error {}", code.0)
}

/// An open `HKEY_CURRENT_USER` subkey, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(subkey: &str, write: bool) -> Result<Key, String> {
    let mut key = HKEY::default();
    let access = if write {
        KEY_READ | KEY_SET_VALUE
    } else {
        KEY_READ
    };
    let name = HSTRING::from(subkey);
    // SAFETY: valid string and out pointer.
    let code = unsafe {
        if write {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &name,
                None,
                PCWSTR::null(),
                REG_OPEN_CREATE_OPTIONS(0),
                access,
                None,
                &mut key,
                None,
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, &name, None, access, &mut key)
        }
    };
    if code == ERROR_SUCCESS {
        Ok(Key(key))
    } else {
        Err(err(subkey, code))
    }
}

fn query(key: &Key, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let name = HSTRING::from(name);
    let mut kind = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    // SAFETY: the first call only asks for the size.
    let code =
        unsafe { RegQueryValueExW(key.0, &name, None, Some(&mut kind), None, Some(&mut len)) };
    if code != ERROR_SUCCESS {
        return None;
    }
    let mut data = vec![0u8; len as usize];
    // SAFETY: `data` has `len` bytes.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            &name,
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut len),
        )
    };
    (code == ERROR_SUCCESS).then(|| {
        data.truncate(len as usize);
        (kind, data)
    })
}

/// A string value of `HKEY_CURRENT_USER\<subkey>`; `None` when absent.
pub fn reg_get_string(subkey: &str, name: &str) -> Option<String> {
    let key = open(subkey, false).ok()?;
    let (kind, data) = query(&key, name)?;
    if kind != REG_SZ && kind != windows::Win32::System::Registry::REG_EXPAND_SZ {
        return None;
    }
    let wide: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|c| *c != 0)
        .collect();
    Some(String::from_utf16_lossy(&wide))
}

/// A DWORD value of `HKEY_CURRENT_USER\<subkey>`; `None` when absent.
pub fn reg_get_dword(subkey: &str, name: &str) -> Option<u32> {
    let key = open(subkey, false).ok()?;
    let (kind, data) = query(&key, name)?;
    (kind == REG_DWORD && data.len() >= 4)
        .then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

pub fn reg_set_string(subkey: &str, name: &str, value: &str) -> Result<(), String> {
    let key = open(subkey, true)?;
    let mut bytes: Vec<u8> = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    bytes.shrink_to_fit();
    // SAFETY: a valid key and buffer.
    let code = unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, REG_SZ, Some(&bytes)) };
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(err(name, code))
    }
}

pub fn reg_set_dword(subkey: &str, name: &str, value: u32) -> Result<(), String> {
    let key = open(subkey, true)?;
    // SAFETY: a valid key and buffer.
    let code = unsafe {
        RegSetValueExW(
            key.0,
            &HSTRING::from(name),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(err(name, code))
    }
}

/// Removes a value; absent is fine.
pub fn reg_delete(subkey: &str, name: &str) -> Result<(), String> {
    let Ok(key) = open(subkey, true) else {
        return Ok(());
    };
    // SAFETY: a valid key.
    let code = unsafe { RegDeleteValueW(key.0, &HSTRING::from(name)) };
    // ERROR_FILE_NOT_FOUND: nothing to delete.
    if code == ERROR_SUCCESS || code.0 == 2 {
        Ok(())
    } else {
        Err(err(name, code))
    }
}

/// Tells WinINet (and so every program that reads the system proxy) that the
/// settings changed.
pub fn notify_proxy_changed() {
    use windows::Win32::Networking::WinInet::{
        INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
    };
    // SAFETY: null handle and buffer are what these two options take.
    unsafe {
        let _ = InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0);
        let _ = InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0);
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle this wrapper owns.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open_process(
    pid: u32,
    access: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS,
) -> Option<Handle> {
    // SAFETY: plain call; the handle is owned by the wrapper.
    unsafe { OpenProcess(access, false, pid) }.ok().map(Handle)
}

/// The full path of a process's executable; `None` for a process that cannot
/// be opened (another user's, a protected one).
pub fn image_path(pid: u32) -> Option<String> {
    let process = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` has `len` units.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Whether a process with this id is running.
pub fn pid_alive(pid: u32) -> bool {
    let Some(process) = open_process(pid, PROCESS_SYNCHRONIZE) else {
        return false;
    };
    // SAFETY: a valid handle; zero timeout polls.
    unsafe { WaitForSingleObject(process.0, 0) }.0 == 0x102 // WAIT_TIMEOUT
}

/// Waits until the process ends or `ms` pass; whether it ended. A process
/// that cannot be opened counts as ended.
pub fn wait_for_exit(pid: u32, ms: u32) -> bool {
    let Some(process) = open_process(pid, PROCESS_SYNCHRONIZE) else {
        return true;
    };
    // SAFETY: a valid handle.
    unsafe { WaitForSingleObject(process.0, ms) }.0 == 0 // WAIT_OBJECT_0
}

/// Ends a process at once.
pub fn terminate(pid: u32) -> bool {
    let Some(process) = open_process(pid, PROCESS_TERMINATE) else {
        return false;
    };
    // SAFETY: a valid handle.
    unsafe { TerminateProcess(process.0, 1) }.is_ok()
}

/// Every process: id and executable file name (`claude.exe`).
pub fn processes() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is closed by the wrapper; the entry is
    // initialised with its size as the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push((
                entry.th32ProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..len]),
            ));
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
    }
    out
}

/// `MIB_TCP_STATE` values Kairo cares about.
pub const TCP_LISTEN: u32 = 2;
pub const TCP_ESTABLISHED: u32 = 5;

/// A row of the TCP table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpRow {
    pub pid: u32,
    pub state: u32,
    pub local: (IpAddr, u16),
    pub remote: (IpAddr, u16),
}

fn port(raw: u32) -> u16 {
    // The port is in network byte order in the low 16 bits.
    u16::from_be((raw & 0xFFFF) as u16)
}

fn table(family: u32) -> Vec<u8> {
    let mut size = 0u32;
    let mut buf: Vec<u8> = Vec::new();
    for _ in 0..4 {
        // SAFETY: `buf` has `size` bytes after the first round.
        let code = unsafe {
            GetExtendedTcpTable(
                if buf.is_empty() {
                    None
                } else {
                    Some(buf.as_mut_ptr().cast())
                },
                &mut size,
                false,
                family,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if code == ERROR_SUCCESS.0 {
            buf.truncate(size as usize);
            return buf;
        }
        if code != ERROR_INSUFFICIENT_BUFFER.0 {
            break;
        }
        // The table can grow between the two calls: leave room.
        buf = vec![0u8; size as usize + 4096];
        size = buf.len() as u32;
    }
    Vec::new()
}

/// The machine's TCP connections and listeners (IPv4 and IPv6) with their
/// owning process ids. No elevation needed; the owner of another user's
/// socket is still reported, its name is what cannot be read.
pub fn tcp_table() -> Vec<TcpRow> {
    let mut out = Vec::new();
    let v4 = table(AF_INET.0 as u32);
    if v4.len() >= std::mem::size_of::<u32>() {
        // SAFETY: the buffer starts with a MIB_TCPTABLE_OWNER_PID: a count and
        // that many rows, within the bytes the call wrote.
        unsafe {
            let head = v4.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let n = (*head).dwNumEntries as usize;
            let rows = std::ptr::addr_of!((*head).table).cast::<MIB_TCPROW_OWNER_PID>();
            let fits = (v4.len() - std::mem::size_of::<u32>())
                / std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
            for i in 0..n.min(fits) {
                let r = rows.add(i).read_unaligned();
                out.push(TcpRow {
                    pid: r.dwOwningPid,
                    state: r.dwState,
                    local: (
                        IpAddr::V4(Ipv4Addr::from(u32::from_be(r.dwLocalAddr))),
                        port(r.dwLocalPort),
                    ),
                    remote: (
                        IpAddr::V4(Ipv4Addr::from(u32::from_be(r.dwRemoteAddr))),
                        port(r.dwRemotePort),
                    ),
                });
            }
        }
    }
    let v6 = table(AF_INET6.0 as u32);
    if v6.len() >= std::mem::size_of::<u32>() {
        // SAFETY: as above, for the IPv6 table.
        unsafe {
            let head = v6.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let n = (*head).dwNumEntries as usize;
            let rows = std::ptr::addr_of!((*head).table).cast::<MIB_TCP6ROW_OWNER_PID>();
            let fits = (v6.len() - std::mem::size_of::<u32>())
                / std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
            for i in 0..n.min(fits) {
                let r = rows.add(i).read_unaligned();
                out.push(TcpRow {
                    pid: r.dwOwningPid,
                    state: r.dwState,
                    local: (
                        IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)),
                        port(r.dwLocalPort),
                    ),
                    remote: (
                        IpAddr::V6(Ipv6Addr::from(r.ucRemoteAddr)),
                        port(r.dwRemotePort),
                    ),
                });
            }
        }
    }
    out
}

/// The file name of an executable path: `C:\a\claude.exe` → `claude.exe`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
