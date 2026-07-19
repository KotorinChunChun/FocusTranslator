use crate::util::to_wide;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    FONT_OUTPUT_PRECISION, FW_BOLD, FW_NORMAL, HFONT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetDlgItem, GetWindowTextLengthW, GetWindowTextW, HMENU, SetWindowTextW,
    WINDOW_STYLE, WS_BORDER, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
    SendMessageW, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, CBS_DROPDOWNLIST,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT,
    WM_SETFONT,
};
use windows::Win32::UI::Controls::{
    EM_SETPASSWORDCHAR,
};
use windows::core::{PCWSTR, w};

const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_PASSWORD: u32 = 0x0020;
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

/// 指定IDの子コントロールHWNDを取得する
pub fn get_dlg_item(parent: HWND, id: i32) -> HWND {
    unsafe {
        GetDlgItem(Some(parent), id).unwrap_or_default()
    }
}


/// コンボボックスに項目を追加する
pub fn combo_add_item(cb: HWND, text: &str) {
    unsafe {
        let wide = to_wide(text);
        SendMessageW(cb, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(wide.as_ptr() as isize)));
    }
}

/// コンボボックスの選択インデックスを設定する
pub fn combo_set_sel(cb: HWND, idx: usize) {
    unsafe {
        SendMessageW(cb, CB_SETCURSEL, Some(WPARAM(idx)), Some(LPARAM(0)));
    }
}

/// コンボボックスの現在の選択インデックスを取得する
pub fn combo_get_sel(cb: HWND) -> usize {
    unsafe {
        let r = SendMessageW(cb, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        if r.0 < 0 { 0 } else { r.0 as usize }
    }
}

/// コンボボックスの全項目をクリアする
pub fn combo_reset_content(cb: HWND) {
    unsafe {
        SendMessageW(cb, CB_RESETCONTENT, Some(WPARAM(0)), Some(LPARAM(0)));
    }
}

/// コンボボックスの指定インデックスのテキストを取得する
pub fn combo_get_item_text(cb: HWND, idx: usize) -> String {
    unsafe {
        let len = SendMessageW(cb, CB_GETLBTEXTLEN, Some(WPARAM(idx)), Some(LPARAM(0))).0;
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        SendMessageW(
            cb,
            CB_GETLBTEXT,
            Some(WPARAM(idx)),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        );
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

/// EnumChildWindows で使われるフォント設定用コールバック (UIフォントを適用する)
pub unsafe extern "system" fn set_font_proc(child: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let hfont = HFONT(lparam.0 as *mut _);
    unsafe { SendMessageW(child, WM_SETFONT, Some(WPARAM(hfont.0 as usize)), Some(LPARAM(1))); }
    windows::core::BOOL(1)
}

/// 標準UIフォント (Yu Gothic UI / ClearType) を生成する。size は文字高さ(px)。
/// オーバーレイの描画・各ダイアログのコントロールフォントで共用する。
pub fn make_font(size: i32, bold: bool) -> HFONT {
    unsafe {
        CreateFontW(
            -size,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0.into(),
            w!("Yu Gothic UI"),
        )
    }
}

/// 指定IDのチェックボックスの状態を設定する
pub fn check_set(parent: HWND, id: i32, checked: bool) {
    unsafe {
        let ctl = get_dlg_item(parent, id);
        SendMessageW(ctl, BM_SETCHECK, Some(WPARAM(if checked { 1 } else { 0 })), Some(LPARAM(0)));
    }
}

/// 指定IDのチェックボックスの状態を取得する
pub fn check_get(parent: HWND, id: i32) -> bool {
    unsafe {
        let ctl = get_dlg_item(parent, id);
        SendMessageW(ctl, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1
    }
}

/// 指定IDのコンボボックスへ項目一覧を追加し、選択位置を設定する
pub fn combo_fill(parent: HWND, id: i32, items: &[&str], selected: usize) {
    let cb = get_dlg_item(parent, id);
    for item in items {
        combo_add_item(cb, item);
    }
    combo_set_sel(cb, selected);
}

/// 指定IDのコンボボックスの現在の選択インデックスを取得する
pub fn combo_sel(parent: HWND, id: i32) -> usize {
    combo_get_sel(get_dlg_item(parent, id))
}

/// 指定IDのコンボボックスの全項目をクリアする
pub fn combo_reset(parent: HWND, id: i32) {
    combo_reset_content(get_dlg_item(parent, id));
}

/// 指定IDのコンボボックスの選択インデックスを設定する
pub fn combo_select(parent: HWND, id: i32, idx: usize) {
    combo_set_sel(get_dlg_item(parent, id), idx);
}
