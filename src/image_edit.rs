// キャプチャ画像のインライン編集 (SPECv0.4 §1-§4)
// 矩形トリミングと投げ輪(ラッソ)トリミングの画像処理を担う。
// UI (プレビュー・ドラッグ操作) は overlay / overlay_layout 側が担当し、
// 「適用」確定時に本モジュールで切り出し・マスク処理を行う。
use crate::capture::Captured;

/// 編集で確定した選択領域 (元画像のピクセル座標)
pub enum Selection {
    /// 矩形: 始点・終点 (順不同)
    Rect { x0: i32, y0: i32, x1: i32, y1: i32 },
    /// 投げ輪: ドラッグ軌跡の座標リスト (閉じた多角形として扱う)
    Lasso(Vec<(i32, i32)>),
}

/// 選択領域を適用して編集後の画像を返す。無効な選択 (小さすぎる等) は None。
pub fn apply(img: &Captured, sel: &Selection) -> Option<Captured> {
    match sel {
        Selection::Rect { x0, y0, x1, y1 } => crop_rect(img, *x0, *y0, *x1, *y1),
        Selection::Lasso(pts) => lasso_crop(img, pts),
    }
}

/// apply() でクロップした際に実際に使われる矩形 (img 内、クランプ後、l<r かつ t<b) を返す。
/// apply() が None を返す入力では None を返す (無効判定はここでも同じ基準を使う)。
/// 全体画像内での img の位置を合成補正するために使う (SPECv0.5.2追補)。
pub fn selection_bounds_clamped(img: &Captured, sel: &Selection) -> Option<(i32, i32, i32, i32)> {
    let w = img.width as i32;
    let h = img.height as i32;
    match sel {
        Selection::Rect { x0, y0, x1, y1 } => {
            let (x0, y0, x1, y1) = (*x0, *y0, *x1, *y1);
            let (l, r) = (x0.min(x1), x0.max(x1));
            let (t, b) = (y0.min(y1), y0.max(y1));
            if r - l < MIN_SEL || b - t < MIN_SEL {
                return None;
            }
            let (l, t) = (l.max(0), t.max(0));
            let (r, b) = (r.min(w), b.min(h));
            if r - l < MIN_SEL || b - t < MIN_SEL {
                return None;
            }
            Some((l, t, r, b))
        }
        Selection::Lasso(pts) => {
            if pts.len() < 3 {
                return None;
            }
            let l = pts.iter().map(|p| p.0).min()?.clamp(0, w - 1);
            let r = pts.iter().map(|p| p.0).max()?.clamp(0, w);
            let t = pts.iter().map(|p| p.1).min()?.clamp(0, h - 1);
            let b = pts.iter().map(|p| p.1).max()?.clamp(0, h);
            if r - l < MIN_SEL || b - t < MIN_SEL {
                return None;
            }
            Some((l, t, r, b))
        }
    }
}

/// 最小の有効選択サイズ (これ未満はドラッグミスとみなす)
const MIN_SEL: i32 = 4;

