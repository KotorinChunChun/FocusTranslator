// ローカルONNX翻訳の推論 (ort クレートによる ONNX Runtime 連携)
// 3種類のモデルに対応する:
//   - Opus-MT / FuguMT: Marian系アーキテクチャ。decoder_start_token_idを起点に貪欲法(greedy)デコード。
//   - NLLB-200 distilled 600M: M2M100系アーキテクチャ。入力の先頭に原文言語トークンを付与し、
//     生成1トークン目に訳先言語トークンを強制する(forced_bos_token_id)必要がある。
// KVキャッシュは使用せず、各生成ステップでデコーダ入力列全体を毎回計算し直す簡易実装
// (系列長に対して計算量はO(n^2)だが、キャッシュ管理が不要でシンプルかつ確実に動作する)。
use crate::onnx_translate_install::Variant;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;

/// 生成する最大トークン数(これを超えたら打ち切る)
const MAX_NEW_TOKENS: usize = 128;

/// NLLB-200 の言語コードトークンID (tokenizer.json の added_tokens より)
const NLLB_ENG_LATN: i64 = 256047;
const NLLB_JPN_JPAN: i64 = 256079;

struct DirCfg {
    encoder_file: &'static str,
    decoder_file: &'static str,
    tokenizer_file: &'static str,
    /// 文末トークンID
    eos_id: i64,
    /// デコーダ開始トークンID。Marian系ではpad_token_idと同一(generation_config.jsonの
    /// bad_words_idsに相当し生成候補から除外)。NLLBではeos_token_idと同一の値になる。
    decoder_start_id: i64,
    num_layers: usize,
    num_heads: i64,
    head_dim: i64,
    /// NLLBのみ: 入力の先頭に付与する原文言語トークンID。Marian系はNone。
    src_lang_id: Option<i64>,
    /// NLLBのみ: 生成1トークン目に強制する訳先言語トークンID。Marian系はNone。
    forced_bos_id: Option<i64>,
}

fn dir_cfg(variant: Variant, to_japanese: bool) -> DirCfg {
    match variant {
        Variant::OpusMt => {
            if to_japanese {
                DirCfg {
                    encoder_file: "en_ja_encoder.onnx",
                    decoder_file: "en_ja_decoder.onnx",
                    tokenizer_file: "en_ja_tokenizer.json",
                    eos_id: 0,
                    decoder_start_id: 46275,
                    num_layers: 6,
                    num_heads: 8,
                    head_dim: 64,
                    src_lang_id: None,
                    forced_bos_id: None,
                }
            } else {
                DirCfg {
                    encoder_file: "ja_en_encoder.onnx",
                    decoder_file: "ja_en_decoder.onnx",
                    tokenizer_file: "ja_en_tokenizer.json",
                    eos_id: 0,
                    decoder_start_id: 60715,
                    num_layers: 6,
                    num_heads: 8,
                    head_dim: 64,
                    src_lang_id: None,
                    forced_bos_id: None,
                }
            }
        }
        Variant::FuguMt => {
            if to_japanese {
                DirCfg {
                    encoder_file: "en_ja_encoder.onnx",
                    decoder_file: "en_ja_decoder.onnx",
                    tokenizer_file: "en_ja_tokenizer.json",
                    eos_id: 0,
                    decoder_start_id: 32000,
                    num_layers: 6,
                    num_heads: 8,
                    head_dim: 64,
                    src_lang_id: None,
                    forced_bos_id: None,
                }
            } else {
                DirCfg {
                    encoder_file: "ja_en_encoder.onnx",
                    decoder_file: "ja_en_decoder.onnx",
                    tokenizer_file: "ja_en_tokenizer.json",
                    eos_id: 0,
                    decoder_start_id: 32000,
                    num_layers: 6,
                    num_heads: 8,
                    head_dim: 64,
                    src_lang_id: None,
                    forced_bos_id: None,
                }
            }
        }
        Variant::Nllb200 => {
            // 方向によらずencoder/decoder/tokenizerは同一。言語トークンのみ入れ替える。
            let (src, tgt) =
                if to_japanese { (NLLB_ENG_LATN, NLLB_JPN_JPAN) } else { (NLLB_JPN_JPAN, NLLB_ENG_LATN) };
            DirCfg {
                encoder_file: "encoder.onnx",
                decoder_file: "decoder.onnx",
                tokenizer_file: "tokenizer.json",
                eos_id: 2,
                decoder_start_id: 2,
                num_layers: 12,
                num_heads: 16,
                head_dim: 64,
                src_lang_id: Some(src),
                forced_bos_id: Some(tgt),
            }
        }
    }
}

struct Engine {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: Tokenizer,
}

/// 読込済みエンジンのキャッシュ。NLLBは方向によらず同一ファイルを参照するため、
/// エンコーダファイルの絶対パスをキーにして自然に共有される。
static ENGINES: Mutex<Option<HashMap<String, Arc<Engine>>>> = Mutex::new(None);

fn models_dir(variant: Variant) -> PathBuf {
    crate::onnx_translate_install::dir(variant)
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

fn load_engine(cfg: &DirCfg, base: &Path) -> Result<Engine, String> {
    let encoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(base.join(cfg.encoder_file)))
        .map_err(|e| format!("エンコーダモデルの読込に失敗しました: {e}"))?;
    let decoder = Session::builder()
        .and_then(|mut b| b.commit_from_file(base.join(cfg.decoder_file)))
        .map_err(|e| format!("デコーダモデルの読込に失敗しました: {e}"))?;
    let tokenizer = load_tokenizer(&base.join(cfg.tokenizer_file))?;
    Ok(Engine { encoder: Mutex::new(encoder), decoder: Mutex::new(decoder), tokenizer })
}

