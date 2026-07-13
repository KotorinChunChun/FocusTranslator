// OneOCR (Windows 11 Snipping Tool 同梱の oneocr.dll) によるローカルOCR
// Windows 11 では Snipping Tool (Microsoft.ScreenSketch パッケージ) に
// oneocr.dll / oneocr.onemodel / onnxruntime.dll が同梱されており、追加インストール不要で
// Windows.Media.Ocr より高精度な認識ができる。公開APIではないため、パッケージフォルダを
// GetPackagesByPackageFamily で解決し、C ABI のエクスポート関数を直接呼び出す。
// (関数シグネチャ・モデルキーは b1tg/win11-oneocr, AuroraWright/oneocr で公知のもの)
use crate::capture::Captured;
use crate::ocr::Focus;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::Mutex;
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HMODULE};
use windows::Win32::Storage::Packaging::Appx::{
    GetPackagePathByFullName, GetPackagesByPackageFamily,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows::core::{PCSTR, PCWSTR, PWSTR};

/// Snipping Tool のパッケージファミリー名 (バージョンによらず固定)
const PACKAGE_FAMILY: &str = "Microsoft.ScreenSketch_8wekyb3d8bbwe";
/// oneocr.onemodel の復号キー (oneocr.dll 利用側で共通の公知の定数)
const MODEL_KEY: &CStr = c"kj)TGtrK>f]b[Piow.gU+nC@s\"\"\"\"\"\"4";
const MAX_LINES: i64 = 1000;

use std::ffi::CStr;

/// oneocr.dll に渡す画像 (BGRA、行ストライドは width*4)
#[repr(C)]
struct Img {
    t: i32,
    col: i32,
    row: i32,
    _unk: i32,
    step: i64,
    data_ptr: i64,
}

/// 行・単語のバウンディングボックス (四隅の座標)
#[repr(C)]
struct BBox {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    x3: f32,
    y3: f32,
    x4: f32,
    y4: f32,
}

type CreateOcrInitOptionsFn = unsafe extern "C" fn(*mut i64) -> i64;
type SetUseModelDelayLoadFn = unsafe extern "C" fn(i64, u8) -> i64;
type CreateOcrPipelineFn = unsafe extern "C" fn(*const i8, *const i8, i64, *mut i64) -> i64;
type CreateOcrProcessOptionsFn = unsafe extern "C" fn(*mut i64) -> i64;
type SetMaxLineCountFn = unsafe extern "C" fn(i64, i64) -> i64;
type RunOcrPipelineFn = unsafe extern "C" fn(i64, *const Img, i64, *mut i64) -> i64;
type GetOcrLineCountFn = unsafe extern "C" fn(i64, *mut i64) -> i64;
type GetOcrLineFn = unsafe extern "C" fn(i64, i64, *mut i64) -> i64;
type GetOcrLineContentFn = unsafe extern "C" fn(i64, *mut *const i8) -> i64;
type GetOcrLineBoundingBoxFn = unsafe extern "C" fn(i64, *mut *const BBox) -> i64;
type ReleaseOcrResultFn = unsafe extern "C" fn(i64) -> i64;

struct Engine {
    /// DLLをプロセス生存中ロードし続けるため保持 (Dropで解放しない)
    _module: HMODULE,
    pipeline: i64,
    process_options: i64,
    run_pipeline: RunOcrPipelineFn,
    get_line_count: GetOcrLineCountFn,
    get_line: GetOcrLineFn,
    get_line_content: GetOcrLineContentFn,
    get_line_bbox: GetOcrLineBoundingBoxFn,
    release_result: Option<ReleaseOcrResultFn>,
}

// 生ポインタ(HMODULE・ハンドル値)を含むが、利用は常に ENGINE のロック越しに直列化される
unsafe impl Send for Engine {}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Snipping Tool パッケージのインストールフォルダを解決する
fn package_dir() -> Option<PathBuf> {
    unsafe {
        let family: Vec<u16> = PACKAGE_FAMILY.encode_utf16().chain(std::iter::once(0)).collect();
        let mut count = 0u32;
        let mut buf_len = 0u32;
        let rc = GetPackagesByPackageFamily(
            PCWSTR(family.as_ptr()),
            &mut count,
            None,
            &mut buf_len,
            None,
        );
        if rc != ERROR_SUCCESS && rc != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        if count == 0 {
            return None;
        }
        let mut names: Vec<PWSTR> = vec![PWSTR::null(); count as usize];
        let mut buf: Vec<u16> = vec![0; buf_len as usize];
        let rc = GetPackagesByPackageFamily(
            PCWSTR(family.as_ptr()),
            &mut count,
            Some(names.as_mut_ptr()),
            &mut buf_len,
            Some(PWSTR(buf.as_mut_ptr())),
        );
        if rc != ERROR_SUCCESS || count == 0 {
            return None;
        }
        let full_name = names[0];
        let mut path_len = 0u32;
        let rc = GetPackagePathByFullName(PCWSTR(full_name.0), &mut path_len, None);
        if rc != ERROR_SUCCESS && rc != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        let mut path_buf: Vec<u16> = vec![0; path_len as usize];
        let rc =
            GetPackagePathByFullName(PCWSTR(full_name.0), &mut path_len, Some(PWSTR(path_buf.as_mut_ptr())));
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = path_buf.iter().position(|&c| c == 0).unwrap_or(path_buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&path_buf[..end])))
    }
}

