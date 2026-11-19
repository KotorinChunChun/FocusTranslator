// llama.cpp (llama-server.exe) 本体と Gemma 4 E2B GGUF モデルの導入 (SPECv0.5.2追補)
// バイナリはGitHub Releasesの最新版を都度APIで解決してダウンロードする(zipはCIのビルド番号を
// 含むファイル名で配布されているため、固定URLでは古くなる)。CPU版(win-cpu-x64)のみ対応。
// モデルはHugging Face配布のGGUF (Q4_0量子化, 約2.8GB) を直接ダウンロードする。
use crate::util;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
/// Windows CPU版バイナリのzipファイル名に含まれる目印
const WIN_CPU_ASSET_MARKER: &str = "bin-win-cpu-x64.zip";
/// Windows Radeon(HIP/ROCm)版バイナリのzipファイル名に含まれる目印
const WIN_RADEON_ASSET_MARKER: &str = "bin-win-hip-radeon-x64.zip";

/// llama.cppのビルド種別 (SPECv0.5.5: インストール時にユーザーが選択する)。
/// 実行ファイル自体(llama-server.exe)はどの種別でも同名・同じ呼び出し方をするため、
/// 起動・停止・API呼び出し側(llama_server.rs)は種別を意識しない。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LlamaVariant {
    /// 全環境で動作する既定版
    Cpu,
    /// NVIDIA GPU向け (要CUDA対応GPU)
    Cuda,
    /// AMD GPU向け (HIP/ROCm)
    Radeon,
}

impl LlamaVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            LlamaVariant::Cpu => "cpu",
            LlamaVariant::Cuda => "cuda",
            LlamaVariant::Radeon => "radeon",
        }
    }
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "cpu" => Some(LlamaVariant::Cpu),
            "cuda" => Some(LlamaVariant::Cuda),
            "radeon" => Some(LlamaVariant::Radeon),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LlamaVariant::Cpu => "CPU版",
            LlamaVariant::Cuda => "CUDA版 (NVIDIA GPU)",
            LlamaVariant::Radeon => "Radeon版 (AMD GPU / HIP)",
        }
    }
}

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
const MMPROJ_URL: &str = "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/mmproj-gemma-4-E2B-it-Q8_0.gguf";
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

/// 導入済みバイナリのバージョン情報(GitHub Releaseの tag_name / published_at)を記録する
/// マーカーファイル (SPECv0.5.5: 更新確認のため導入時のバージョンを残しておく)。
fn version_marker_path() -> PathBuf {
    bin_dir().join("version.txt")
}

/// 導入済みバイナリの (tag_name, published_at) を返す。マーカーが無い場合
/// (v0.5.4以前に導入したユーザー等)は None。
pub fn installed_version() -> Option<(String, String)> {
    let text = std::fs::read_to_string(version_marker_path()).ok()?;
    let mut lines = text.lines();
    let tag = lines.next()?.trim().to_string();
    let published = lines.next().unwrap_or("").trim().to_string();
    if tag.is_empty() {
        None
    } else {
        Some((tag, published))
    }
}

/// 導入済みバイナリのビルド種別。マーカーの3行目 (SPECv0.5.5で追加)。
/// v0.5.4以前に導入した場合や旧マーカーには記録が無いため None (=不明。CPU版とみなして扱う)。
pub fn installed_variant() -> Option<LlamaVariant> {
    let text = std::fs::read_to_string(version_marker_path()).ok()?;
    LlamaVariant::from_str_opt(text.lines().nth(2)?.trim())
}

/// バージョンマーカーファイルを書き出す (3行目にビルド種別を記録。SPECv0.5.5)
fn write_version_marker(tag_name: &str, published_at: &str, variant: LlamaVariant) {
    let _ = std::fs::write(
        version_marker_path(),
        format!("{tag_name}\n{published_at}\n{}\n", variant.as_str()),
    );
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
    if trimmed.is_empty() {
        model_path()
    } else {
        PathBuf::from(trimmed)
    }
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
    if trimmed.is_empty() {
        mmproj_path()
    } else {
        PathBuf::from(trimmed)
    }
}

