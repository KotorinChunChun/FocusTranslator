// 共通ユーティリティ: ワイド文字列、クリップボード、DPAPI、言語推定、計測ログ
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use std::path::PathBuf;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HLOCAL, HWND, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindow, GW_OWNER, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId};
use windows::Win32::Foundation::{CloseHandle, MAX_PATH};

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// アプリの表示名。ウィンドウタイトル・MessageBox・トレイ等のユーザー向け表記に使う。
/// 内部名 (ウィンドウクラス名・ミューテックス名・データフォルダ等) は FocusTranslator のまま。
pub const APP_DISPLAY_NAME: &str = "なにこれ？（Focus Translator）";

/// アプリの短縮表示名。オーバーレイのシステムメッセージ行など、常時表示する
/// タイトルとして使う (括弧書きの英語名までは表示しない)。
pub const APP_SHORT_NAME: &str = "なにこれ？";

static DISPLAY_NAME_W: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();

/// 表示名の PCWSTR。静的領域を指すのでそのまま API に渡してよい。
pub fn display_name_pcwstr() -> windows::core::PCWSTR {
    windows::core::PCWSTR(DISPLAY_NAME_W.get_or_init(|| to_wide(APP_DISPLAY_NAME)).as_ptr())
}

/// テスト用の直列化ロック。FOCUSTRANSLATOR_DATA_DIR を切り替えるテスト(logdb)と、
/// 実データディレクトリのモデルを参照するテスト(onnx_translate)がプロセス内で
/// 並行実行されると config_dir() の解決先が途中で変わるため、両者はこのロックを取る。
#[cfg(test)]
pub static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 設定・モデルの保存先。`FOCUSTRANSLATOR_DATA_DIR` が設定されていればそちらを使う
/// (動作確認・自動テスト用に実際のユーザー設定と分離するため)。通常は `%APPDATA%\FocusTranslator`。
pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FOCUSTRANSLATOR_DATA_DIR") {
        let p = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&p);
        return p;
    }
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let p = PathBuf::from(base).join("FocusTranslator");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn set_clipboard_text(hwnd: HWND, text: &str) -> bool {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        let mut ok = false;
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let ptr = GlobalLock(hmem);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr as *mut u8, bytes);
                let _ = GlobalUnlock(hmem);
                ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0))).is_ok();
            }
            if !ok {
                let _ = windows::Win32::Foundation::GlobalFree(Some(HGLOBAL(hmem.0)));
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

/// クリップボードの内容種別 (SPECv0.5.4 §20: 「コピー中の内容」ボタンの活性判定に使う)。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClipboardKind {
    /// テキストも画像も無い (ボタンはグレーアウト)
    #[default]
    None,
    /// CF_UNICODETEXT が利用可能
    Text,
    /// CF_DIB (ビットマップ画像) が利用可能
    Image,
}

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

/// クリップボードの内容種別を判定する (SPECv0.5.4 §20)。OpenClipboard 不要で軽量なため
/// オーバーレイ同期毎に呼べる。テキストを画像より優先する。
pub fn clipboard_kind() -> ClipboardKind {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
            ClipboardKind::Text
        } else if IsClipboardFormatAvailable(CF_DIB).is_ok() {
            ClipboardKind::Image
        } else {
            ClipboardKind::None
        }
    }
}

/// クリップボードのテキスト (CF_UNICODETEXT) を取得する (SPECv0.5.4 §20)。
pub fn get_clipboard_text(hwnd: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return None;
        }
        let result = (|| {
            let h = GetClipboardData(CF_UNICODETEXT).ok()?;
            let ptr = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(HGLOBAL(h.0));
            Some(s)
        })();
        let _ = CloseClipboard();
        result
    }
}

/// クリップボードの画像 (CF_DIB) を取得して `Captured`(BGRA) へ変換する (SPECv0.5.4 §20)。
/// BI_RGB の 24bit / 32bit のみ対応 (圧縮DIBは非対応で None)。ボトムアップ/トップダウンの
/// どちらの向きにも対応する。
pub fn get_clipboard_image(hwnd: HWND) -> Option<crate::capture::Captured> {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return None;
        }
        let result = (|| {
            let h = GetClipboardData(CF_DIB).ok()?;
            let hg = HGLOBAL(h.0);
            let ptr = GlobalLock(hg) as *const u8;
            if ptr.is_null() {
                return None;
            }
            let size = GlobalSize(hg);
            let parsed = parse_dib(std::slice::from_raw_parts(ptr, size));
            let _ = GlobalUnlock(hg);
            parsed
        })();
        let _ = CloseClipboard();
        result
    }
}