/// 矩形トリミング: 始点終点を正規化してクロップする (SPECv0.4 §3.1, §4-2)
pub fn crop_rect(img: &Captured, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<Captured> {
    let (l, r) = (x0.min(x1), x0.max(x1));
    let (t, b) = (y0.min(y1), y0.max(y1));
    if r - l < MIN_SEL || b - t < MIN_SEL {
        return None;
    }
    crate::capture::crop(img, l, t, r - l, b - t)
}

/// 投げ輪トリミング (SPECv0.4 §4-1, §4-2):
/// 軌跡の多角形領域外を「塗りつぶし対象ピクセルの平均色」で塗りつぶし、
/// 領域を囲む最小矩形でクロップする。平均色を算出できない場合は白にフォールバック。
pub fn lasso_crop(img: &Captured, pts: &[(i32, i32)]) -> Option<Captured> {
    if pts.len() < 3 {
        return None;
    }
    // バウンディングボックス (画像内にクランプ)
    let l = pts.iter().map(|p| p.0).min()?.clamp(0, img.width as i32 - 1);
    let r = pts.iter().map(|p| p.0).max()?.clamp(0, img.width as i32);
    let t = pts.iter().map(|p| p.1).min()?.clamp(0, img.height as i32 - 1);
    let b = pts.iter().map(|p| p.1).max()?.clamp(0, img.height as i32);
    if r - l < MIN_SEL || b - t < MIN_SEL {
        return None;
    }
    let mut out = crate::capture::crop(img, l, t, r - l, b - t)?;
    let (w, h) = (out.width as i32, out.height as i32);

    // 走査線ごとに多角形の内側スパンを求め、外側ピクセルのマスクを作る
    let mut outside = vec![true; (w * h) as usize];
    for y in 0..h {
        let spans = scanline_spans(pts, y + t, l);
        for (sx, ex) in spans {
            let sx = sx.clamp(0, w);
            let ex = ex.clamp(0, w);
            for x in sx..ex {
                outside[(y * w + x) as usize] = false;
            }
        }
    }

    // 領域外(=塗りつぶし対象)ピクセルの平均色を算出。対象が無ければ白。
    let (mut sb, mut sg, mut sr, mut n) = (0u64, 0u64, 0u64, 0u64);
    for (i, is_out) in outside.iter().enumerate() {
        if *is_out {
            let p = i * 4;
            sb += out.bgra[p] as u64;
            sg += out.bgra[p + 1] as u64;
            sr += out.bgra[p + 2] as u64;
            n += 1;
        }
    }
    let fill = if n > 0 {
        [(sb / n) as u8, (sg / n) as u8, (sr / n) as u8, 255]
    } else {
        [255, 255, 255, 255]
    };

    for (i, is_out) in outside.iter().enumerate() {
        if *is_out {
            let p = i * 4;
            out.bgra[p..p + 4].copy_from_slice(&fill);
        }
    }
    Some(out)
}

/// 選択範囲を「消す」(トリミングはせず、選択範囲の内側だけを周辺色で塗りつぶして
/// 元と同じサイズの画像を返す) (SPECv0.4追補)。
/// 塗りつぶし色は選択範囲のバウンディングボックスを外側へ RING px 拡張したリング領域
/// (=選択範囲に隣接するピクセル)の平均色とする。リングが空(=画像全体が選択範囲)の
/// 場合は白にフォールバックする。
pub fn erase_selection(img: &Captured, sel: &Selection) -> Option<Captured> {
    let w = img.width as i32;
    let h = img.height as i32;
    let mut inside = vec![false; (w * h) as usize];
    let (l, t, r, b) = match sel {
        Selection::Rect { x0, y0, x1, y1 } => {
            let l = (*x0).min(*x1).clamp(0, w);
            let r = (*x0).max(*x1).clamp(0, w);
            let t = (*y0).min(*y1).clamp(0, h);
            let b = (*y0).max(*y1).clamp(0, h);
            if r - l < MIN_SEL || b - t < MIN_SEL {
                return None;
            }
            for y in t..b {
                for x in l..r {
                    inside[(y * w + x) as usize] = true;
                }
            }
            (l, t, r, b)
        }
        Selection::Lasso(pts) => {
            if pts.len() < 3 {
                return None;
            }
            let l = pts.iter().map(|p| p.0).min()?.clamp(0, w);
            let r = pts.iter().map(|p| p.0).max()?.clamp(0, w);
            let t = pts.iter().map(|p| p.1).min()?.clamp(0, h);
            let b = pts.iter().map(|p| p.1).max()?.clamp(0, h);
            if r - l < MIN_SEL || b - t < MIN_SEL {
                return None;
            }
            for y in t..b {
                let spans = scanline_spans(pts, y, 0); // フル画像座標のまま使う (offset無し)
                for (sx, ex) in spans {
                    let sx = sx.clamp(0, w);
                    let ex = ex.clamp(0, w);
                    for x in sx..ex {
                        inside[(y * w + x) as usize] = true;
                    }
                }
            }
            (l, t, r, b)
        }
    };

    // 隣接色の平均: バウンディングボックスを外側へ RING px 拡張したリング領域
    // (=選択範囲の外側かつ拡張範囲内)の平均色。無ければ白にフォールバック。
    const RING: i32 = 12;
    let el = (l - RING).max(0);
    let et = (t - RING).max(0);
    let er = (r + RING).min(w);
    let eb = (b + RING).min(h);
    let (mut sb, mut sg, mut sr, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in et..eb {
        for x in el..er {
            let idx = (y * w + x) as usize;
            if !inside[idx] {
                let p = idx * 4;
                sb += img.bgra[p] as u64;
                sg += img.bgra[p + 1] as u64;
                sr += img.bgra[p + 2] as u64;
                n += 1;
            }
        }
    }
    let fill = if n > 0 {
        [(sb / n) as u8, (sg / n) as u8, (sr / n) as u8, 255]
    } else {
        [255, 255, 255, 255]
    };

    let mut out = img.clone();
    for (i, is_in) in inside.iter().enumerate() {
        if *is_in {
            let p = i * 4;
            out.bgra[p..p + 4].copy_from_slice(&fill);
        }
    }
    Some(out)
}

/// 多角形と水平線 y の交差から内側スパン [(開始x, 終了x)] を求める (偶奇規則)。
/// x はクロップ後座標に合わせるため offset_x を引いて返す。
fn scanline_spans(pts: &[(i32, i32)], y: i32, offset_x: i32) -> Vec<(i32, i32)> {
    let mut xs: Vec<f32> = Vec::new();
    let n = pts.len();
    let yc = y as f32 + 0.5; // ピクセル中心で判定
    for i in 0..n {
        let (x1, y1) = (pts[i].0 as f32, pts[i].1 as f32);
        let (x2, y2) = (pts[(i + 1) % n].0 as f32, pts[(i + 1) % n].1 as f32);
        if (y1 <= yc && yc < y2) || (y2 <= yc && yc < y1) {
            let x = x1 + (yc - y1) / (y2 - y1) * (x2 - x1);
            xs.push(x);
        }
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.chunks(2)
        .filter_map(|c| {
            if c.len() == 2 {
                Some((c[0].round() as i32 - offset_x, c[1].round() as i32 - offset_x))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 単色 BGRA 画像を作る
    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Captured {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&bgra);
        }
        Captured { width: w, height: h, bgra: buf }
    }

    fn px(img: &Captured, x: u32, y: u32) -> [u8; 4] {
        let p = ((y * img.width + x) * 4) as usize;
        [img.bgra[p], img.bgra[p + 1], img.bgra[p + 2], img.bgra[p + 3]]
    }

    #[test]
    fn 矩形トリミングは正規化してクロップする() {
        let img = solid(100, 80, [10, 20, 30, 255]);
        // 終点→始点の逆順ドラッグでも同じ結果
        let a = crop_rect(&img, 10, 10, 50, 40).unwrap();
        let b = crop_rect(&img, 50, 40, 10, 10).unwrap();
        assert_eq!((a.width, a.height), (40, 30));
        assert_eq!((b.width, b.height), (40, 30));
    }

    #[test]
    fn 小さすぎる矩形は無効() {
        let img = solid(100, 80, [0, 0, 0, 255]);
        assert!(crop_rect(&img, 10, 10, 12, 12).is_none());
    }

    #[test]
    fn 投げ輪は領域外を平均色で塗りバウンディングボックスでクロップする() {
        // 黒画像に対しひし形の投げ輪 → 四隅 (領域外) は平均色=黒のまま
        let img = solid(60, 60, [0, 0, 0, 255]);
        let diamond = vec![(30, 10), (50, 30), (30, 50), (10, 30)];
        let out = lasso_crop(&img, &diamond).unwrap();
        assert_eq!((out.width, out.height), (40, 40));
        // 中心は内側 (元の黒)
        assert_eq!(px(&out, 20, 20), [0, 0, 0, 255]);
        // 左上隅は外側 → 平均色(=黒)
        assert_eq!(px(&out, 0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn 投げ輪の外側は周辺平均色になる() {
        // 左半分が黒・右半分が白の画像。投げ輪を左側に置くと外側の平均はグレー寄り。
        let mut img = solid(40, 40, [0, 0, 0, 255]);
        for y in 0..40u32 {
            for x in 20..40u32 {
                let p = ((y * 40 + x) * 4) as usize;
                img.bgra[p..p + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let tri = vec![(5, 5), (35, 5), (5, 35)];
        let out = lasso_crop(&img, &tri).unwrap();
        // 右下隅 (三角形の外側) は塗りつぶされている: 外側平均は黒と白の混合
        let c = px(&out, out.width - 1, out.height - 1);
        assert!(c[0] > 0 && c[0] < 255, "外側は平均色で塗られる (got {c:?})");
        // 内側 (左上寄り) は元の黒のまま
        assert_eq!(px(&out, 2, 2), [0, 0, 0, 255]);
    }

    #[test]
    fn 点が少なすぎる投げ輪は無効() {
        let img = solid(40, 40, [0, 0, 0, 255]);
        assert!(lasso_crop(&img, &[(1, 1), (2, 2)]).is_none());
    }

    #[test]
    fn 矩形の消去は同サイズのまま内側だけ隣接色で塗る() {
        let img = solid(60, 60, [10, 20, 30, 255]);
        let sel = Selection::Rect { x0: 20, y0: 20, x1: 40, y1: 40 };
        let out = erase_selection(&img, &sel).unwrap();
        // サイズは変わらない (トリミングしない)
        assert_eq!((out.width, out.height), (60, 60));
        // 選択範囲内は周辺色(単色画像なので同じ色)で塗られる
        assert_eq!(px(&out, 30, 30), [10, 20, 30, 255]);
        // 選択範囲外は元のまま
        assert_eq!(px(&out, 0, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn 消去は選択範囲に隣接する色を使う_遠い背景の影響を受けない() {
        // 画像全体は白。選択範囲(左上)のすぐ外だけを黒で囲む。
        let mut img = solid(80, 80, [255, 255, 255, 255]);
        // 選択範囲 (20,20)-(40,40) の周囲(リング内)を黒で塗る
        for y in 5..55u32 {
            for x in 5..55u32 {
                let is_ring = !(20..40).contains(&x) || !(20..40).contains(&y);
                if is_ring {
                    let p = ((y * 80 + x) * 4) as usize;
                    img.bgra[p..p + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        let sel = Selection::Rect { x0: 20, y0: 20, x1: 40, y1: 40 };
        let out = erase_selection(&img, &sel).unwrap();
        // 隣接色(黒)で塗られるので、選択範囲内は黒に近い値になる(遠くの白背景の影響を受けない)
        let c = px(&out, 30, 30);
        assert!(c[0] < 40, "隣接する黒に近い色で塗られるはず (got {c:?})");
    }

    #[test]
    fn 小さすぎる選択の消去は無効() {
        let img = solid(60, 60, [0, 0, 0, 255]);
        let sel = Selection::Rect { x0: 10, y0: 10, x1: 12, y1: 12 };
        assert!(erase_selection(&img, &sel).is_none());
    }
}
