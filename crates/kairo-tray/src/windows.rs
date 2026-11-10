//! The notification-area icon: `Shell_NotifyIconW` on a hidden window of its
//! own thread, with a popup menu rebuilt each time it opens.
//!
//! A click shows Kairo's window; a right click opens the menu. The icon is
//! dim while the core is stopped. When Explorer restarts, the icon is added
//! again (`TaskbarCreated`).

use std::cell::RefCell;
use std::time::Duration;

use kairo_core::{Mode, Service};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, GetDC, HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CW_USEDEFAULT, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, HICON,
    HMENU, ICONINFO, MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_NULL,
    WNDCLASSW,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::{ExitId, Hooks, Model, act};

const WM_TRAY: u32 = WM_APP + 2;
const WM_DIM: u32 = WM_APP + 3;
const NIN_SELECT: u32 = 0x400;
const NIN_KEYSELECT: u32 = 0x401;
const ICON_ID: u32 = 1;

const ID_CORE: usize = 20;
const ID_SHOW: usize = 21;
const ID_QUIT: usize = 22;
const ID_ALL: usize = 23;
const ID_TEST: usize = 24;
const ID_SETTINGS: usize = 25;
const ID_FASTEST: usize = 26;
const ID_MODE: usize = 100; // + index
const ID_REST: usize = 1000; // + index
const ID_AI: usize = 2000; // + index

struct Ctx {
    svc: Service,
    hooks: Hooks,
    icon: HICON,
    dim_icon: HICON,
    taskbar_created: u32,
    running: bool,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// An icon from the app's logo (ARGB bytes from [`crate::icon_argb`]).
fn make_icon(dim: bool) -> Option<HICON> {
    const SIZE: i32 = 32;
    let argb = crate::icon_argb(SIZE as u32, dim);
    // The colour bitmap wants BGRA, pre-multiplied.
    let mut bgra = Vec::with_capacity(argb.len());
    for px in argb.chunks_exact(4) {
        let (a, r, g, b) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
        bgra.extend_from_slice(&[
            (b * a / 255) as u8,
            (g * a / 255) as u8,
            (r * a / 255) as u8,
            a as u8,
        ]);
    }
    // SAFETY: GDI objects are created, filled and handed to the icon, then the
    // bitmaps are deleted (the icon keeps its own copies).
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        ReleaseDC(None, dc);
        let color = color.ok()?;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(color.0));
            return None;
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len());
        let mask_bits = vec![0u8; (SIZE * SIZE / 8) as usize];
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        });
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon.ok()
    }
}

fn data(hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    for (slot, unit) in nid.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
        *slot = unit;
    }
    nid
}

fn tooltip(running: bool) -> &'static str {
    if running { "Kairo: connected" } else { "Kairo" }
}

fn add_icon(hwnd: HWND, ctx: &Ctx) {
    let nid = data(
        hwnd,
        if ctx.running { ctx.icon } else { ctx.dim_icon },
        tooltip(ctx.running),
    );
    // SAFETY: a valid structure for a window of this thread.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        let mut v = nid;
        v.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &v);
    }
}

fn menu_text(text: &str) -> HSTRING {
    // `&` marks an accelerator in a menu: a literal one is doubled.
    HSTRING::from(crate::without_flags(text).replace('&', "&&"))
}

/// Appends an item; `checked` puts a tick before it.
fn item(menu: HMENU, id: usize, text: &str, enabled: bool, checked: bool) {
    let mut flags = MF_STRING;
    if !enabled {
        flags |= MF_GRAYED | MF_DISABLED;
    }
    if checked {
        flags |= MF_CHECKED;
    }
    // SAFETY: a live menu and a valid string.
    unsafe {
        let _ = AppendMenuW(menu, flags, id, &menu_text(text));
    }
}

fn separator(menu: HMENU) {
    // SAFETY: a live menu.
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    }
}

/// The menu for `m`, the same one the other platforms show.
fn build(m: &Model) -> Option<HMENU> {
    // SAFETY: menus are created, filled and destroyed by the caller.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        item(
            menu,
            ID_CORE,
            if m.running { "Connected" } else { &m.status },
            m.has_profile,
            m.running,
        );
        if !m.ai.is_empty() {
            item(menu, 0, &m.ai, false, false);
        }
        separator(menu);
        for e in &m.exits {
            let Ok(sub) = CreatePopupMenu() else { continue };
            if let Some(on) = e.fastest {
                item(sub, ID_FASTEST, "Fastest", true, on);
                separator(sub);
            }
            let base = if e.id == ExitId::Ai { ID_AI } else { ID_REST };
            for (i, c) in e.choices.iter().enumerate() {
                item(sub, base + i, &Model::choice_label(c), true, c.checked);
            }
            separator(sub);
            item(sub, ID_ALL, "All Servers…", true, false);
            item(sub, ID_TEST, "Test Again", true, false);
            let title = menu_text(&format!("{}: {}", e.title, e.place));
            let _ = AppendMenuW(menu, MF_POPUP | MF_STRING, sub.0 as usize, &title);
        }
        separator(menu);
        item(menu, 0, "Mode", false, false);
        for (i, mode) in Mode::ALL.iter().enumerate() {
            item(
                menu,
                ID_MODE + i,
                Model::route_title(*mode),
                true,
                *mode == m.mode,
            );
        }
        separator(menu);
        item(menu, ID_SHOW, "Open Kairo", true, false);
        item(menu, ID_SETTINGS, "Settings…", true, false);
        item(menu, ID_QUIT, "Quit Kairo", true, false);
        Some(menu)
    }
}