/// CF_DIB バイト列 (BITMAPINFOHEADER + ピクセル) を `Captured`(BGRA, トップダウン) へ変換する。
fn parse_dib(data: &[u8]) -> Option<crate::capture::Captured> {
    // BITMAPINFOHEADER の必要フィールドを読む (先頭40バイト)
    if data.len() < 40 {
        return None;
    }
    let rd_u32 = |off: usize| u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
    let rd_i32 = |off: usize| i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
    let rd_u16 = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]);

    let bi_size = rd_u32(0) as usize;
    let width = rd_i32(4);
    let height_raw = rd_i32(8);
    let bit_count = rd_u16(14);
    let compression = rd_u32(16);
    let clr_used = rd_u32(32) as usize;

    // BI_RGB(0) と BI_BITFIELDS(3) の 24/32bit のみ対応 (圧縮DIBは非対応)
    if (compression != 0 && compression != 3)
        || !(bit_count == 24 || bit_count == 32)
        || width <= 0
        || height_raw == 0
    {
        app_log(&format!(
            "clipboard DIB unsupported: bi_size={bi_size} bit={bit_count} comp={compression} w={width} h={height_raw}"
        ));
        return None;
    }
    let top_down = height_raw < 0;
    let width = width as usize;
    let height = height_raw.unsigned_abs() as usize;

    // ピクセルデータ開始オフセット = ヘッダ + カラーマスク + カラーパレット。
    // BI_BITFIELDS かつ BITMAPINFOHEADER(40) のときは、ヘッダ直後に12バイトのマスクが入る
    // (BITMAPV4/V5HEADER ではマスクはヘッダ内に含まれるため加算しない)。
    let mask_bytes = if compression == 3 && bi_size == 40 { 12 } else { 0 };
    let palette_bytes = clr_used * 4;
    let pixel_off = bi_size + mask_bytes + palette_bytes;
    let bytes_per_px = (bit_count / 8) as usize;
    // 各行は4バイト境界へパディングされる (DIBのstride規則)
    let stride = width.saturating_mul(bytes_per_px).div_ceil(4) * 4;
    if pixel_off.saturating_add(stride.saturating_mul(height)) > data.len() {
        app_log(&format!(
            "clipboard DIB size mismatch: need={} have={} (off={pixel_off} stride={stride} h={height})",
            pixel_off + stride * height,
            data.len()
        ));
        return None;
    }

    let mut bgra = vec![0u8; width * height * 4];
    for row in 0..height {
        // ボトムアップ(既定)は最下行から格納されているため、出力の行を反転する
        let src_row = if top_down { row } else { height - 1 - row };
        let src_line = pixel_off + src_row * stride;
        for col in 0..width {
            let sp = src_line + col * bytes_per_px;
            let dp = (row * width + col) * 4;
            bgra[dp] = data[sp]; // B
            bgra[dp + 1] = data[sp + 1]; // G
            bgra[dp + 2] = data[sp + 2]; // R
            bgra[dp + 3] = if bytes_per_px == 4 { data[sp + 3] } else { 255 }; // A
        }
    }
    Some(crate::capture::Captured {
        width: width as u32,
        height: height as u32,
        bgra,
    })
}

/// DPAPI でユーザー単位に暗号化し base64 で返す。空文字はそのまま。
pub fn dpapi_encrypt(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    unsafe {
        let bytes = plain.as_bytes();
        let inb = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut outb = CRYPT_INTEGER_BLOB::default();
        if CryptProtectData(&inb, None, None, None, None, 0, &mut outb).is_ok() {
            let slice = std::slice::from_raw_parts(outb.pbData, outb.cbData as usize);
            let s = B64.encode(slice);
            let _ = LocalFree(Some(HLOCAL(outb.pbData as *mut _)));
            s
        } else {
            String::new()
        }
    }
}

pub fn dpapi_decrypt(enc_b64: &str) -> String {
    if enc_b64.is_empty() {
        return String::new();
    }
    let Ok(raw) = B64.decode(enc_b64) else {
        return String::new();
    };
    unsafe {
        let inb = CRYPT_INTEGER_BLOB {
            cbData: raw.len() as u32,
            pbData: raw.as_ptr() as *mut u8,
        };
        let mut outb = CRYPT_INTEGER_BLOB::default();
        if CryptUnprotectData(&inb, None, None, None, None, 0, &mut outb).is_ok() {
            let slice = std::slice::from_raw_parts(outb.pbData, outb.cbData as usize);
            let s = String::from_utf8_lossy(slice).into_owned();
            let _ = LocalFree(Some(HLOCAL(outb.pbData as *mut _)));
            s
        } else {
            String::new()
        }
    }
}

/// 文字数で切り詰め、超過分は "…" で省略する(ボタン表示等の短縮用)
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

