// ローカルONNX翻訳モデルの導入確認とワンクリックインストール (SPEC §7.2, §13)
// 配布元: Xenova/opus-mt-ja-en, Xenova/opus-mt-en-jap (HuggingFace, Helsinki-NLP OPUS-MTのONNX量子化版)
// 日→英・英→日の双方向、encoder/decoder/tokenizerの計6ファイルをSHA256検証のうえダウンロードする。
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

/// モデルファイルは最大100MB程度を想定。安全側に150MBを上限とする。
const MAX_BODY: u64 = 150 * 1024 * 1024;

const FILES: [ModelFile; 6] = [
    ModelFile {
        file: "ja_en_encoder.onnx",
        url: "https://huggingface.co/Xenova/opus-mt-ja-en/resolve/main/onnx/encoder_model_quantized.onnx",
        sha256: "345262b16bcdda1468b0f3380c112b7ce79f731176b4b1d21f6edd5b2ae0d25c",
    },
    ModelFile {
        file: "ja_en_decoder.onnx",
        url: "https://huggingface.co/Xenova/opus-mt-ja-en/resolve/main/onnx/decoder_model_merged_quantized.onnx",
        sha256: "b304d0014e4e1575437b6af95467b6cb54405d923732d8359113bd6dbbee93c0",
    },
    ModelFile {
        file: "ja_en_tokenizer.json",
        url: "https://huggingface.co/Xenova/opus-mt-ja-en/resolve/main/tokenizer.json",
        sha256: "770ff2855437cf44f1f110550c5a9dca773253a167aeac36076b2073d259aa3b",
    },
    ModelFile {
        file: "en_ja_encoder.onnx",
        url: "https://huggingface.co/Xenova/opus-mt-en-jap/resolve/main/onnx/encoder_model_quantized.onnx",
        sha256: "4062a86cbec1d388e779294f07b784179d87a648c90771e94879b3a28cd96be7",
    },
    ModelFile {
        file: "en_ja_decoder.onnx",
        url: "https://huggingface.co/Xenova/opus-mt-en-jap/resolve/main/onnx/decoder_model_merged_quantized.onnx",
        sha256: "084c7544b640eeea722b4858328f479d804236b916a2bec761442ff062726619",
    },
    ModelFile {
        file: "en_ja_tokenizer.json",
        url: "https://huggingface.co/Xenova/opus-mt-en-jap/resolve/main/tokenizer.json",
        sha256: "240dd3befcfb8727158fb23fbc8a94a41e5b827ad486601ea0805c17fb9f6fd9",
    },
];

fn dir() -> PathBuf {
    util::config_dir().join("models").join("onnx_translate")
}

/// 6ファイルすべてが導入済みか
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
        .timeout_global(Some(std::time::Duration::from_secs(180)))
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

/// 6ファイルを順にダウンロード・検証する。既に導入済みなら何もしない。
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
