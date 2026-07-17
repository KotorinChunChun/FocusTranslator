// 共通ユーティリティ: ワイド文字列、クリップボード、DPAPI、言語推定、計測ログ
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use std::path::PathBuf;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HLOCAL, HWND, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId};
use windows::Win32::Foundation::{CloseHandle, MAX_PATH};

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// アプリの表示名。ウィンドウタイトル・MessageBox・トレイ等のユーザー向け表記に使う。
/// 内部名 (ウィンドウクラス名・ミューテックス名・データフォルダ等) は FocusTranslator のまま。
pub const APP_DISPLAY_NAME: &str = "なにこれ？（Focus Translator）";

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
    const CF_UNICODETEXT: u32 = 13;
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
            if GetWindowTextW(hwnd, &mut buf) > 0 {
                if let Some(pos) = buf.iter().position(|&c| c == 0) {
                    title = String::from_utf16(&buf[..pos]).ok();
                }
            }
        }

        // App exe
        let mut exe = None;
        let mut pid = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid > 0 {
            if let Ok(hprocess) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
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
        }

        (exe, title)
    }
}
