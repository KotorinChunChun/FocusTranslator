// オーバーレイのレイアウト計算 (SPEC v0.3 §3)
// OverlayContent からボタン・テキストの配置を計算し、Layout を返す。
// overlay.rs の描画 (paint) とウィンドウ管理から分離して可読性を高める。
use crate::engine;
use crate::overlay::{
    OverlayContent, CHIP_CLOSE, CHIP_COPY, CHIP_COPY_INFO, CHIP_COPY_SRC, CHIP_COPY_TR,
    CHIP_EXPLAIN, CHIP_EXPLAIN_QUICK, CHIP_IMAGE, CHIP_OCR_BASE, CHIP_OPEN_LOG, CHIP_PIN,
    CHIP_SETTINGS, CHIP_SWAP_LANG, CHIP_TR_BASE, CHIP_UIA_NODE_BASE,
};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK,
    DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, DrawTextW, FONT_OUTPUT_PRECISION, FW_BOLD,
    FW_NORMAL, GetDC, HDC, HFONT, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::core::w;

// 配色定数
pub const COL_BG: u32 = 0x00221E1C;
pub const COL_BORDER: u32 = 0x00524A46;
pub const COL_TEXT: u32 = 0x00F0EEEC;
pub const COL_STATUS: u32 = 0x0050C8FF;
pub const COL_CHIP: u32 = 0x003F3833;
pub const COL_CHIP_ACTIVE: u32 = 0x00D28C3C;
pub const COL_CHIP_TEXT: u32 = 0x00E8E4E0;
pub const COL_CHIP_DISABLED: u32 = 0x00787068;
pub const COL_LABEL: u32 = 0x00908A84;
pub const COL_PANEL_BG: u32 = 0x002B2723;
pub const COL_PANEL_BORDER: u32 = 0x00423C37;
pub const COL_ACCENT_INFO: u32 = 0x0050B4FF;
pub const COL_ACCENT_OCR: u32 = 0x007864FF;
pub const COL_ACCENT_TR: u32 = 0x00C850DC;
pub const COL_ACCENT_EXPLAIN: u32 = 0x00FF5082;
pub const COL_UIA_BADGE: u32 = 0x0080D0A0;
pub const COL_CLOSE_HOVER: u32 = 0x003C3CD6;

pub const PAD: i32 = 12;
pub const PANEL_MARGIN: i32 = 6;
pub const PANEL_RADIUS: i32 = 8;
pub const ACCENT_W: i32 = 4;
pub const MAXW: i32 = 620;

// フォントサイズ (統一スケール)
pub const FONT_INFO: i32 = 11;
pub const FONT_CHIP: i32 = 12;
pub const FONT_HEADING: i32 = 13;
pub const FONT_BODY: i32 = 17;

pub const CHIP_H: i32 = 24;
pub const CLOSE_SIZE: i32 = 20;

pub enum Item {
    Text { rect: RECT, text: String, size: i32, color: u32, bold: bool },
    Chip { rect: RECT, label: String, id: usize, active: bool, enabled: bool },
}

/// ブロック(カード)の背景。見出し・本文より下のレイヤーに描画される。
pub struct Panel {
    pub rect: RECT,
    pub accent: u32,
}

pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub content_h: i32,
    pub items: Vec<Item>,
    pub panels: Vec<Panel>,
}

pub fn make_font(size: i32, bold: bool) -> HFONT {
    unsafe {
        CreateFontW(
            -size,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0.into(),
            w!("Yu Gothic UI"),
        )
    }
}

pub fn measure(hdc: HDC, text: &str, size: i32, bold: bool, maxw: i32) -> (i32, i32) {
    unsafe {
        let font = make_font(size, bold);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            wide.push(' ' as u16);
        }
        let mut r = RECT { left: 0, top: 0, right: maxw, bottom: 0 };
        DrawTextW(hdc, &mut wide, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        (r.right - r.left, r.bottom - r.top)
    }
}

