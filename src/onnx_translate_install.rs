// ローカルONNX翻訳モデルの導入確認とワンクリックインストール (SPEC §7.2, §13)
// 3種類のモデルを切替式で提供する:
//   - opus_mt: Xenova/opus-mt-ja-en, Xenova/opus-mt-en-jap (Helsinki-NLP OPUS-MTのONNX量子化版)
//   - fugu_mt: Kadonox/fugumt-ja-en-onnx, Kadonox/fugumt-en-ja-onnx (staka/fugumt-*のONNX量子化版。
//     OpusMTと同じMarian構成だがWikipedia/JParaCrawl等でファインチューニングされ技術文に強い)
//   - nllb200: Xenova/nllb-200-distilled-600M (Meta NLLB-200の蒸留600Mモデル。ja⇄en以外にも対応する
//     多言語アーキテクチャのため、方向によらずencoder/decoder/tokenizerは同一ファイルを共有する)
// 日→英・英→日の双方向、SHA256検証のうえダウンロードする。
// 推論本体は onnx_translate.rs (ort クレートによるONNX Runtime推論) を参照。
use crate::util;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;

/// ローカル翻訳モデルの種類。設定画面で切替可能。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    OpusMt,
    FuguMt,
    Nllb200,
}

impl Variant {
    pub const ALL: [Variant; 3] = [Variant::OpusMt, Variant::FuguMt, Variant::Nllb200];

    /// 設定ファイル保存用のキー文字列
    pub fn key(self) -> &'static str {
        match self {
            Variant::OpusMt => "opus_mt",
            Variant::FuguMt => "fugu_mt",
            Variant::Nllb200 => "nllb200",
        }
    }

    pub fn from_key(k: &str) -> Self {
        match k {
            "fugu_mt" => Variant::FuguMt,
            "nllb200" => Variant::Nllb200,
            _ => Variant::OpusMt,
        }
    }

    /// 設定画面に表示する名称
    pub fn display(self) -> &'static str {
        match self {
            Variant::OpusMt => "Opus-MT (既定・軽量)",
            Variant::FuguMt => "FuguMT (技術文に強い)",
            Variant::Nllb200 => "NLLB-200 distilled 600M (高精度・大容量)",
        }
    }

    /// モデルファイルの格納先サブディレクトリ。既存導入分との互換のため opus_mt は直下(空文字)のまま。
    fn subdir(self) -> &'static str {
        match self {
            Variant::OpusMt => "",
            Variant::FuguMt => "fugu_mt",
            Variant::Nllb200 => "nllb200",
        }
    }
}

struct ModelFile {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// モデルファイルは最大2GB程度を想定 (NLLB-200のdecoderが約450MB)。安全側に600MBを上限とする。
const MAX_BODY: u64 = 600 * 1024 * 1024;

const OPUS_MT_FILES: [ModelFile; 6] = [
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

/// NLLB-200は多言語モデルのため、方向(ja→en/en→ja)によらずencoder/decoder/tokenizerを共有する。
const NLLB200_FILES: [ModelFile; 3] = [
    ModelFile {
        file: "encoder.onnx",
        url: "https://huggingface.co/Xenova/nllb-200-distilled-600M/resolve/main/onnx/encoder_model_quantized.onnx",
        sha256: "5cde664eacba07a62f198857ec6c06e09572b1ebb77c8137f1fa99ac604a3a28",
    },
    ModelFile {
        file: "decoder.onnx",
        url: "https://huggingface.co/Xenova/nllb-200-distilled-600M/resolve/main/onnx/decoder_model_merged_quantized.onnx",
        sha256: "dd66608c2a4194e78f95548fa0e64f24302303698c5b09fa8e1f9e16ec00676b",
    },
    ModelFile {
        file: "tokenizer.json",
        url: "https://huggingface.co/Xenova/nllb-200-distilled-600M/resolve/main/tokenizer.json",
        sha256: "8ac789ad7dabea44d41537822d48c516ba358374c51813e2cba78c006e150c94",
    },
];

fn files(variant: Variant) -> &'static [ModelFile] {
    match variant {
        Variant::OpusMt => &OPUS_MT_FILES,
        Variant::FuguMt => &FUGU_MT_FILES,
        Variant::Nllb200 => &NLLB200_FILES,
    }
}

pub fn dir(variant: Variant) -> PathBuf {
    let base = util::config_dir().join("models").join("onnx_translate");
    let sub = variant.subdir();
    if sub.is_empty() { base } else { base.join(sub) }
}

/// 指定モデルの全ファイルが導入済みか
pub fn installed(variant: Variant) -> bool {
    let d = dir(variant);
    files(variant).iter().all(|f| d.join(f.file).is_file())
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

/// 指定モデルのファイルを順にダウンロード・検証する。既に導入済みなら何もしない。
pub fn install_variant(variant: Variant) -> Result<(), String> {
    let d = dir(variant);
    std::fs::create_dir_all(&d).map_err(|e| format!("フォルダ作成に失敗しました: {e}"))?;
    for f in files(variant) {
        if d.join(f.file).is_file() {
            continue;
        }
        fetch_one(f, &d)?;
    }
    Ok(())
}