/// Windowsのアプリモード (設定 > 個人用設定 > 色) がライトモードか。
/// レジストリ AppsUseLightTheme (1=ライト / 0=ダーク) を読む。読めない環境はダーク扱い。
pub fn system_apps_light_theme() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
        .is_ok()
            && data != 0
    }
}

/// 日本語・中国語圏の文字を含むか(翻訳方向の推定用)
pub fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9D)
    })
}

/// 計測ログ(有効時のみ)。原文・訳文は記録しない。
pub fn perf_log(enabled: bool, line: &str) {
    if !enabled {
        return;
    }
    use std::io::Write;
    let path = config_dir().join("perf.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(f, "{ts} {line}");
    }
}

/// 予期しないエラーの記録(デバッグ用、テキスト内容は含めない)
pub fn app_log(line: &str) {
    use std::io::Write;
    let path = config_dir().join("app.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(f, "{ts} {line}");
    }
}

/// 指定した HWND から実行ファイル名とウィンドウタイトルを取得する
pub fn get_window_context(hwnd: HWND) -> (Option<String>, Option<String>) {
    unsafe {
        // App title
        let mut title = None;
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            if GetWindowTextW(hwnd, &mut buf) > 0
                && let Some(pos) = buf.iter().position(|&c| c == 0)
            {
                title = String::from_utf16(&buf[..pos]).ok();
            }
        }

        // App exe
        let mut exe = None;
        let mut pid = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid > 0
            && let Ok(hprocess) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
        {
            let mut path = vec![0u16; MAX_PATH as usize];
            let mut size = MAX_PATH;
            if QueryFullProcessImageNameW(hprocess, windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0), windows::core::PWSTR(path.as_mut_ptr()), &mut size).is_ok() {
                let full_path = String::from_utf16_lossy(&path[..size as usize]);
                if let Some(file_name) = std::path::Path::new(&full_path).file_name().and_then(|n| n.to_str()) {
                    exe = Some(file_name.to_string());
                }
            }
            let _ = CloseHandle(hprocess);
        }

        (exe, title)
    }
}

/// キャプチャ対象ウィンドウからアプリの実行ファイル名・タイトルを取得する (SPECv0.5.4 §9b)。
/// メニューポップアップ (クラス #32768) 等はそれ自体がトップレベルウィンドウだがタイトルを
/// 持たないため、GW_OWNER でオーナーウィンドウ (メニューを開いた親アプリ) を辿って
/// タイトルを補完する。実行ファイル名は同一プロセスなのでメニュー側からでも取れる。
pub fn get_app_context(hwnd: HWND) -> (Option<String>, Option<String>) {
    let (exe, title) = get_window_context(hwnd);
    if title.is_some() {
        return (exe, title);
    }
    // タイトルが取れない (メニュー等)。オーナーウィンドウを辿って親アプリ情報を補完する。
    let owner = unsafe { GetWindow(hwnd, GW_OWNER).unwrap_or_default() };
    if !owner.is_invalid() {
        let (owner_exe, owner_title) = get_window_context(owner);
        if owner_title.is_some() {
            return (exe.or(owner_exe), owner_title);
        }
    }
    (exe, title)
}

/// 診断用: ウィンドウのクラス名を取得する (SPECv0.5.4 §9b: メニュー等で親アプリを
/// 見失う問題の切り分け用)。取得できなければ空文字。
pub fn get_window_class(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

/// 診断用: 対象ウィンドウの class/exe/title に加え、GW_OWNER で辿ったオーナーウィンドウと
/// 現在のフォアグラウンドウィンドウの情報を1行にまとめる (SPECv0.5.4 §9b)。
/// メニューポップアップ (#32768) 上でキャプチャしたときに、どこから親アプリを辿れるかを調べる。
pub fn window_diag(hwnd: HWND) -> String {
    unsafe {
        let class = get_window_class(hwnd);
        let (exe, title) = get_window_context(hwnd);
        let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
        let (owner_exe, owner_title, owner_class) = if owner.is_invalid() {
            (None, None, String::new())
        } else {
            let (e, t) = get_window_context(owner);
            (e, t, get_window_class(owner))
        };
        let fg = GetForegroundWindow();
        let (fg_exe, fg_title, fg_class) = if fg.is_invalid() {
            (None, None, String::new())
        } else {
            let (e, t) = get_window_context(fg);
            (e, t, get_window_class(fg))
        };
        format!(
            "target[class={class:?} exe={exe:?} title={title:?}] owner[class={owner_class:?} exe={owner_exe:?} title={owner_title:?}] fg[class={fg_class:?} exe={fg_exe:?} title={fg_title:?}]"
        )
    }
}