pub fn compute_layout(hwnd: HWND, content: &OverlayContent) -> Layout {
    unsafe {
        let hdc = GetDC(Some(hwnd));
        let mut items: Vec<Item> = Vec::new();
        let mut panel_spans: Vec<(i32, i32, u32)> = Vec::new();
        let mut y = PAD;
        let mut need_w = 240i32;
        // 翻訳結果ブロックの言語反転ボタン (ラベル, 幅, 見出しY)。最終幅の確定後に右端へ置く。
        let mut swap_btn: Option<(String, i32, i32)> = None;
        // 【アプリケーション】行の右端ボタン (キャプチャ画像幅(0なら無し), 上端Y)。
        let mut app_row_cap_btn: Option<(i32, i32)> = None;
        // システムメッセージ行の右端ボタン (設定ボタン幅, ログを開く幅, 上端Y)。
        let sys_msg_btns: Option<(i32, i32, i32)>;

        if content.error_only {
            let msg = content.status.clone().unwrap_or_default();
            let (tw, th) = measure(hdc, &msg, FONT_HEADING, false, MAXW);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + tw + 4, bottom: y + th },
                text: msg,
                size: FONT_HEADING,
                color: COL_STATUS,
                bold: false,
            });
            y += th + PAD;
            need_w = need_w.max(tw + PAD * 2 + 4);
            let _ = ReleaseDC(Some(hwnd), hdc);
            return Layout { w: need_w.min(MAXW + PAD * 2), h: y, content_h: y, items, panels: Vec::new() };
        }

        // 見出し行を配置する。chips_left=true のときはチップを見出し文字の手前(左端)に、
        // false のときは従来どおり見出し文字の右側に並べる。戻り値は行の高さ。
        let heading_row = |items: &mut Vec<Item>,
                           y: i32,
                           text: &str,
                           color: u32,
                           chips: &[(&str, usize, bool)],
                           chips_left: bool,
                           need_w: &mut i32|
         -> i32 {
            if chips_left && !chips.is_empty() {
                let mut x = PAD;
                for (lab, id, enabled) in chips {
                    let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                    let w = cw + 16;
                    items.push(Item::Chip {
                        rect: RECT { left: x, top: y - 1, right: x + w, bottom: y - 1 + CLOSE_SIZE },
                        label: lab.to_string(),
                        id: *id,
                        active: false,
                        enabled: *enabled,
                    });
                    x += w + 8;
                }
                let (hw, hh) = measure(hdc, text, FONT_HEADING, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: x, top: y, right: x + hw + 4, bottom: y + hh },
                    text: text.to_string(),
                    size: FONT_HEADING,
                    color,
                    bold: false,
                });
                *need_w = (*need_w).max(x + hw + PAD);
                hh.max(CLOSE_SIZE)
            } else {
                let (hw, hh) = measure(hdc, text, FONT_HEADING, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + hw + 4, bottom: y + hh },
                    text: text.to_string(),
                    size: FONT_HEADING,
                    color,
                    bold: false,
                });
                let mut x = PAD + hw + 10;
                let row_h = hh.max(CLOSE_SIZE);
                for (lab, id, enabled) in chips {
                    let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                    let w = cw + 16;
                    items.push(Item::Chip {
                        rect: RECT { left: x, top: y - 1, right: x + w, bottom: y - 1 + CLOSE_SIZE },
                        label: lab.to_string(),
                        id: *id,
                        active: false,
                        enabled: *enabled,
                    });
                    x += w + 6;
                }
                *need_w = (*need_w).max(x + PAD - 6);
                row_h
            }
        };

        let chip_row = |items: &mut Vec<Item>,
                        y: &mut i32,
                        keys: &[&str],
                        labels: &[&str],
                        cur: &str,
                        enabled: &[bool],
                        base: usize,
                        need_w: &mut i32| {
            let mut x = PAD;
            for (i, lab) in labels.iter().enumerate() {
                let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                let w = cw + 18;
                items.push(Item::Chip {
                    rect: RECT { left: x, top: *y, right: x + w, bottom: *y + CHIP_H },
                    label: lab.to_string(),
                    id: base + i,
                    active: keys[i] == cur,
                    enabled: enabled[i],
                });
                x += w + 6;
            }
            *need_w = (*need_w).max(x + PAD - 6);
            *y += CHIP_H + 6;
        };

        // システムメッセージ行 (テキスト ＋ 右端に[設定][ログを開く])
        // 進行中メッセージ(末尾が「…」)はドットをアニメーションし、エラー等はそのまま出す。
        let mut sys_disp = String::new();
        if let Some(s) = &content.status {
            let is_progress = s.ends_with('…');
            sys_disp = s.clone();
            if is_progress {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let count = (millis / 300) % 4;
                sys_disp = sys_disp.replace("…", "");
                sys_disp.push_str(&".".repeat(count as usize));
            }
        }

        if let Some(b) = &content.badge {
            if !sys_disp.is_empty() {
                sys_disp.push_str("  ");
            }
            sys_disp.push_str(&format!("[{}]", b));
        }

        // ボタンの幅計算
        let (set_cw, _) = measure(hdc, "設定", FONT_CHIP, false, 200);
        let set_w = set_cw + 20;
        let (log_cw, _) = measure(hdc, "ログを開く", FONT_CHIP, false, 200);
        let log_w = log_cw + 20;
        let sys_btns_w = set_w + 6 + log_w;

        // テキストの描画
        let row_h = CHIP_H;
        let mut sys_text_h = 0;
        let mut sys_text_w = 0;
        if !sys_disp.is_empty() {
            let (tw, th) = measure(hdc, &sys_disp, FONT_HEADING, false, MAXW - sys_btns_w - 20);
            sys_text_w = tw;
            sys_text_h = th;
            let text_y = y + (row_h.max(th) - th) / 2;
            items.push(Item::Text {
                rect: RECT { left: PAD, top: text_y, right: PAD + tw + 4, bottom: text_y + th },
                text: sys_disp,
                size: FONT_HEADING,
                color: COL_STATUS,
                bold: false,
            });
        }
        let sys_row_h = row_h.max(sys_text_h);
        sys_msg_btns = Some((set_w, log_w, y + (sys_row_h - CHIP_H) / 2));
        need_w = need_w.max(PAD + sys_text_w + 10 + sys_btns_w + PAD);
        y += sys_row_h + 8;

        // 【入力内容】: 対象アプリ情報 + UIAパスノードボタン + OCR対象画像ボタン。コピーは見出しラベルの左端。
        if !content.app_title.is_empty() || content.has_image || !content.uia_nodes.is_empty() {
            let block_start = y;
            let hh = heading_row(
                &mut items,
                y,
                "【入力内容】",
                COL_LABEL,
                &[("📋", CHIP_COPY_INFO, true)],
                true,
                &mut need_w,
            );
            y += hh + 4;

            // 【アプリケーション】行
            if !content.app_title.is_empty() {
                let cap_w = if content.has_image {
                    let (cw, _) = measure(hdc, "キャプチャ画像", FONT_CHIP, false, 200);
                    cw + 20
                } else {
                    0
                };
                let btns_w = if content.has_image { cap_w + 6 } else { 0 };
                let reserve = btns_w + 12 + CLOSE_SIZE * 2 + 14;
                let avail = (MAXW - reserve).max(80);
                let mut info = format!("アプリケーション：{}", content.app_title);
                let (mut tw, th) = measure(hdc, &info, FONT_INFO, false, 4000);
                if tw > avail {
                    let base = "アプリケーション：";
                    let budget = ((avail as f32 / tw.max(1) as f32) * content.app_title.chars().count() as f32) as usize;
                    info = format!("{base}{}", crate::util::truncate_chars(&content.app_title, budget.max(4)));
                    let (tw2, _) = measure(hdc, &info, FONT_INFO, false, 4000);
                    tw = tw2;
                }
                let row_h = th.max(CHIP_H);
                let text_y = y + (row_h - th) / 2;
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: text_y, right: PAD + tw + 4, bottom: text_y + th },
                    text: info,
                    size: FONT_INFO,
                    color: COL_TEXT,
                    bold: false,
                });
                app_row_cap_btn = Some((cap_w, y));
                need_w = need_w.max(PAD + tw + 12 + btns_w + PANEL_MARGIN + 6);
                y += row_h + 4;
            }

            // UIAパスの各ノードをボタン化
            if !content.uia_nodes.is_empty() {
                let mut x = PAD;
                for (i, node) in content.uia_nodes.iter().enumerate() {
                    if i > 0 {
                        let sep = "＞";
                        let (sw, sh) = measure(hdc, sep, FONT_CHIP, false, 40);
                        let sy = y + (CHIP_H - sh) / 2;
                        items.push(Item::Text {
                            rect: RECT { left: x, top: sy, right: x + sw, bottom: sy + sh },
                            text: sep.to_string(),
                            size: FONT_CHIP,
                            color: COL_LABEL,
                            bold: false,
                        });
                        x += sw + 6;
                    }
                    let node_text = node.text.trim();
                    let has_text = !node_text.is_empty();
                    let lab = if has_text {
                        crate::util::truncate_chars(node_text, 10)
                    } else {
                        node.label.clone()
                    };
                    let (cw, _) = measure(hdc, &lab, FONT_CHIP, false, 600);
                    let w = cw + 18;
                    items.push(Item::Chip {
                        rect: RECT { left: x, top: y, right: x + w, bottom: y + CHIP_H },
                        label: lab,
                        id: CHIP_UIA_NODE_BASE + i,
                        active: has_text && node_text == content.source.trim(),
                        enabled: has_text,
                    });
                    x += w + 6;
                }
                need_w = need_w.max(x + PAD - 6);
                y += CHIP_H + 6;
            }

            // アプリ名が無い場合のフォールバック
            if content.has_image && content.app_title.is_empty() {
                let lab = "キャプチャ画像";
                let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                items.push(Item::Chip {
                    rect: RECT { left: PAD, top: y, right: PAD + cw + 20, bottom: y + CHIP_H },
                    label: lab.to_string(),
                    id: CHIP_IMAGE,
                    active: false,
                    enabled: true,
                });
                y += CHIP_H + 4;
            }
            y += 4;
            panel_spans.push((block_start, y, COL_ACCENT_INFO));
            y += 6;
        }

        // 【OCR結果】(UIA経路の場合はOCRを行わないため専用の見出しにする): コピーは左端
        if !content.source.is_empty() {
            let block_start = y;
            let heading = if content.via_uia {
                "【画面読み取り結果 (UIA取得)】".to_string()
            } else {
                format!("【OCR結果 ({})】", engine::ocr_label(&content.cur_ocr))
            };
            let heading_color = if content.via_uia { COL_UIA_BADGE } else { COL_LABEL };
            let hh = heading_row(
                &mut items,
                y,
                &heading,
                heading_color,
                &[("📋", CHIP_COPY_SRC, true)],
                true,
                &mut need_w,
            );
            y += hh + 4;

            let (sw, sh) = measure(hdc, &content.source, FONT_BODY, false, MAXW);
            let text_h = sh.max(24);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                text: content.source.clone(),
                size: FONT_BODY,
                color: COL_TEXT,
                bold: false,
            });
            y += text_h + 6;
            need_w = need_w.max(sw + PAD * 2 + 4);

            // UIA経路はOCRを行っていないため、どのOCRエンジンもアクティブ表示にしない
            let ocr_cur: &str = if content.via_uia { "" } else { content.cur_ocr.as_str() };
            chip_row(
                &mut items,
                &mut y,
                &engine::OCR_KEYS,
                &engine::OCR_LABELS,
                ocr_cur,
                &content.ocr_enabled,
                CHIP_OCR_BASE,
                &mut need_w,
            );
            let accent = if content.via_uia { COL_UIA_BADGE } else { COL_ACCENT_OCR };
            panel_spans.push((block_start, y, accent));
            y += 6;
        }

        // 【翻訳結果】またはステータス: コピーは左端、言語反転は右端(後で配置)
        if let Some(t) = &content.translation {
            let block_start = y;
            if !content.source_lang.is_empty() {
                let lab = format!("{}→{}", content.source_lang, content.target_lang);
                let (cw, _) = measure(hdc, &lab, FONT_CHIP, false, 200);
                need_w = need_w.max(PAD + 200 + cw + 18 + PANEL_MARGIN + 6);
                swap_btn = Some((lab, cw + 18, y));
            }
            let heading = if content.cur_tr == "llm" {
                if let Some(detail) = &content.tr_engine_detail {
                    format!("【翻訳結果(LLM:{})】", detail)
                } else {
                    format!("【翻訳結果 ({})】", engine::tr_label(&content.cur_tr))
                }
            } else {
                format!("【翻訳結果 ({})】", engine::tr_label(&content.cur_tr))
            };
            let hh = heading_row(
                &mut items,
                y,
                &heading,
                COL_LABEL,
                &[("📋", CHIP_COPY_TR, true)],
                true,
                &mut need_w,
            );
            y += hh + 4;

            let (tw, th) = measure(hdc, t, FONT_BODY, true, MAXW);
            let text_h = th.max(24);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                text: t.clone(),
                size: FONT_BODY,
                color: COL_TEXT,
                bold: true,
            });
            y += text_h + 8;
            need_w = need_w.max(tw + PAD * 2 + 4);

            chip_row(
                &mut items,
                &mut y,
                &engine::TR_KEYS,
                &engine::TR_LABELS,
                &content.cur_tr,
                &content.tr_enabled,
                CHIP_TR_BASE,
                &mut need_w,
            );
            panel_spans.push((block_start, y, COL_ACCENT_TR));
            y += 6;
        }

        // 【解説】ブロック
        let block_start = y;
        let heading = if let Some(_) = &content.explanation {
            if content.explain_engine.is_empty() {
                "【解説結果】".to_string()
            } else {
                format!("【解説結果 ({})】", content.explain_engine)
            }
        } else {
            "【解説】".to_string()
        };

        let copy_btns: &[(&str, usize, bool)] = &[("📋", CHIP_COPY, true)];

        let hh = heading_row(
            &mut items,
            y,
            &heading,
            COL_ACCENT_EXPLAIN,
            copy_btns,
            true,
            &mut need_w,
        );
        y += hh + 4;

        if content.explaining {
            let (tw, th) = measure(hdc, "解説を取得中...", FONT_HEADING, false, MAXW);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                text: "解説を取得中...".to_string(),
                size: FONT_HEADING,
                color: COL_STATUS,
                bold: false,
            });
            y += th + 8;
            need_w = need_w.max(tw + PAD * 2 + 4);
        } else if let Some(expl) = &content.explanation {
            let (tw, th) = measure(hdc, expl, FONT_BODY, false, MAXW);
            let text_h = th.max(20);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                text: expl.clone(),
                size: FONT_BODY,
                color: COL_TEXT,
                bold: false,
            });
            y += text_h + 8;
            need_w = need_w.max(tw + PAD * 2 + 4);
        }

        // 操作チップ: 解説 / プロンプト編集
        let mut x = PAD;
        let ops: &[(&str, usize)] = &[
            ("解説", CHIP_EXPLAIN_QUICK),
            ("解説プロンプトを編集して送信", CHIP_EXPLAIN),
        ];
        for (lab, id) in ops {
            let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
            let w = cw + 20;
            items.push(Item::Chip {
                rect: RECT { left: x, top: y, right: x + w, bottom: y + CHIP_H },
                label: lab.to_string(),
                id: *id,
                active: false,
                enabled: true,
            });
            x += w + 6;
        }
        need_w = need_w.max(x + PAD - 6);
        y += CHIP_H + 6;

        panel_spans.push((block_start, y, COL_ACCENT_EXPLAIN));
        y += 6;

        let _ = ReleaseDC(Some(hwnd), hdc);
        let w = need_w.min(MAXW + PAD * 2);

        // 処理中は閉じる以外の全チップを無効化 (ウィンドウ全体のロック)
        if content.busy {
            for item in &mut items {
                if let Item::Chip { id, enabled, .. } = item
                    && *id != CHIP_CLOSE
                {
                    *enabled = false;
                }
            }
        }

        // 右上角: システムメッセージ行と同じ行に [設定] [ログを開く] [📌] [✕] を右寄せで配置する。
        let ty_base = sys_msg_btns.map(|(_, _, y)| y).unwrap_or(6);
        let ty_close = ty_base + (CHIP_H - CLOSE_SIZE) / 2;

        let close_right = w - 6;
        let close_left = close_right - CLOSE_SIZE;
        let pin_right = close_left - 4;
        let pin_left = pin_right - CLOSE_SIZE;

        items.push(Item::Chip {
            rect: RECT { left: pin_left, top: ty_close, right: pin_right, bottom: ty_close + CLOSE_SIZE },
            label: "📌".to_string(),
            id: CHIP_PIN,
            active: content.pinned,
            enabled: !content.busy,
        });
        items.push(Item::Chip {
            rect: RECT { left: close_left, top: ty_close, right: close_right, bottom: ty_close + CLOSE_SIZE },
            label: "✕".to_string(),
            id: CHIP_CLOSE,
            active: false,
            enabled: true,
        });

        // 翻訳結果ブロックの右上に言語反転ボタン
        if let Some((lab, bw, hy)) = swap_btn {
            let right = w - PANEL_MARGIN - 6;
            let left = right - bw;
            items.push(Item::Chip {
                rect: RECT { left, top: hy - 1, right, bottom: hy - 1 + CLOSE_SIZE },
                label: lab,
                id: CHIP_SWAP_LANG,
                active: false,
                enabled: !content.busy,
            });
        }

        // システムメッセージ行の右端に [設定][ログを開く] を配置する。
        if let Some((set_w, log_w, ty)) = sys_msg_btns {
            let log_right = pin_left - 6;
            let log_left = log_right - log_w;
            items.push(Item::Chip {
                rect: RECT { left: log_left, top: ty, right: log_right, bottom: ty + CHIP_H },
                label: "ログを開く".to_string(),
                id: CHIP_OPEN_LOG,
                active: false,
                enabled: !content.busy,
            });
            let set_right = log_left - 6;
            let set_left = set_right - set_w;
            items.push(Item::Chip {
                rect: RECT { left: set_left, top: ty, right: set_right, bottom: ty + CHIP_H },
                label: "設定".to_string(),
                id: CHIP_SETTINGS,
                active: false,
                enabled: !content.busy,
            });
        }

        // 【アプリケーション】行の右端に [キャプチャ画像] を配置する。
        if let Some((cap_w, ty)) = app_row_cap_btn {
            if cap_w > 0 {
                let cap_right = w - PANEL_MARGIN - 6;
                let cap_left = cap_right - cap_w;
                items.push(Item::Chip {
                    rect: RECT { left: cap_left, top: ty, right: cap_right, bottom: ty + CHIP_H },
                    label: "キャプチャ画像".to_string(),
                    id: CHIP_IMAGE,
                    active: false,
                    enabled: !content.busy,
                });
            }
        }

        // ブロック(カード)の背景
        let panels: Vec<Panel> = panel_spans
            .iter()
            .map(|(top, bottom, accent)| Panel {
                rect: RECT { left: PANEL_MARGIN, top: top - 6, right: w - PANEL_MARGIN, bottom: bottom - 2 },
                accent: *accent,
            })
            .collect();

        let display_h = y.min(800);
        Layout { w, h: display_h, content_h: y, items, panels }
    }
}