fn engine_for(cfg: &DirCfg, base: &Path) -> Result<Arc<Engine>, String> {
    let key = base.join(cfg.encoder_file).to_string_lossy().to_string();
    let mut guard = ENGINES.lock().map_err(|_| "エンジンキャッシュのロックに失敗しました".to_string())?;
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(e) = map.get(&key) {
        return Ok(e.clone());
    }
    let eng = Arc::new(load_engine(cfg, base)?);
    map.insert(key, eng.clone());
    Ok(eng)
}

/// ja↔en のローカルONNX翻訳を実行する。to_japanese=true なら英→日、false なら日→英。
pub fn translate(text: &str, to_japanese: bool, variant: Variant) -> Result<String, String> {
    if !crate::onnx_translate_install::installed(variant) {
        return Err("ローカル翻訳モデルが未導入です。設定画面からインストールしてください".into());
    }
    let cfg = dir_cfg(variant, to_japanese);
    let base = models_dir(variant);
    let eng = engine_for(&cfg, &base)?;
    run(&eng, &cfg, text)
}

fn run(eng: &Engine, cfg: &DirCfg, text: &str) -> Result<String, String> {
    let ids: Vec<i64> = if let Some(src) = cfg.src_lang_id {
        // NLLB: [原文言語トークン] + 本文 + [eos] を手動で組み立てる
        // (fast tokenizerの既定post-processorは固定言語ペア用のため使わない)
        let encoding =
            eng.tokenizer.encode(text, false).map_err(|e| format!("トークナイズに失敗しました: {e}"))?;
        let mut v = vec![src];
        v.extend(encoding.get_ids().iter().map(|&i| i as i64));
        v.push(cfg.eos_id);
        v
    } else {
        let encoding =
            eng.tokenizer.encode(text, true).map_err(|e| format!("トークナイズに失敗しました: {e}"))?;
        encoding.get_ids().iter().map(|&i| i as i64).collect()
    };
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
    if let Some(forced) = cfg.forced_bos_id {
        // NLLB: 生成1トークン目は訳先言語トークンに固定する
        decoder_ids.push(forced);
    }
    let mut decoder =
        eng.decoder.lock().map_err(|_| "デコーダのロックに失敗しました".to_string())?;

    // キャッシュ未使用のダミー past_key_values (系列長0の空テンソル)。
    // 0要素データを持つテンソルは `from_array_view` の "raw data" 経路では作成できないため
    // (全次元 >= 1 の制約がある)、アロケータで直接確保する。全レイヤー分使い回す。
    let empty_kv: Tensor<f32> = {
        let allocator = decoder.allocator();
        Tensor::new(allocator, vec![1i64, cfg.num_heads, 0i64, cfg.head_dim])
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
        for l in 0..cfg.num_layers {
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
            // 開始/パディングトークン自身の生成は禁止 (HF generation_config の bad_words_ids 相当)。
            // ただしNLLBのように開始トークンとeosトークンが同一の場合はこの禁止を適用しない
            // (適用すると正しい終端判定ができなくなるため)。
            if cfg.decoder_start_id != cfg.eos_id && i as i64 == cfg.decoder_start_id {
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

    let skip = 1 + if cfg.forced_bos_id.is_some() { 1 } else { 0 };
    let out_ids: Vec<u32> = decoder_ids[skip..].iter().map(|&i| i as u32).collect();
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

    /// FOCUSTRANSLATOR_DATA_DIR を切り替えるテスト(logdb)との干渉を防ぐ
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::util::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn ja_to_en_smoke() {
        let _guard = env_lock();
        if !crate::onnx_translate_install::installed(Variant::OpusMt) {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let out = translate("こんにちは、元気ですか？", false, Variant::OpusMt).unwrap();
        println!("ja->en: {out}");
        assert!(!out.is_empty());
    }

    #[test]
    fn en_to_ja_smoke() {
        let _guard = env_lock();
        if !crate::onnx_translate_install::installed(Variant::OpusMt) {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let out = translate("Thank you very much.", true, Variant::OpusMt).unwrap();
        println!("en->ja: {out}");
        assert!(!out.is_empty());
    }

    #[test]
    fn fugu_mt_smoke() {
        let _guard = env_lock();
        if !crate::onnx_translate_install::installed(Variant::FuguMt) {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let out = translate("Compiling focus-translator v0.1.0", true, Variant::FuguMt).unwrap();
        println!("fugu en->ja: {out}");
        assert!(!out.is_empty());
        let out2 = translate("こんにちは、元気ですか？", false, Variant::FuguMt).unwrap();
        println!("fugu ja->en: {out2}");
        assert!(!out2.is_empty());
    }

    #[test]
    fn nllb200_smoke() {
        let _guard = env_lock();
        if !crate::onnx_translate_install::installed(Variant::Nllb200) {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let out = translate("Compiling focus-translator v0.1.0", true, Variant::Nllb200).unwrap();
        println!("nllb en->ja: {out}");
        assert!(!out.is_empty());
        let out2 = translate("こんにちは、元気ですか？", false, Variant::Nllb200).unwrap();
        println!("nllb ja->en: {out2}");
        assert!(!out2.is_empty());
    }
}
