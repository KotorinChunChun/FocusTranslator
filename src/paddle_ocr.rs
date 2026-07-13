// PaddleOCR (RapidOCR配布 PP-OCRv4 mobile ONNX) のローカル推論本体 (SPEC §7.1)
// 検出: DBNet の確率マップを閾値二値化し、連結成分の外接矩形をテキスト領域として扱う
//       (unclipは面積/周長比による矩形膨張で近似)。回転矩形には非対応(横書きのみ想定)。
// 認識: CTCヘッドの greedy decode (連続重複の除去 + blank除去)。
use crate::capture::Captured;
use ort::session::{Session, SessionInputValue};
use ort::value::TensorRef;
use std::path::Path;
use std::sync::{Arc, Mutex};

const DET_LIMIT_SIDE: u32 = 960;
const DET_THRESH: f32 = 0.3;
const DET_BOX_THRESH: f32 = 0.5;
const DET_UNCLIP_RATIO: f32 = 1.6;
const DET_MIN_SIZE: u32 = 4;
const DET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const DET_STD: [f32; 3] = [0.229, 0.224, 0.225];

const REC_IMG_H: u32 = 48;
/// 非常に横長な行を検出した場合の安全上限(通常のUI文字列であれば十分な余裕がある)
const REC_IMG_W_MAX: u32 = 1600;

struct Engine {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    /// 認識クラスindex(1始まり、0はCTCのblank)から文字への対応表。dict.txtの各行 + 末尾に空白1文字。
    dict: Vec<String>,
}

static ENGINE: Mutex<Option<Arc<Engine>>> = Mutex::new(None);

fn load_dict(path: &Path) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("文字辞書の読込に失敗しました: {e}"))?;
    let mut dict: Vec<String> =
        content.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
    dict.push(" ".to_string());
    Ok(dict)
}

fn load_engine() -> Result<Engine, String> {
    let dir = crate::paddle_install::dir();
    let det = Session::builder()
        .and_then(|mut b| b.commit_from_file(dir.join("det.onnx")))
        .map_err(|e| format!("PaddleOCR検出モデルの読込に失敗しました: {e}"))?;
    let rec = Session::builder()
        .and_then(|mut b| b.commit_from_file(dir.join("rec.onnx")))
        .map_err(|e| format!("PaddleOCR認識モデルの読込に失敗しました: {e}"))?;
    let dict = load_dict(&dir.join("dict.txt"))?;
    Ok(Engine { det: Mutex::new(det), rec: Mutex::new(rec), dict })
}

fn engine() -> Result<Arc<Engine>, String> {
    let mut guard = ENGINE.lock().map_err(|_| "PaddleOCRエンジンのロックに失敗しました".to_string())?;
    if let Some(e) = guard.as_ref() {
        return Ok(e.clone());
    }
    let eng = Arc::new(load_engine()?);
    *guard = Some(eng.clone());
    Ok(eng)
}