/// GitHub Releasesの最新リリース情報 (SPECv0.5.5: 更新確認のためversion/公開日も保持する)
pub struct LatestRelease {
    /// 例: "b1234"
    pub tag_name: String,
    /// ISO8601形式の公開日時 (例: "2026-06-01T12:00:00Z")
    pub published_at: String,
    /// ダウンロードすべきzipのURL一覧。CPU/Radeonは1つ、CUDAは本体+cudart(ランタイム
    /// 再頒布パッケージ)の2つ (SPECv0.5.5: llama.cppのCUDA版はランタイムDLLが別配布のため)。
    pub zip_urls: Vec<String>,
}

/// アセット一覧から条件に合う最初のダウンロードURLを探す
fn find_asset_url(assets: &[serde_json::Value], pred: impl Fn(&str) -> bool) -> Option<String> {
    assets.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        if pred(name) {
            a["browser_download_url"].as_str().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// ビルド種別に応じて、ダウンロードすべきzip URL一覧(CUDAのみ2件)を解決する。
/// CUDA版は必要ドライバのバージョンが低いほうが互換性が広いため、公開されている
/// CUDAツールキットバージョンのうち最小のものを既定候補にする。
fn resolve_zip_urls(
    variant: LlamaVariant,
    assets: &[serde_json::Value],
) -> Result<Vec<String>, String> {
    match variant {
        LlamaVariant::Cpu => find_asset_url(assets, |n| n.ends_with(WIN_CPU_ASSET_MARKER))
            .map(|u| vec![u])
            .ok_or_else(|| "Windows CPU版のバイナリが見つかりませんでした".to_string()),
        LlamaVariant::Radeon => find_asset_url(assets, |n| n.ends_with(WIN_RADEON_ASSET_MARKER))
            .map(|u| vec![u])
            .ok_or_else(|| "Windows Radeon(HIP)版のバイナリが見つかりませんでした".to_string()),
        LlamaVariant::Cuda => {
            let mut candidates: Vec<(f64, String)> = Vec::new();
            for a in assets {
                let Some(name) = a["name"].as_str() else {
                    continue;
                };
                if !name.starts_with("llama-") || !name.contains("-bin-win-cuda-") {
                    continue;
                }
                let Some(ver_str) = name
                    .split("-bin-win-cuda-")
                    .nth(1)
                    .and_then(|s| s.strip_suffix("-x64.zip"))
                else {
                    continue;
                };
                let Ok(ver) = ver_str.parse::<f64>() else {
                    continue;
                };
                candidates.push((ver, ver_str.to_string()));
            }
            let (_, ver_str) = candidates
                .into_iter()
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .ok_or_else(|| "Windows CUDA版のバイナリが見つかりませんでした".to_string())?;
            let main_url = find_asset_url(assets, |n| {
                n.contains(&format!("-bin-win-cuda-{ver_str}-x64.zip")) && n.starts_with("llama-")
            })
            .ok_or_else(|| "Windows CUDA版のバイナリが見つかりませんでした".to_string())?;
            let cudart_marker = format!("cudart-llama-bin-win-cuda-{ver_str}-x64.zip");
            let cudart_url = find_asset_url(assets, |n| n == cudart_marker).ok_or_else(|| {
                "CUDAランタイム(cudart)パッケージが見つかりませんでした".to_string()
            })?;
            Ok(vec![main_url, cudart_url])
        }
    }
}

/// GitHub Releasesの最新版情報(タグ名・公開日・指定ビルド種別のzip URL一覧)を取得する
fn fetch_latest_release(variant: LlamaVariant) -> Result<LatestRelease, String> {
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
    let tag_name = json["tag_name"].as_str().unwrap_or("").to_string();
    let published_at = json["published_at"].as_str().unwrap_or("").to_string();
    let assets = json["assets"]
        .as_array()
        .ok_or("リリース情報にアセットがありません")?;
    let zip_urls = resolve_zip_urls(variant, assets)?;
    Ok(LatestRelease {
        tag_name,
        published_at,
        zip_urls,
    })
}

/// 導入済みバージョンと最新リリースを比較する (SPECv0.5.5)。タグ名が異なる(または導入済み
/// バージョン情報が無い=v0.5.4以前の導入)場合は更新ありとして Some を返す。
/// 比較に使うビルド種別は現在導入済みのもの(不明なら CPU版とみなす)。
pub fn check_for_update() -> Result<Option<LatestRelease>, String> {
    let variant = installed_variant().unwrap_or(LlamaVariant::Cpu);
    let latest = fetch_latest_release(variant)?;
    match installed_version() {
        Some((tag, _)) if tag == latest.tag_name => Ok(None),
        _ => Ok(Some(latest)),
    }
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
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("受信中にエラーが発生しました: {e}"))?;
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
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("zipの展開に失敗しました: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // 配布zipは "build/bin/xxx.exe" のようなディレクトリ構成のことがあるため、
        // ファイル名(ベースネーム)だけを見て bin_dir 直下へフラットに展開する。
        let Some(name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|f| f.to_owned()))
        else {
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

/// zipをダウンロードして bin_dir へ展開し、成功したらバージョンマーカーを書く共通処理
/// (SPECv0.5.5: 新規導入・更新の両方で使う)。CUDA版は release.zip_urls に2件
/// (本体+cudartランタイム)入っているため、順に全て展開する。
fn download_and_extract_binary(
    release: &LatestRelease,
    variant: LlamaVariant,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    for (i, url) in release.zip_urls.iter().enumerate() {
        let zip_path = dir.join(format!("llama.part{i}.zip"));
        download_to_file(url, &zip_path, 300, &mut on_progress)?;
        let result = extract_zip(&zip_path, &dir);
        let _ = std::fs::remove_file(&zip_path);
        result?;
    }
    if !installed() {
        return Err("展開後にllama-server.exeが見つかりませんでした".into());
    }
    write_version_marker(&release.tag_name, &release.published_at, variant);
    Ok(())
}

/// llama.cpp本体を指定ビルド種別で導入する。既に導入済みなら何もしない。
/// on_progress は10秒おきに (受信済みバイト数, 合計バイト数) を通知する (SPECv0.5.3:
/// モデル/mmprojの導入と同様に設定画面へ進捗を反映するため)。
pub fn install_binary(
    variant: LlamaVariant,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    if installed() {
        return Ok(());
    }
    let release = fetch_latest_release(variant)?;
    download_and_extract_binary(&release, variant, on_progress)
}

/// 導入済みのllama.cpp本体を、指定リリース(通常は check_for_update() で見つかった最新版)へ
/// 更新する。install_binary() と異なり導入済みかどうかのガードは持たない — 呼び出し元
/// (設定画面)が「更新する」と決めた後に呼ぶための経路 (SPECv0.5.5)。
/// 呼び出し前に、サーバーが稼働中なら停止しておくこと(実行中のexeは上書きできない)。
/// variant は現在導入済みのビルド種別をそのまま引き継ぐ(更新時に種別を変えることはできない
/// — 変更したい場合は削除してから新しい種別で再導入する)。
pub fn update_binary(
    release: &LatestRelease,
    variant: LlamaVariant,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    download_and_extract_binary(release, variant, on_progress)
}

/// 導入済みのllama.cpp本体一式を削除する (SPECv0.5.5)。呼び出し前にサーバーが稼働中なら
/// 停止しておくこと(実行中のexeは削除できないため)。言語モデル/mmprojファイルは対象外
/// (別行で個別に管理しているため削除しない)。
///
/// `std::fs::remove_dir_all` は使わない: %APPDATA% が OneDrive 同期下にある環境で、
/// クラウド代替ファイル(Files On-Demandのプレースホルダ)特有の再解析ポイントを
/// 辿ろうとして失敗する既知の問題がある(実機で "OS error 4395" を確認)。
/// このディレクトリはextract_zip()が常にサブディレクトリ無しでフラットに展開する
/// 構成のため、直下のファイルを1つずつ削除してから空のディレクトリを消す方式にすれば
/// この問題を回避できる(想定外にサブディレクトリが存在した場合のみ再帰削除を試みる)。
pub fn uninstall_binary() -> Result<(), String> {
    let dir = bin_dir();
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("削除に失敗しました: {e}"))? {
        let path = entry
            .map_err(|e| format!("削除に失敗しました: {e}"))?
            .path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|e| format!("削除に失敗しました: {e}"))?;
        } else {
            std::fs::remove_file(&path).map_err(|e| format!("削除に失敗しました: {e}"))?;
        }
    }
    std::fs::remove_dir(&dir).map_err(|e| format!("削除に失敗しました: {e}"))?;
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
        return Err(
            "ダウンロードしたモデルファイルが小さすぎます(配布元の変更の可能性があります)".into(),
        );
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
        return Err(
            "ダウンロードしたmmprojファイルが小さすぎます(配布元の変更の可能性があります)".into(),
        );
    }
    Ok(())
}
