//! UIA選択文字列検出テスト用のモックアプリ (SPECv0.5.4 §12)。
//!
//! 素のWin32コントロール(単一行EDIT / 複数行EDIT / 編集可能コンボボックス)を並べた
//! ウィンドウを表示する。focus-translator を起動した状態でこのウィンドウ上のテキストを
//! 選択し、プレビューキー/キャプチャキーを押して「選択中の文字列」が検出されるかを確認する。
//! 設定画面などで再現していた「標準EDITの選択が検出できない」問題の切り分けに使う。
//!
//! 実行: `cargo run --example uia_mock`
//!
//! これは focus-translator 本体とは独立した検査対象アプリで、UIA検出そのものは行わない
//! (検出ロジックは本体側の uia::probe_at_point が担う)。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CB_ADDSTRING, CW_USEDEFAULT, CreateMenu, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GetMessageW, HMENU, IDC_ARROW, LoadCursorW, MF_POPUP, MF_STRING, MSG,
    PostQuitMessage, RegisterClassW, SW_SHOW, SendMessageW, SetMenu, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CREATE, WM_DESTROY, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

// windows クレートに定数が無いEDIT/COMBOBOXスタイル (win32 の値をそのまま使う)
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_AUTOHSCROLL: u32 = 0x0080;
const CBS_DROPDOWN: u32 = 0x0002;

fn main() {
    unsafe {
        let hinst = GetModuleHandleW(None).expect("module handle");
        let class = w!("UiaMockWindow");
        let wc = WNDCLASSW {
            hInstance: hinst.into(),
            lpszClassName: class,
            lpfnWndProc: Some(wndproc),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("UIA Mock - 選択文字列検出テスト"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            700,
            540,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .expect("create window");

        // メニューバーを付ける (SPECv0.5.4 §9b: メニューアイテム上でのキャプチャ検証用)。
        // 「ファイル」「編集」ドロップダウンにいくつか項目を並べる。
        let menu = CreateMenu().expect("create menu");
        let file_menu = CreateMenu().expect("create file menu");
        let _ = AppendMenuW(file_menu, MF_STRING, 2001, w!("新規作成"));
        let _ = AppendMenuW(file_menu, MF_STRING, 2002, w!("開く..."));
        let _ = AppendMenuW(file_menu, MF_STRING, 2003, w!("名前を付けて保存"));
        let edit_menu = CreateMenu().expect("create edit menu");
        let _ = AppendMenuW(edit_menu, MF_STRING, 2101, w!("元に戻す"));
        let _ = AppendMenuW(edit_menu, MF_STRING, 2102, w!("すべて選択"));
        let _ = AppendMenuW(menu, MF_POPUP, file_menu.0 as usize, w!("ファイル(&F)"));
        let _ = AppendMenuW(menu, MF_POPUP, edit_menu.0 as usize, w!("編集(&E)"));
        let _ = SetMenu(hwnd, Some(menu));

        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            unsafe { create_children(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe fn create_children(parent: HWND) {
    unsafe {
        let hinst = GetModuleHandleW(None).expect("module handle");
        let hinst = hinst.into();

        make(
            parent, hinst, w!("STATIC"),
            w!("各コントロールでテキストを選択し、focus-translator のキーを押して検出を確認してください:"),
            WS_CHILD | WS_VISIBLE, 12, 10, 664, 20, 0,
        );

        make(
            parent, hinst, w!("STATIC"), w!("① 単一行 EDIT:"),
            WS_CHILD | WS_VISIBLE, 12, 42, 664, 18, 0,
        );
        make(
            parent, hinst, w!("EDIT"),
            w!("Select this text in a single-line edit box. 単一行の選択テスト。"),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL),
            12, 62, 664, 26, 101,
        );

        make(
            parent, hinst, w!("STATIC"), w!("② 複数行 EDIT:"),
            WS_CHILD | WS_VISIBLE, 12, 98, 664, 18, 0,
        );
        make(
            parent, hinst, w!("EDIT"),
            w!("This is a multi-line edit control.\r\nSelect any part of this paragraph to test the selected-text detection.\r\n複数行のテキストからも範囲選択して検出できるかを確認します。"),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | WINDOW_STYLE(ES_MULTILINE | ES_AUTOVSCROLL),
            12, 118, 664, 130, 102,
        );

        make(
            parent, hinst, w!("STATIC"), w!("③ 編集可能コンボボックス:"),
            WS_CHILD | WS_VISIBLE, 12, 258, 664, 18, 0,
        );
        let combo = make(
            parent, hinst, w!("COMBOBOX"), w!(""),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(CBS_DROPDOWN),
            12, 278, 320, 220, 103,
        );
        for item in [w!("First combo item"), w!("Second combo item"), w!("三番目の項目")] {
            SendMessageW(combo, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(item.as_ptr() as isize)));
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn make(
    parent: HWND,
    hinst: windows::Win32::Foundation::HINSTANCE,
    class: PCWSTR,
    text: PCWSTR,
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            text,
            style,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as isize as *mut _)),
            Some(hinst),
            None,
        )
        .expect("create child control")
    }
}
