// ローカルONNX翻訳の推論 (ort クレートによる ONNX Runtime 連携)
// opus-mt-ja-en / opus-mt-en-jap (Xenova配布のONNX量子化モデル) による貪欲法(greedy)デコード。
// KVキャッシュは使用せず、各生成ステップでデコーダ入力列全体を毎回計算し直す簡易実装
// (系列長に対して計算量はO(n^2)だが、キャッシュ管理が不要でシンプルかつ確実に動作する)。
use crate::util;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokenizers::Tokenizer;

const NUM_LAYERS: usize = 6;
const NUM_HEADS: i64 = 8;
const HEAD_DIM: i64 = 64;
/// 生成する最大トークン数(これを超えたら打ち切る)
const MAX_NEW_TOKENS: usize = 128;

struct DirCfg {
    encoder_file: &'static str,
    decoder_file: &'static str,
    tokenizer_file: &'static str,
    /// 文末トークンID
    eos_id: i64,
    /// デコーダ開始トークンID(このモデルではpad_token_idと同一)。
    /// generation_config.json の bad_words_ids に相当し、生成候補からは除外する。
    decoder_start_id: i64,
}

const JA_TO_EN: DirCfg = DirCfg {
    encoder_file: "ja_en_encoder.onnx",
    decoder_file: "ja_en_decoder.onnx",
    tokenizer_file: "ja_en_tokenizer.json",
    eos_id: 0,
    decoder_start_id: 60715,
};
const EN_TO_JA: DirCfg = DirCfg {
    encoder_file: "en_ja_encoder.onnx",
    decoder_file: "en_ja_decoder.onnx",
    tokenizer_file: "en_ja_tokenizer.json",
    eos_id: 0,
    decoder_start_id: 46275,
};

struct Engine {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: Tokenizer,
}

static JA_EN: OnceLock<Result<Engine, String>> = OnceLock::new();
static EN_JA: OnceLock<Result<Engine, String>> = OnceLock::new();

fn models_dir() -> PathBuf {
    util::config_dir().join("models").join("onnx_translate")
}

/// Xenova配布のtokenizer.jsonはPrecompiledノーマライザのcharsmapを含まないため、
/// そのままでは構築に失敗しうる。該当ノーマライザを無効化してから読み込む。
fn load_tokenizer(path: &Path) -> Result<Tokenizer, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("トークナイザの読込に失敗しました: {e}"))?;
    let mut json: JsonValue =
        serde_json::from_slice(&bytes).map_err(|e| format!("トークナイザの解析に失敗しました: {e}"))?;
    if let Some(n) = json.get_mut("normalizer")
        && n.get("type").and_then(|t| t.as_str()) == Some("Precompiled") {
            *n = JsonValue::Null;
        }
    let bytes = serde_json::to_vec(&json).map_err(|e| e.to_string())?;
    Tokenizer::from_bytes(&bytes).map_err(|e| format!("トークナイザの構築に失敗しました: {e}"))
}

fn load_engine(cfg: &DirCfg) -> Result<Engine, String> {
    let base = models_dir();
    let encoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(base.join(cfg.encoder_file)))
        .map_err(|e| format!("エンコーダモデルの読込に失敗しました: {e}"))?;
    let decoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(base.join(cfg.decoder_file)))
        .map_err(|e| format!("デコーダモデルの読込に失敗しました: {e}"))?;
    let tokenizer = load_tokenizer(&base.join(cfg.tokenizer_file))?;
    Ok(Engine { encoder: Mutex::new(encoder), decoder: Mutex::new(decoder), tokenizer })
}

fn engine(to_japanese: bool) -> Result<&'static Engine, String> {
    let (cfg, cell): (&'static DirCfg, &'static OnceLock<Result<Engine, String>>) =
        if to_japanese { (&EN_TO_JA, &EN_JA) } else { (&JA_TO_EN, &JA_EN) };
    cell.get_or_init(|| load_engine(cfg)).as_ref().map_err(|e| e.clone())
}

/// ja↔en のローカルONNX翻訳を実行する。to_japanese=true なら英→日、false なら日→英。
pub fn translate(text: &str, to_japanese: bool) -> Result<String, String> {
    if !crate::onnx_translate_install::installed() {
        return Err("ローカル翻訳モデルが未導入です。設定画面からインストールしてください".into());
    }
    let cfg = if to_japanese { &EN_TO_JA } else { &JA_TO_EN };
    let eng = engine(to_japanese)?;
    run(eng, cfg, text)
}

