// タスクトレイ常駐 (SPEC §1, §2.1)
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, IDI_APPLICATION, LoadIconW,
    MF_SEPARATOR, MF_STRING, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu,
};
use windows::core::w;

pub const CMD_SETTINGS: usize = 1;
pub const CMD_REGION: usize = 2;
pub const CMD_EXIT: usize = 4;

fn make_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: crate::WM_APP_TRAY,
        ..Default::default()
    };
    unsafe {
        nid.hIcon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
    }
    let tip: Vec<u16> = "Focus Translator".encode_utf16().collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    nid
}

pub fn add_icon(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &make_nid(hwnd));
    }
}

pub fn remove_icon(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &make_nid(hwnd));
    }
}

/// コンテキストメニューを表示し、選択されたコマンドIDを返す(0=キャンセル)
pub fn show_menu(hwnd: HWND) -> usize {
    unsafe {
        let Ok(menu): Result<HMENU, _> = CreatePopupMenu() else {
            return 0;
        };
        let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("設定..."));
        let _ = AppendMenuW(menu, MF_STRING, CMD_REGION, w!("範囲指定翻訳"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, w!("終了"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}
