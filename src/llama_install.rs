// llama.cpp (llama-server.exe) 本体と Gemma 4 E2B GGUF モデルの導入 (SPECv0.5.2追補)
// バイナリはGitHub Releasesの最新版を都度APIで解決してダウンロードする(zipはCIのビルド番号を
// 含むファイル名で配布されているため、固定URLでは古くなる)。CPU版(win-cpu-x64)のみ対応。
// モデルはHugging Face配布のGGUF (Q4_0量子化, 約2.8GB) を直接ダウンロードする。
use crate::util;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const GITHUB_LATEST_RELEASE_API: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
/// Windows CPU版バイナリのzipファイル名に含まれる目印
const WIN_CPU_ASSET_MARKER: &str = "bin-win-cpu-x64.zip";

/// 配布元: ggml-org/gemma-4-E2B-it-GGUF (Q4_0量子化, 約2.84GB)
const MODEL_URL: &str =
    "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf";
const MODEL_FILE: &str = "gemma-4-E2B-it-Q4_0.gguf";
/// チェックサムは配布元に公開情報が無いため未検証。ダウンロード完了ファイルが
/// 極端に小さい(エラーページ等)場合のみ検出できるよう最小サイズで健全性チェックする。
const MODEL_MIN_BYTES: u64 = 1_000_000_000; // 1GB

/// 画像入力対応(VLM)に必要なマルチモーダル投影ファイル。Gemma 4 E2B自体は元々
/// テキスト/画像/音声に対応したモデルのため、この mmproj を base モデルと併せて
/// llama-server へ渡すことで画像入力に対応できる (SPECv0.5.2追補: OCRのLLM経路が
/// 画像を送れず失敗する問題への対応)。
const MMPROJ_URL: &str =
    "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/mmproj-gemma-4-E2B-it-Q8_0.gguf";
const MMPROJ_FILE: &str = "mmproj-gemma-4-E2B-it-Q8_0.gguf";
const MMPROJ_MIN_BYTES: u64 = 100_000_000; // 100MB (実サイズ約557MB)

pub fn bin_dir() -> PathBuf {
    util::config_dir().join("llama").join("bin")
}

pub fn model_dir() -> PathBuf {
    util::config_dir().join("models").join("llm")
}

pub fn server_exe_path() -> PathBuf {
    bin_dir().join("llama-server.exe")
}

pub fn model_path() -> PathBuf {
    model_dir().join(MODEL_FILE)
}

/// llama-server.exe が導入済みか
pub fn installed() -> bool {
    server_exe_path().is_file()
}

/// モデルファイルが導入済みか (既定の管理下ディレクトリのみ判定。手動選択パスは
/// resolve_model_path() 経由で別途確認する)
pub fn model_installed() -> bool {
    model_path().is_file()
}

/// 実際にサーバーへ渡すモデルパスを決定する。設定で明示パスが指定されていればそれを使い
/// (LM Studio等で導入済みのGGUFを再利用する場合)、空文字なら既定の管理下ディレクトリを使う
/// (SPECv0.5.2追補)。
pub fn resolve_model_path(override_path: &str) -> PathBuf {
    let trimmed = override_path.trim();
    if trimmed.is_empty() { model_path() } else { PathBuf::from(trimmed) }
}

/// 既定のmmprojファイルパス (画像入力対応用)
pub fn mmproj_path() -> PathBuf {
    model_dir().join(MMPROJ_FILE)
}

/// mmprojファイルが導入済みか (既定の管理下ディレクトリのみ判定)
pub fn mmproj_installed() -> bool {
    mmproj_path().is_file()
}

/// resolve_model_path() のmmproj版
pub fn resolve_mmproj_path(override_path: &str) -> PathBuf {
    let trimmed = override_path.trim();
    if trimmed.is_empty() { mmproj_path() } else { PathBuf::from(trimmed) }
}