/// (x, y) の BGR 値をバイリニア補間で取得する(0..255 範囲)。imgはBGRA。
fn sample_bgr(img: &Captured, x: f32, y: f32) -> [f32; 3] {
    let w = img.width as i64;
    let h = img.height as i64;
    let xc = x.clamp(0.0, (w - 1).max(0) as f32);
    let yc = y.clamp(0.0, (h - 1).max(0) as f32);
    let x0 = xc.floor() as i64;
    let y0 = yc.floor() as i64;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = xc - x0 as f32;
    let fy = yc - y0 as f32;
    let px = |xx: i64, yy: i64, c: usize| -> f32 {
        let idx = ((yy as u32 * img.width + xx as u32) * 4 + c as u32) as usize;
        img.bgra[idx] as f32
    };
    let mut out = [0f32; 3];
    for (c, out_val) in out.iter_mut().enumerate() {
        let v00 = px(x0, y0, c);
        let v10 = px(x1, y0, c);
        let v01 = px(x0, y1, c);
        let v11 = px(x1, y1, c);
        let top = v00 * (1.0 - fx) + v10 * fx;
        let bot = v01 * (1.0 - fx) + v11 * fx;
        *out_val = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// 画像全体を (w,h) にリサイズし、CHW配列(mean/std正規化済み)を作る
fn resize_normalize_full(img: &Captured, w: u32, h: u32, mean: &[f32; 3], std: &[f32; 3]) -> Vec<f32> {
    let mut out = vec![0f32; 3 * (w * h) as usize];
    let sx = img.width as f32 / w as f32;
    let sy = img.height as f32 / h as f32;
    let plane = (w * h) as usize;
    for yy in 0..h {
        for xx in 0..w {
            let src_x = (xx as f32 + 0.5) * sx - 0.5;
            let src_y = (yy as f32 + 0.5) * sy - 0.5;
            let bgr = sample_bgr(img, src_x, src_y);
            let idx = (yy * w + xx) as usize;
            for c in 0..3 {
                out[c * plane + idx] = (bgr[c] / 255.0 - mean[c]) / std[c];
            }
        }
    }
    out
}

/// 検出モデルの入力サイズ: 長辺を960以下に収め、各辺を32の倍数に丸める
fn det_target_size(w: u32, h: u32) -> (u32, u32) {
    let max_side = w.max(h) as f32;
    let ratio = if max_side > DET_LIMIT_SIDE as f32 { DET_LIMIT_SIDE as f32 / max_side } else { 1.0 };
    let rh = (((h as f32 * ratio / 32.0).round() as u32) * 32).max(32);
    let rw = (((w as f32 * ratio / 32.0).round() as u32) * 32).max(32);
    (rw, rh)
}

#[derive(Clone, Copy)]
struct TextBox {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// DBNet確率マップから、閾値二値化+連結成分の外接矩形でテキスト領域を検出する
fn run_det(eng: &Engine, img: &Captured) -> Result<Vec<TextBox>, String> {
    let (rw, rh) = det_target_size(img.width, img.height);
    let chw = resize_normalize_full(img, rw, rh, &DET_MEAN, &DET_STD);
    let ratio_w = rw as f32 / img.width as f32;
    let ratio_h = rh as f32 / img.height as f32;

    let mut det = eng.det.lock().map_err(|_| "検出モデルのロックに失敗しました".to_string())?;
    let input_name = det.inputs()[0].name().to_string();
    let tensor = TensorRef::from_array_view((vec![1i64, 3, rh as i64, rw as i64], chw.as_slice()))
        .map_err(|e| format!("検出入力テンソルの作成に失敗しました: {e}"))?;
    let outputs = det
        .run(vec![(std::borrow::Cow::Borrowed(input_name.as_str()), SessionInputValue::from(tensor))])
        .map_err(|e| format!("検出モデルの実行に失敗しました: {e}"))?;
    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("検出結果の取得に失敗しました: {e}"))?;
    let h = shape[2] as usize;
    let w = shape[3] as usize;

    let mut visited = vec![false; h * w];
    let mut boxes = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            let idx = sy * w + sx;
            if visited[idx] || data[idx] < DET_THRESH {
                continue;
            }
            visited[idx] = true;
            let mut stack = vec![(sx, sy)];
            let (mut minx, mut maxx, mut miny, mut maxy) = (sx, sx, sy, sy);
            let mut sum = 0f32;
            let mut count = 0usize;
            while let Some((cx, cy)) = stack.pop() {
                let ci = cy * w + cx;
                sum += data[ci];
                count += 1;
                minx = minx.min(cx);
                maxx = maxx.max(cx);
                miny = miny.min(cy);
                maxy = maxy.max(cy);
                let neighbors = [
                    (cx.wrapping_sub(1), cy),
                    (cx + 1, cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy + 1),
                ];
                for (nx, ny) in neighbors {
                    if nx >= w || ny >= h {
                        continue;
                    }
                    let ni = ny * w + nx;
                    if !visited[ni] && data[ni] >= DET_THRESH {
                        visited[ni] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let bw = (maxx - minx + 1) as u32;
            let bh = (maxy - miny + 1) as u32;
            if bw < DET_MIN_SIZE || bh < DET_MIN_SIZE {
                continue;
            }
            let score = sum / count as f32;
            if score < DET_BOX_THRESH {
                continue;
            }
            // unclip: DBNetの標準的な膨張量(面積*率/周長)を軸並行矩形に近似適用する
            let area = (bw * bh) as f32;
            let perimeter = 2.0 * (bw + bh) as f32;
            let expand = area * DET_UNCLIP_RATIO / perimeter;
            let x0 = (minx as f32 - expand).max(0.0);
            let y0 = (miny as f32 - expand).max(0.0);
            let x1 = ((maxx + 1) as f32 + expand).min(w as f32);
            let y1 = ((maxy + 1) as f32 + expand).min(h as f32);
            boxes.push(TextBox {
                x0: x0 / ratio_w,
                y0: y0 / ratio_h,
                x1: x1 / ratio_w,
                y1: y1 / ratio_h,
            });
        }
    }
    Ok(boxes)
}

/// 行単位の読み順(上→下、同じ行内は左→右)に並べ替える
fn sort_boxes(mut boxes: Vec<TextBox>) -> Vec<TextBox> {
    boxes.sort_by(|a, b| {
        if (a.y0 - b.y0).abs() < 10.0 {
            a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.y0.partial_cmp(&b.y0).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    boxes
}

/// 検出矩形を認識モデル入力サイズ(高さ48固定、幅はアスペクト比に応じて可変)に切り出す
fn crop_resize_rec(img: &Captured, b: &TextBox) -> (Vec<f32>, u32) {
    let box_w = (b.x1 - b.x0).max(1.0);
    let box_h = (b.y1 - b.y0).max(1.0);
    let ratio = box_w / box_h;
    let target_w = ((REC_IMG_H as f32 * ratio).ceil() as u32).clamp(1, REC_IMG_W_MAX);
    let plane = (target_w * REC_IMG_H) as usize;
    let mut out = vec![0f32; 3 * plane];
    for yy in 0..REC_IMG_H {
        for xx in 0..target_w {
            let src_x = b.x0 + (xx as f32 + 0.5) * box_w / target_w as f32;
            let src_y = b.y0 + (yy as f32 + 0.5) * box_h / REC_IMG_H as f32;
            let bgr = sample_bgr(img, src_x, src_y);
            let idx = (yy * target_w + xx) as usize;
            for c in 0..3 {
                // PaddleOCR rec の正規化は ImageNet統計を使わず (px/255 - 0.5) / 0.5 の単純スケーリング
                out[c * plane + idx] = (bgr[c] / 255.0 - 0.5) / 0.5;
            }
        }
    }
    (out, target_w)
}

/// CTC greedy decode: 連続重複を1つに畳み込み、blank(index 0)を除去する
fn run_rec(eng: &Engine, img: &Captured, b: &TextBox) -> Result<String, String> {
    let (chw, w) = crop_resize_rec(img, b);
    let mut rec = eng.rec.lock().map_err(|_| "認識モデルのロックに失敗しました".to_string())?;
    let input_name = rec.inputs()[0].name().to_string();
    let tensor = TensorRef::from_array_view((vec![1i64, 3, REC_IMG_H as i64, w as i64], chw.as_slice()))
        .map_err(|e| format!("認識入力テンソルの作成に失敗しました: {e}"))?;
    let outputs = rec
        .run(vec![(std::borrow::Cow::Borrowed(input_name.as_str()), SessionInputValue::from(tensor))])
        .map_err(|e| format!("認識モデルの実行に失敗しました: {e}"))?;
    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("認識結果の取得に失敗しました: {e}"))?;
    let seq_len = shape[1] as usize;
    let vocab = shape[2] as usize;

    let mut text = String::new();
    let mut last_idx: i64 = -1;
    for t in 0..seq_len {
        let row = &data[t * vocab..(t + 1) * vocab];
        let mut best_i = 0usize;
        let mut best_v = f32::MIN;
        for (i, &v) in row.iter().enumerate() {
            if v > best_v {
                best_v = v;
                best_i = i;
            }
        }
        if best_i != 0 && best_i as i64 != last_idx && let Some(ch) = eng.dict.get(best_i - 1) {
            text.push_str(ch);
        }
        last_idx = best_i as i64;
    }
    Ok(text)
}

/// PaddleOCR (det+rec) による推論本体。focus_yが指定されればカーソルに最も近い1行のみを返す。
pub fn ocr_paddle(img: &Captured, focus_y: Option<f32>) -> Result<String, String> {
    let eng = engine()?;
    let boxes = run_det(&eng, img)?;
    if boxes.is_empty() {
        return Err("テキストを検出できませんでした".into());
    }
    let boxes = sort_boxes(boxes);

    let mut items: Vec<(f32, String)> = Vec::new();
    for b in &boxes {
        let text = run_rec(&eng, img, b)?;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        items.push(((b.y0 + b.y1) / 2.0, text.to_string()));
    }
    if items.is_empty() {
        return Err("テキストを検出できませんでした".into());
    }

    match focus_y {
        Some(fy) => {
            let mut best = &items[0];
            for it in &items {
                if (it.0 - fy).abs() < (best.0 - fy).abs() {
                    best = it;
                }
            }
            Ok(best.1.clone())
        }
        None => {
            let texts: Vec<String> = items.iter().map(|i| i.1.clone()).collect();
            Ok(crate::ocr::join_paragraph(&texts))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::render_text;

    #[test]
    fn smoke_english() {
        if !crate::paddle_install::installed() {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let img = render_text("Hello World", 400, 80);
        let out = ocr_paddle(&img, None).expect("推論に失敗しました");
        println!("paddle ocr (en): {out}");
        assert!(!out.is_empty());
    }

    #[test]
    fn smoke_japanese() {
        if !crate::paddle_install::installed() {
            eprintln!("モデル未導入のためスキップ");
            return;
        }
        let img = render_text("こんにちは世界", 400, 80);
        let out = ocr_paddle(&img, None).expect("推論に失敗しました");
        println!("paddle ocr (ja): {out}");
        assert!(!out.is_empty());
    }
}