fn run(eng: &Engine, cfg: &DirCfg, text: &str) -> Result<String, String> {
    let encoding =
        eng.tokenizer.encode(text, true).map_err(|e| format!("トークナイズに失敗しました: {e}"))?;
    let ids: Vec<i64> = encoding.get_ids().iter().map(|&i| i as i64).collect();
    if ids.is_empty() {
        return Err("翻訳対象のテキストがありません".into());
    }
    let seq_len = ids.len() as i64;
    let attn: Vec<i64> = vec![1; ids.len()];

    // エンコーダは一度だけ実行し、隠れ状態を全生成ステップで使い回す
    let (enc_hidden, d_model): (Vec<f32>, i64) = {
        let mut encoder =
            eng.encoder.lock().map_err(|_| "エンコーダのロックに失敗しました".to_string())?;
        let input_ids = TensorRef::from_array_view((vec![1i64, seq_len], ids.as_slice()))
            .map_err(|e| format!("入力テンソルの作成に失敗しました: {e}"))?;
        let attention_mask = TensorRef::from_array_view((vec![1i64, seq_len], attn.as_slice()))
            .map_err(|e| format!("入力テンソルの作成に失敗しました: {e}"))?;
        let outputs = encoder
            .run(ort::inputs!["input_ids" => input_ids, "attention_mask" => attention_mask])
            .map_err(|e| format!("エンコーダの実行に失敗しました: {e}"))?;
        let (shape, data) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("エンコーダ出力の取得に失敗しました: {e}"))?;
        (data.to_vec(), shape[2])
    };

    let mut decoder_ids: Vec<i64> = vec![cfg.decoder_start_id];
    let mut decoder =
        eng.decoder.lock().map_err(|_| "デコーダのロックに失敗しました".to_string())?;

    // キャッシュ未使用のダミー past_key_values (系列長0の空テンソル)。
    // 0要素データを持つテンソルは `from_array_view` の "raw data" 経路では作成できないため
    // (全次元 >= 1 の制約がある)、アロケータで直接確保する。24箇所すべてで使い回す。
    let empty_kv: Tensor<f32> = {
        let allocator = decoder.allocator();
        Tensor::new(allocator, vec![1i64, NUM_HEADS, 0i64, HEAD_DIM])
            .map_err(|e| format!("空テンソルの作成に失敗しました: {e}"))?
    };

    for _ in 0..MAX_NEW_TOKENS {
        let dec_len = decoder_ids.len() as i64;
        let mut inputs = ort::inputs![
            "encoder_attention_mask" => TensorRef::from_array_view((vec![1i64, seq_len], attn.as_slice()))
                .map_err(|e| e.to_string())?,
            "input_ids" => TensorRef::from_array_view((vec![1i64, dec_len], decoder_ids.as_slice()))
                .map_err(|e| e.to_string())?,
            "encoder_hidden_states" => TensorRef::from_array_view((vec![1i64, seq_len, d_model], enc_hidden.as_slice()))
                .map_err(|e| e.to_string())?,
            "use_cache_branch" => Tensor::from_array((vec![1i64], vec![false]))
                .map_err(|e| e.to_string())?,
        ];
        for l in 0..NUM_LAYERS {
            for kind in ["decoder", "encoder"] {
                for part in ["key", "value"] {
                    inputs.push((format!("past_key_values.{l}.{kind}.{part}").into(), empty_kv.view().into()));
                }
            }
        }

        let outputs =
            decoder.run(inputs).map_err(|e| format!("デコーダの実行に失敗しました: {e}"))?;
        let (shape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("デコーダ出力の取得に失敗しました: {e}"))?;
        let vocab = shape[2] as usize;
        let last_pos = (shape[1] as usize - 1) * vocab;
        let row = &logits[last_pos..last_pos + vocab];

        let mut best_id = 0usize;
        let mut best_val = f32::MIN;
        for (i, &v) in row.iter().enumerate() {
            // 開始/パディングトークン自身の生成は禁止 (HF generation_config の bad_words_ids 相当)
            if i as i64 == cfg.decoder_start_id {
                continue;
            }
            if v > best_val {
                best_val = v;
                best_id = i;
            }
        }
        if best_id as i64 == cfg.eos_id {
            break;
        }
        decoder_ids.push(best_id as i64);
    }

    let out_ids: Vec<u32> = decoder_ids[1..].iter().map(|&i| i as u32).collect();
    let text = eng
        .tokenizer
        .decode(&out_ids, true)
        .map_err(|e| format!("デコードに失敗しました: {e}"))?;
    let text = text.trim().to_string();
    if text.is_empty() { Err("翻訳結果が空でした".into()) } else { Ok(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ja_to_en_smoke() {
        if !crate::onnx_translate_install::installed() {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let out = translate("こんにちは、元気ですか？", false).unwrap();
        println!("ja->en: {out}");
        assert!(!out.is_empty());
    }

    #[test]
    fn en_to_ja_smoke() {
        if !crate::onnx_translate_install::installed() {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let out = translate("Thank you very much.", true).unwrap();
        println!("en->ja: {out}");
        assert!(!out.is_empty());
    }
}
