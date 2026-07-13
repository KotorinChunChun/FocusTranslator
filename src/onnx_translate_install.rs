// ローカルONNX翻訳モデル(FuguMT)の導入確認とワンクリックインストール (SPEC §7.2, §13)
//   - fugu_mt: Kadonox/fugumt-ja-en-onnx, Kadonox/fugumt-en-ja-onnx (staka/fugumt-*のONNX量子化版。
//     Marian構成をWikipedia/JParaCrawl等でファインチューニングし技術文に強い)
// 日→英・英→日の双方向、SHA256検証のうえダウンロードする。
// 推論本体は onnx_translate.rs (ort クレートによるONNX Runtime推論) を参照。
use crate::util;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;

struct ModelFile {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// FuguMT のファイルは最大でも数十MB程度。安全側に200MBを上限とする。
const MAX_BODY: u64 = 200 * 1024 * 1024;

const FUGU_MT_FILES: [ModelFile; 6] = [
    ModelFile {
        file: "ja_en_encoder.onnx",
        url: "https://huggingface.co/Kadonox/fugumt-ja-en-onnx/resolve/main/onnx/encoder_model_quantized.onnx",
        sha256: "caea82d93435b76cd01edcb9dfb6dbfce82bde435bdc739a198a298014884b06",
    },
    ModelFile {
        file: "ja_en_decoder.onnx",
        url: "https://huggingface.co/Kadonox/fugumt-ja-en-onnx/resolve/main/onnx/decoder_model_merged_quantized.onnx",
        sha256: "faf90e39de2b027461f09297f7edc83734c24c00bcfb1482c7ec1ca77e84a989",
    },
    ModelFile {
        file: "ja_en_tokenizer.json",
        url: "https://huggingface.co/Kadonox/fugumt-ja-en-onnx/resolve/main/tokenizer.json",
        sha256: "e0bb9bef12bb06c118b2bcf7e6ec5975a0dfa3d89191df6cc331767b1643af78",
    },
    ModelFile {
        file: "en_ja_encoder.onnx",
        url: "https://huggingface.co/Kadonox/fugumt-en-ja-onnx/resolve/main/onnx/encoder_model_quantized.onnx",
        sha256: "60b290662bcb83d7ac1f20749dcbac95d4ff92752389ae418fd514ef0d4ce50b",
    },
    ModelFile {
        file: "en_ja_decoder.onnx",
        url: "https://huggingface.co/Kadonox/fugumt-en-ja-onnx/resolve/main/onnx/decoder_model_merged_quantized.onnx",
        sha256: "669dbec3443a70f278b9e713598751d0f34d5b0b28aeab8d0f1a9f20ccd87c97",
    },
    ModelFile {
        file: "en_ja_tokenizer.json",
        url: "https://huggingface.co/Kadonox/fugumt-en-ja-onnx/resolve/main/tokenizer.json",
        sha256: "d69293c4be36a8396f33befcc2ab4f2badeb048a33d824a104c794e11726cb7b",
    },
];

pub fn dir() -> PathBuf {
    util::config_dir().join("models").join("onnx_translate").join("fugu_mt")
}

/// 全ファイルが導入済みか
pub fn installed() -> bool {
    let d = dir();
    FUGU_MT_FILES.iter().all(|f| d.join(f.file).is_file())
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
        .timeout_global(Some(std::time::Duration::from_secs(300)))
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

/// FuguMTのファイルを順にダウンロード・検証する。既に導入済みなら何もしない。
pub fn install() -> Result<(), String> {
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    for f in &FUGU_MT_FILES {
        if d.join(f.file).is_file() {
            continue;
        }
        fetch_one(f, &d)?;
    }
    Ok(())
}