/// OneOCRに必要なファイル一式 (onnxruntime.dll は oneocr.dll の依存DLL)
const FILES: [&str; 3] = ["oneocr.dll", "oneocr.onemodel", "onnxruntime.dll"];

/// oneocr.dll と oneocr.onemodel を含むフォルダ (Snipping Tool同梱分)
fn oneocr_dir() -> Option<PathBuf> {
    let dir = package_dir()?.join("SnippingTool");
    if FILES.iter().all(|f| dir.join(f).is_file()) { Some(dir) } else { None }
}

/// WindowsApps配下はACLによりDLLの直接ロードが拒否されるため、コピー先として使う
/// アプリ設定フォルダ内の格納先
fn local_dir() -> PathBuf {
    crate::util::config_dir().join("models").join("oneocr")
}

fn local_copy_complete() -> bool {
    let d = local_dir();
    FILES.iter().all(|f| d.join(f).is_file())
}

/// Snipping Toolのファイル一式をアプリ設定フォルダへコピーする (初回のみ。
/// パッケージ更新でサイズが変わった場合は上書きして追従する)
fn ensure_local_copy() -> Result<PathBuf, String> {
    let src = oneocr_dir()
        .ok_or_else(|| "OneOCRが見つかりません(Windows 11のSnipping Toolが必要です)".to_string())?;
    let dst = local_dir();
    std::fs::create_dir_all(&dst).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    for f in FILES {
        let s = src.join(f);
        let d = dst.join(f);
        let same_size = match (std::fs::metadata(&s), std::fs::metadata(&d)) {
            (Ok(sm), Ok(dm)) => sm.len() == dm.len(),
            _ => false,
        };
        if !same_size {
            std::fs::copy(&s, &d).map_err(|e| format!("{f} のコピーに失敗しました: {e}"))?;
        }
    }
    Ok(dst)
}

/// OneOCRが利用可能か (初期化済み・コピー済み、または Snipping Tool のファイルが見つかるか)
pub fn available() -> bool {
    if let Ok(guard) = ENGINE.lock()
        && guard.is_some()
    {
        return true;
    }
    local_copy_complete() || oneocr_dir().is_some()
}

fn get_proc(module: HMODULE, name: &CStr) -> Result<unsafe extern "system" fn() -> isize, String> {
    unsafe {
        GetProcAddress(module, PCSTR(name.as_ptr() as *const u8))
            .ok_or_else(|| format!("oneocr.dll に {} が見つかりません", name.to_string_lossy()))
    }
}

macro_rules! proc_as {
    ($module:expr, $name:literal, $ty:ty) => {{
        let p = get_proc($module, $name)?;
        unsafe { std::mem::transmute::<_, $ty>(p) }
    }};
}