fn pick(id: usize, model: &Model, ctx: &Ctx) {
    let choice = |exit: ExitId, i: usize| {
        model
            .exits
            .iter()
            .find(|e| e.id == exit)
            .and_then(|e| e.choices.get(i))
            .map(|c| c.name.clone())
    };
    match id {
        i if (ID_MODE..ID_MODE + 3).contains(&i) => act::mode(&ctx.svc, Mode::ALL[i - ID_MODE]),
        ID_CORE => act::connect(&ctx.svc, !model.running),
        ID_SHOW => (ctx.hooks.show)(),
        ID_QUIT => (ctx.hooks.quit)(),
        ID_ALL => (ctx.hooks.page)("exits"),
        ID_SETTINGS => (ctx.hooks.page)("settings"),
        ID_TEST => act::test_again(&ctx.svc),
        ID_FASTEST => act::locate(&ctx.svc, String::new()),
        i if i >= ID_AI => {
            if let Some(name) = choice(ExitId::Ai, i - ID_AI) {
                act::ai_exit(&ctx.svc, name);
            }
        }
        i if i >= ID_REST => {
            if let Some(name) = choice(ExitId::Rest, i - ID_REST) {
                act::locate(&ctx.svc, name);
            }
        }
        _ => {}
    }
}

fn show_menu(hwnd: HWND) {
    let Some((svc, model)) = CTX.with(|c| {
        c.borrow()
            .as_ref()
            .map(|ctx| (ctx.svc.clone(), Model::of(&ctx.svc)))
    }) else {
        return;
    };
    let _ = svc;
    let Some(menu) = build(&model) else { return };
    // SAFETY: the standard tray-menu sequence: the window must be foreground
    // for the menu to close when the user clicks elsewhere.
    let chosen = unsafe {
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            at.x,
            at.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        id.0 as usize
    };
    if chosen != 0 {
        // The click's work (it may block on the service) leaves the menu loop first.
        CTX.with(|c| {
            if let Some(ctx) = c.borrow().as_ref() {
                pick(chosen, &model, ctx);
            }
        });
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match (lparam.0 as u32) & 0xFFFF {
                NIN_SELECT | NIN_KEYSELECT => {
                    CTX.with(|c| {
                        if let Some(ctx) = c.borrow().as_ref() {
                            (ctx.hooks.show)();
                        }
                    });
                }
                WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DIM => {
            CTX.with(|c| {
                if let Some(ctx) = c.borrow_mut().as_mut() {
                    ctx.running = wparam.0 != 0;
                    let nid = data(
                        hwnd,
                        if ctx.running { ctx.icon } else { ctx.dim_icon },
                        tooltip(ctx.running),
                    );
                    // SAFETY: a valid structure for an icon this window added.
                    unsafe {
                        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                    }
                }
            });
            LRESULT(0)
        }
        m if CTX.with(|c| c.borrow().as_ref().is_some_and(|x| x.taskbar_created == m)) => {
            // Explorer restarted: its notification area is empty again.
            CTX.with(|c| {
                if let Some(ctx) = c.borrow().as_ref() {
                    add_icon(hwnd, ctx);
                }
            });
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: the default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn run(svc: Service, hooks: Hooks, ready: std::sync::mpsc::Sender<Result<isize, String>>) {
    let (Some(icon), Some(dim_icon)) = (make_icon(false), make_icon(true)) else {
        let _ = ready.send(Err("could not make the tray icon".into()));
        return;
    };
    // SAFETY: a window class and a hidden window of this thread, then its loop.
    unsafe {
        let Ok(module) = GetModuleHandleW(PCWSTR::null()) else {
            let _ = ready.send(Err("no module handle".into()));
            return;
        };
        let class = w!("KairoTrayWindow");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Kairo tray"),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("tray window: {e}")));
                return;
            }
        };
        let running = svc.state().borrow().core.is_running();
        let ctx = Ctx {
            svc: svc.clone(),
            hooks,
            icon,
            dim_icon,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            running,
        };
        add_icon(hwnd, &ctx);
        CTX.with(|c| *c.borrow_mut() = Some(ctx));
        let _ = ready.send(Ok(hwnd.0 as isize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let nid = data(hwnd, icon, "");
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        let _ = DestroyIcon(icon);
        let _ = DestroyIcon(dim_icon);
    }
}

/// The tray's window and thread, for [`stop`].
static RUNNING: std::sync::Mutex<Option<(isize, std::thread::JoinHandle<()>)>> =
    std::sync::Mutex::new(None);

/// Closes the tray window, which removes the icon, and waits for its thread.
pub fn stop() {
    let Some((hwnd, handle)) = RUNNING.lock().ok().and_then(|mut r| r.take()) else {
        return;
    };
    // SAFETY: posting to the tray's window; WM_CLOSE ends its message loop.
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut std::ffi::c_void)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
    }
    let _ = handle.join();
}

pub fn start(svc: Service, hooks: Hooks) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_svc = svc.clone();
    let handle = std::thread::Builder::new()
        .name("kairo-tray".into())
        .spawn(move || run(thread_svc, hooks, tx))
        .map_err(|e| e.to_string())?;
    let hwnd = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the tray did not start".to_owned())??;
    if let Ok(mut r) = RUNNING.lock() {
        *r = Some((hwnd, handle));
    }
    // Dim the icon while the core is stopped.
    let rt = svc.runtime();
    rt.spawn(async move {
        let mut state = svc.state();
        let mut last = None;
        loop {
            let running = state.borrow().core.is_running();
            if last != Some(running) {
                last = Some(running);
                // SAFETY: posting to a window of another thread is what PostMessage is for.
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(hwnd as *mut std::ffi::c_void)),
                        WM_DIM,
                        WPARAM(usize::from(running)),
                        LPARAM(0),
                    );
                }
            }
            if state.changed().await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    Ok(())
}
