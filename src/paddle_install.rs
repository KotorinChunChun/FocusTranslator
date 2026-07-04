// PaddleOCR (RapidOCR配布 ONNX) モデルの導入確認とワンクリックインストール (SPEC §7.1, §13)
// 検出/認識/辞書の3ファイルを SHA256 検証のうえダウンロードする。
// 配布元: RapidAI/RapidOCR (ModelScope, PP-OCRv4 mobile)。日本語+ラテン文字の横書きを想定。
// ONNX Runtime による推論本体は paddle_ocr モジュールを参照。ここではモデル導入までを担う。
use crate::util;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;

/// モデルファイルは最大20MB程度を想定。安全側に50MBを上限とする。
const MAX_BODY: u64 = 50 * 1024 * 1024;

struct ModelFile {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const FILES: [ModelFile; 3] = [
    ModelFile {
        file: "det.onnx",
        url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.1/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx",
        sha256: "d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9",
    },
    ModelFile {
        file: "rec.onnx",
        url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.1/onnx/PP-OCRv4/rec/japan_PP-OCRv4_rec_mobile.onnx",
        sha256: "e1075a67dba758ecfc7ebc78a10ae61c95ac8fb66a9c86fab5541e33f085cb7a",
    },
    ModelFile {
        file: "dict.txt",
        url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.1/paddle/PP-OCRv4/rec/japan_PP-OCRv4_rec_mobile/japan_dict.txt",
        sha256: "1dcfcb41eec90576a945b3084f22ade11ced506e24f14879245b071698f308e8",
    },
];

pub fn dir() -> PathBuf {
    util::config_dir().join("models").join("paddleocr")
}

/// 3ファイルすべてが導入済みか(インストール完了時のみ本体を配置するため存在確認で十分)
pub fn installed() -> bool {
    FILES.iter().all(|f| dir().join(f.file).is_file())
}

fn sha256_hex(path: &std::path::Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 1ファイルをダウンロードしてSHA256検証後に配置する。失敗時は一時ファイルを残さない。
fn fetch_one(f: &ModelFile, target_dir: &std::path::Path) -> Result<(), String> {
    let mut res = ureq::get(f.url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .build()
        .call()
        .map_err(|e| format!("{} のダウンロードに失敗しました: {e}", f.file))?;
    let body = res
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|e| format!("{} の受信に失敗しました: {e}", f.file))?;

    let tmp = target_dir.join(format!("{}.part", f.file));
    {
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        out.write_all(&body).map_err(|e| e.to_string())?;
    }
    let actual = sha256_hex(&tmp)?;
    if actual != f.sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{} のチェックサムが一致しません(配布元が更新された可能性があります)",
            f.file
        ));
    }
    std::fs::rename(&tmp, target_dir.join(f.file)).map_err(|e| e.to_string())?;
    Ok(())
}

/// 3ファイルを順にダウンロード・検証する。既に導入済みなら何もしない。
pub fn install() -> Result<(), String> {
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    for f in &FILES {
        if d.join(f.file).is_file() {
            continue;
        }
        fetch_one(f, &d)?;
    }
    Ok(())
}