fn load_engine() -> Result<Engine, String> {
    // WindowsApps配下からの直接ロードはACLで拒否されるため、設定フォルダへコピーして読み込む
    let dir = if local_copy_complete() { local_dir() } else { ensure_local_copy()? };
    let dll_path = dir.join("oneocr.dll");
    let wide: Vec<u16> = dll_path.as_os_str().encode_wide_nul();
    let module = unsafe {
        LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH)
            .map_err(|e| format!("oneocr.dll の読込に失敗しました: {e}"))?
    };
    let module = HMODULE(module.0);

    let create_init_options = proc_as!(module, c"CreateOcrInitOptions", CreateOcrInitOptionsFn);
    let set_delay_load =
        proc_as!(module, c"OcrInitOptionsSetUseModelDelayLoad", SetUseModelDelayLoadFn);
    let create_pipeline = proc_as!(module, c"CreateOcrPipeline", CreateOcrPipelineFn);
    let create_process_options =
        proc_as!(module, c"CreateOcrProcessOptions", CreateOcrProcessOptionsFn);
    let set_max_lines =
        proc_as!(module, c"OcrProcessOptionsSetMaxRecognitionLineCount", SetMaxLineCountFn);
    let run_pipeline = proc_as!(module, c"RunOcrPipeline", RunOcrPipelineFn);
    let get_line_count = proc_as!(module, c"GetOcrLineCount", GetOcrLineCountFn);
    let get_line = proc_as!(module, c"GetOcrLine", GetOcrLineFn);
    let get_line_content = proc_as!(module, c"GetOcrLineContent", GetOcrLineContentFn);
    let get_line_bbox = proc_as!(module, c"GetOcrLineBoundingBox", GetOcrLineBoundingBoxFn);
    let release_result = get_proc(module, c"ReleaseOcrResult")
        .ok()
        .map(|p| unsafe { std::mem::transmute::<_, ReleaseOcrResultFn>(p) });

    let model_path = CString::new(dir.join("oneocr.onemodel").to_string_lossy().as_bytes())
        .map_err(|_| "モデルパスの変換に失敗しました".to_string())?;

    unsafe {
        let mut init_options = 0i64;
        if create_init_options(&mut init_options) != 0 {
            return Err("OneOCRの初期化オプション作成に失敗しました".into());
        }
        if set_delay_load(init_options, 0) != 0 {
            return Err("OneOCRの初期化オプション設定に失敗しました".into());
        }
        let mut pipeline = 0i64;
        if create_pipeline(model_path.as_ptr(), MODEL_KEY.as_ptr(), init_options, &mut pipeline) != 0 {
            return Err("OneOCRモデルの読込に失敗しました".into());
        }
        let mut process_options = 0i64;
        if create_process_options(&mut process_options) != 0 {
            return Err("OneOCRの実行オプション作成に失敗しました".into());
        }
        if set_max_lines(process_options, MAX_LINES) != 0 {
            return Err("OneOCRの実行オプション設定に失敗しました".into());
        }
        Ok(Engine {
            _module: module,
            pipeline,
            process_options,
            run_pipeline,
            get_line_count,
            get_line,
            get_line_content,
            get_line_bbox,
            release_result,
        })
    }
}

/// OsStr → NUL終端UTF-16
trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}
impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

/// OneOCRによるローカルOCR。
/// 戻り値は ocr_windows と同形式: (採用テキスト, Paragraphモード時のカーソル直下1行)
pub fn ocr_oneocr(img: &Captured, focus: Focus) -> Result<(String, Option<String>), String> {
    let mut guard = ENGINE.lock().map_err(|_| "OneOCRエンジンのロックに失敗しました".to_string())?;
    if guard.is_none() {
        *guard = Some(load_engine()?);
    }
    let eng = guard.as_ref().expect("直前に初期化済み");

    let image = Img {
        t: 3,
        col: img.width as i32,
        row: img.height as i32,
        _unk: 0,
        step: (img.width * 4) as i64,
        data_ptr: img.bgra.as_ptr() as i64,
    };

    let mut items: Vec<(f32, f32, String)> = Vec::new();
    unsafe {
        let mut instance = 0i64;
        if (eng.run_pipeline)(eng.pipeline, &image, eng.process_options, &mut instance) != 0 {
            return Err("OneOCRの実行に失敗しました".into());
        }
        let mut line_count = 0i64;
        if (eng.get_line_count)(instance, &mut line_count) != 0 {
            line_count = 0;
        }
        for i in 0..line_count {
            let mut line = 0i64;
            if (eng.get_line)(instance, i, &mut line) != 0 || line == 0 {
                continue;
            }
            let mut content: *const i8 = std::ptr::null();
            if (eng.get_line_content)(line, &mut content) != 0 || content.is_null() {
                continue;
            }
            let text = CStr::from_ptr(content).to_string_lossy().trim().to_string();
            if text.is_empty() {
                continue;
            }
            let (mut top, mut bottom) = (img.height as f32 / 2.0, img.height as f32 / 2.0);
            let mut bbox: *const BBox = std::ptr::null();
            if (eng.get_line_bbox)(line, &mut bbox) == 0 && !bbox.is_null() {
                let b = &*bbox;
                top = b.y1.min(b.y2).min(b.y3).min(b.y4);
                bottom = b.y1.max(b.y2).max(b.y3).max(b.y4);
            }
            items.push((top, bottom, text));
        }
        if let Some(release) = eng.release_result {
            release(instance);
        }
    }
    crate::ocr::select_by_focus(items, focus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_english() {
        if !available() {
            eprintln!("OneOCR未搭載環境のためスキップ");
            return;
        }
        let img = crate::test_util::render_text("Hello World", 400, 80);
        let (out, _) = ocr_oneocr(&img, Focus::All).expect("推論に失敗しました");
        println!("oneocr (en): {out}");
        assert!(!out.is_empty());
    }

    #[test]
    fn smoke_japanese() {
        if !available() {
            eprintln!("OneOCR未搭載環境のためスキップ");
            return;
        }
        let img = crate::test_util::render_text("こんにちは世界", 400, 80);
        let (out, _) = ocr_oneocr(&img, Focus::All).expect("推論に失敗しました");
        println!("oneocr (ja): {out}");
        assert!(!out.is_empty());
    }
}
