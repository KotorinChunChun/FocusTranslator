use crate::util::to_wide;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetDlgItem, GetWindowTextLengthW, GetWindowTextW, HMENU, SetWindowTextW,
    WINDOW_STYLE, WS_BORDER, WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    SendMessageW, BS_AUTOCHECKBOX, CBS_DROPDOWNLIST,
};
use windows::Win32::UI::Controls::{
    EM_SETPASSWORDCHAR,
};
use windows::core::{PCWSTR, w};

const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_PASSWORD: u32 = 0x0020;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_WANTRETURN: u32 = 0x1000;
const PASSWORD_CHAR: usize = 0x25CF; // ●

#[allow(clippy::too_many_arguments)]
pub fn ctl(
    parent: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    unsafe {
        let wide = to_wide(text);
        CreateWindowExW(
            Default::default(),
            class,
            PCWSTR(wide.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            Some(instance),
            None,
        )
        .unwrap_or_default()
    }
}

pub fn label(parent: HWND, instance: HINSTANCE, text: &str, x: i32, y: i32, w: i32) {
    ctl(parent, instance, w!("STATIC"), text, WINDOW_STYLE(0), x, y, w, 20, 0);
}

pub fn edit(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("EDIT"),
        "",
        WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL),
        x,
        y,
        w,
        22,
        id,
    )
}

pub fn multiline(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("EDIT"),
        "",
        WS_BORDER | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN),
        x,
        y,
        w,
        h,
        id,
    )
}

pub fn password_edit(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, id: i32) -> HWND {
    let h = ctl(
        parent,
        instance,
        w!("EDIT"),
        "",
        WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL | ES_PASSWORD),
        x,
        y,
        w,
        22,
        id,
    );
    unsafe {
        SendMessageW(h, EM_SETPASSWORDCHAR, Some(WPARAM(PASSWORD_CHAR)), Some(LPARAM(0)));
    }
    h
}

pub fn combo(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("COMBOBOX"),
        "",
        WS_TABSTOP | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        x,
        y,
        w,
        200,
        id,
    )
}

pub fn button(parent: HWND, instance: HINSTANCE, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(parent, instance, w!("BUTTON"), text, WS_TABSTOP, x, y, w, 26, id)
}

/// 子コントロールへテキストを設定する
pub fn set_ctl_text(parent: HWND, id: i32, text: &str) {
    unsafe {
        let ctl = GetDlgItem(Some(parent), id).unwrap_or_default();
        let wide = to_wide(text);
        let _ = SetWindowTextW(ctl, PCWSTR(wide.as_ptr()));
    }
}

/// 子コントロールのテキストを取得する
pub fn get_ctl_text(parent: HWND, id: i32) -> String {
    unsafe {
        let ctl = GetDlgItem(Some(parent), id).unwrap_or_default();
        let len = GetWindowTextLengthW(ctl);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(ctl, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// マルチラインEDITへの書込み: Win32 EDITは改行に\r\nを要するため\nを正規化して変換する
pub fn set_multiline_text(parent: HWND, id: i32, text: &str) {
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
    set_ctl_text(parent, id, &normalized);
}

/// マルチラインEDITからの読込み: 保存データは\n改行で統一する
pub fn get_multiline_text(parent: HWND, id: i32) -> String {
    get_ctl_text(parent, id).replace("\r\n", "\n")
}

pub fn checkbox(parent: HWND, instance: HINSTANCE, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("BUTTON"),
        text,
        WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        x,
        y,
        w,
        22,
        id,
    )
}