/// GitHub Releasesの最新版からWindows CPU版zipのダウンロードURLを取得する
fn resolve_latest_zip_url() -> Result<String, String> {
    let mut res = ureq::get(GITHUB_LATEST_RELEASE_API)
        .header("User-Agent", "FocusTranslator")
        .header("Accept", "application/vnd.github+json")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .call()
        .map_err(|e| format!("llama.cppの最新リリース情報の取得に失敗しました: {e}"))?;
    let json: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("リリース情報の解析に失敗しました: {e}"))?;
    let assets = json["assets"].as_array().ok_or("リリース情報にアセットがありません")?;
    assets
        .iter()
        .find_map(|a| {
            let name = a["name"].as_str()?;
            if name.ends_with(WIN_CPU_ASSET_MARKER) {
                a["browser_download_url"].as_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| "Windows CPU版のバイナリが見つかりませんでした".to_string())
}

/// URLから target_path へストリームでダウンロードする(全文をメモリに載せない)。
/// on_progress には (受信済みバイト数, 判明していれば合計バイト数) を10秒おきに通知する
/// (SPECv0.5.2追補: 大きなモデルファイルのダウンロード状況を設定画面へ反映するため)。
/// 失敗時は途中生成物(.part)を削除する。
fn download_to_file(
    url: &str,
    target_path: &Path,
    timeout_secs: u64,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let mut res = ureq::get(url)
        .header("User-Agent", "FocusTranslator")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build()
        .call()
        .map_err(|e| format!("ダウンロードに失敗しました: {e}"))?;
    let total: Option<u64> = res
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());
    let tmp = target_path.with_extension("part");
    let result: Result<(), String> = (|| {
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut reader = res.body_mut().as_reader();
        let mut buf = [0u8; 64 * 1024];
        let mut downloaded = 0u64;
        let mut last_report = Instant::now();
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("受信中にエラーが発生しました: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            downloaded += n as u64;
            if last_report.elapsed() >= Duration::from_secs(10) {
                on_progress(downloaded, total);
                last_report = Instant::now();
            }
        }
        on_progress(downloaded, total);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return result;
    }
    std::fs::rename(&tmp, target_path).map_err(|e| e.to_string())?;
    Ok(())
}

/// zipアーカイブを展開する(トップレベルのファイル/フォルダをすべて target_dir 直下へ展開)。
fn extract_zip(zip_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zipの展開に失敗しました: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // 配布zipは "build/bin/xxx.exe" のようなディレクトリ構成のことがあるため、
        // ファイル名(ベースネーム)だけを見て bin_dir 直下へフラットに展開する。
        let Some(name) = entry.enclosed_name().and_then(|p| p.file_name().map(|f| f.to_owned())) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let out_path = target_dir.join(name);
        let mut out = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// llama.cpp本体(CPU版)を導入する。既に導入済みなら何もしない。
/// on_progress は10秒おきに (受信済みバイト数, 合計バイト数) を通知する (SPECv0.5.3:
/// モデル/mmprojの導入と同様に設定画面へ進捗を反映するため)。
pub fn install_binary(on_progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    if installed() {
        return Ok(());
    }
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    let url = resolve_latest_zip_url()?;
    let zip_path = dir.join("llama.part.zip");
    download_to_file(&url, &zip_path, 300, on_progress)?;
    let result = extract_zip(&zip_path, &dir);
    let _ = std::fs::remove_file(&zip_path);
    result?;
    if !installed() {
        return Err("展開後にllama-server.exeが見つかりませんでした".into());
    }
    Ok(())
}

/// Gemma 4 E2B (Q4_0 GGUF) モデルを導入する。既に導入済みなら何もしない。
/// on_progress は10秒おきに (受信済みバイト数, 合計バイト数) を通知する (SPECv0.5.2追補)。
pub fn install_model(on_progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    if model_installed() {
        return Ok(());
    }
    let dir = model_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    let target = model_path();
    download_to_file(MODEL_URL, &target, 1800, on_progress)?;
    let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    if size < MODEL_MIN_BYTES {
        let _ = std::fs::remove_file(&target);
        return Err("ダウンロードしたモデルファイルが小さすぎます(配布元の変更の可能性があります)".into());
    }
    Ok(())
}

/// mmproj(画像入力対応)ファイルを導入する。既に導入済みなら何もしない。
pub fn install_mmproj(on_progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    if mmproj_installed() {
        return Ok(());
    }
    let dir = model_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    let target = mmproj_path();
    download_to_file(MMPROJ_URL, &target, 1800, on_progress)?;
    let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    if size < MMPROJ_MIN_BYTES {
        let _ = std::fs::remove_file(&target);
        return Err("ダウンロードしたmmprojファイルが小さすぎます(配布元の変更の可能性があります)".into());
    }
    Ok(())
}

/// バイナリ・モデルの両方を導入する(設定画面の1ボタン導入用ではなく、ボタンを分けて
/// 導入する現行UIでは個別に呼ばれる。将来の一括導入用に残す)。
#[allow(dead_code)]
pub fn install_all() -> Result<(), String> {
    install_binary(|_, _| {})?;
    install_model(|_, _| {})?;
    Ok(())
}

