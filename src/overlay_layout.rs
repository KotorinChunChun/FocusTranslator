// オーバーレイのレイアウト計算 (SPEC v0.3 §3)
// OverlayContent からボタン・テキストの配置を計算し、Layout を返す。
// overlay.rs の描画 (paint) とウィンドウ管理から分離して可読性を高める。
use crate::engine;
use crate::overlay::{
    EditTool, OverlayContent, CHIP_CLOSE, CHIP_COPY, CHIP_COPY_INFO,
    CHIP_COPY_SRC, CHIP_COPY_TR, CHIP_EDIT_APPLY, CHIP_EDIT_CANCEL, CHIP_EDIT_ERASE,
    CHIP_EDIT_LASSO, CHIP_EDIT_RECT, CHIP_EDIT_RESET, CHIP_EDIT_UNDO, CHIP_EXPLAIN,
    CHIP_EXPLAIN_QUICK, CHIP_IMAGE, CHIP_OCR_BASE, CHIP_OPEN_LOG, CHIP_PIN, CHIP_SETTINGS,
    CHIP_SWAP_LANG, CHIP_TR_BASE, CHIP_UIA_NODE_BASE, CHIP_EDIT_SRC, CHIP_EDIT_TR, CHIP_EDIT_EXP,
};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK,
    DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, DrawTextW, FONT_OUTPUT_PRECISION, FW_BOLD,
    FW_NORMAL, GetDC, GetMonitorInfoW, HDC, HFONT, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, ReleaseDC, SelectObject,
};
use windows::core::w;

/// オーバーレイの配色一式。config.overlay_theme ("system" | "light" | "dark") に応じて
/// apply_theme() で THEME_DARK / THEME_LIGHT を切り替える。色値は COLORREF (0x00BBGGRR)。
#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub bg: u32,
    pub border: u32,
    /// 本文テキスト (アプリ名行・UIA/OCR結果・訳文) は背景と最大コントラストにする (SPECv0.4 §6)
    pub text: u32,
    pub status: u32,
    pub chip: u32,
    pub chip_active: u32,
    /// ホバー中のチップ背景色 (chip より明るいグレー。フォーカス可視化用)
    pub chip_hover: u32,
    /// ホバー中のアクティブチップ背景色 (chip_active より明るい同系色)
    pub chip_active_hover: u32,
    pub chip_text: u32,
    /// アクティブなチップ (chip_active/chip_active_hover 背景) の文字色。
    /// chip_active の橙背景は明背景でも読みやすいよう、両テーマ共通で白系にする。
    pub chip_active_text: u32,
    pub chip_disabled: u32,
    pub label: u32,
    pub panel_bg: u32,
    pub panel_border: u32,
    pub accent_info: u32,
    pub accent_tr: u32,
    pub accent_explain: u32,
    pub uia_badge: u32,
    pub close_hover: u32,
}

/// ダークテーマ (従来配色)
const THEME_DARK: Theme = Theme {
    bg: 0x00221E1C,
    border: 0x00524A46,
    text: 0x00FFFFFF,
    status: 0x0050C8FF,
    chip: 0x003F3833,
    chip_active: 0x00D28C3C,
    chip_hover: 0x00554C44,
    chip_active_hover: 0x00E6A356,
    chip_text: 0x00E8E4E0,
    chip_active_text: 0x00FFFFFF,
    chip_disabled: 0x00787068,
    label: 0x00908A84,
    panel_bg: 0x002B2723,
    panel_border: 0x00423C37,
    accent_info: 0x0050B4FF,
    accent_tr: 0x00C850DC,
    // 元の紫 (0x00FF5082) は暗背景で沈んで読みづらかったため、輝度の高いラベンダーへ調整。
    accent_explain: 0x00FFA8D8,
    uia_badge: 0x0080D0A0,
    close_hover: 0x003C3CD6,
};

/// ライトテーマ: ダークと同じ色相構成を明背景向けに調整する。
/// 背景は温かみのある白、カードは純白、本文は濃色。テキストとして使うアクセント色
/// (status / label / accent_info / accent_explain / uia_badge) は白地で読めるよう暗めに補正する。
const THEME_LIGHT: Theme = Theme {
    bg: 0x00F7F4F1,
    border: 0x00BFB8B3,
    text: 0x00201C1A,
    status: 0x002C6E8C,
    chip: 0x00E9E4DF,
    chip_active: 0x00D28C3C,
    chip_hover: 0x00D9D2CB,
    chip_active_hover: 0x00E6A356,
    chip_text: 0x003B3531,
    chip_active_text: 0x00FFFFFF,
    chip_disabled: 0x00ABA49E,
    label: 0x00706962,
    panel_bg: 0x00FFFFFF,
    panel_border: 0x00D9D3CE,
    accent_info: 0x0032709E,
    accent_tr: 0x00903A9E,
    // ダーク側と同じ色相 (ラベンダー) を白背景向けの明度に調整。
    accent_explain: 0x00C45A8E,
    uia_badge: 0x00467258,
    close_hover: 0x00A8A8E8,
};

thread_local! {
    static THEME_IS_LIGHT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 現在のテーマ (オーバーレイのレイアウト計算・描画から参照する)
pub fn theme() -> &'static Theme {
    if THEME_IS_LIGHT.with(|c| c.get()) { &THEME_LIGHT } else { &THEME_DARK }
}

/// 設定値 ("system" | "light" | "dark") から現在テーマを確定する。
/// "system" は Windows のアプリモード (AppsUseLightTheme) に追従する。
/// テーマが実際に切り替わったときのみ true を返す (呼び出し側の再描画判定用)。
pub fn apply_theme(mode: &str) -> bool {
    let light = match mode {
        "light" => true,
        "dark" => false,
        _ => crate::util::system_apps_light_theme(),
    };
    THEME_IS_LIGHT.with(|c| {
        let changed = c.get() != light;
        c.set(light);
        changed
    })
}

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
    /// 画像編集モードのプレビュー描画矩形とスケール (元画像ピクセル→画面座標の倍率)
    pub edit_preview: Option<(RECT, f32)>,
}

/// 編集プレビューの最大表示サイズ (縦横比維持で収める。拡大はしない。SPECv0.4 §2.2)
const EDIT_PREVIEW_MAX_W: i32 = 480;
const EDIT_PREVIEW_MAX_H: i32 = 460;

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
        let thm = theme();
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
            let msg = match &content.status {
                Some(s) if !s.is_empty() => format!("{} - {}", crate::util::APP_SHORT_NAME, s),
                _ => crate::util::APP_SHORT_NAME.to_string(),
            };
            let (tw, th) = measure(hdc, &msg, FONT_HEADING, false, MAXW);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + tw + 4, bottom: y + th },
                text: msg,
                size: FONT_HEADING,
                color: thm.status,
                bold: false,
            });
            y += th + PAD;
            need_w = need_w.max(tw + PAD * 2 + 4);
            let _ = ReleaseDC(Some(hwnd), hdc);
            return Layout { w: need_w.min(MAXW + PAD * 2), h: y, content_h: y, items, panels: Vec::new(), edit_preview: None };
        }

        // 見出し行を配置する。見出し文字は左寄せ、チップは右端に寄せて配置する。
        // 戻り値は行の高さ。
        let heading_row = |items: &mut Vec<Item>,
                           y: i32,
                           text: &str,
                           color: u32,
                           chips: &[(&str, usize, bool)],
                           need_w: &mut i32|
         -> i32 {
            let (hw, hh) = measure(hdc, text, FONT_HEADING, false, MAXW);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + hw + 4, bottom: y + hh },
                text: text.to_string(),
                size: FONT_HEADING,
                color,
                bold: false,
            });
            
            // チップ全体の幅を計算
            let mut chips_w = 0;
            let mut chip_sizes = Vec::new();
            for (lab, _, _) in chips {
                let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                let w = cw + 16;
                chip_sizes.push(w);
                chips_w += w + 6;
            }
            
            // 右端(MAXW + PAD)から逆算して配置
            let mut x = MAXW + PAD - chips_w;
            let row_h = hh.max(CLOSE_SIZE);
            for (i, (lab, id, enabled)) in chips.iter().enumerate() {
                let w = chip_sizes[i];
                items.push(Item::Chip {
                    rect: RECT { left: x, top: y - 1, right: x + w, bottom: y - 1 + CLOSE_SIZE },
                    label: lab.to_string(),
                    id: *id,
                    active: false,
                    enabled: *enabled,
                });
                x += w + 6;
            }
            *need_w = (*need_w).max(MAXW + PAD);
            row_h
        };

        let chip_row = |items: &mut Vec<Item>,
                        y: &mut i32,
                        keys: &[String],
                        labels: &[String],
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
        // アプリ名 (APP_SHORT_NAME) を常時タイトルとして表示し、状態メッセージがあれば
        // 「なにこれ？ - キャプチャしました。」のように続けて表示する。
        // 進行中メッセージ(末尾が「…」)はドットをアニメーションし、エラー等はそのまま出す。
        let mut msg_part = String::new();
        if let Some(s) = &content.status {
            let is_progress = s.ends_with('…');
            msg_part = s.clone();
            if is_progress {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let count = (millis / 300) % 4;
                msg_part = msg_part.replace("…", "");
                msg_part.push_str(&".".repeat(count as usize));
            }
        }

        if let Some(b) = &content.badge {
            if !msg_part.is_empty() {
                msg_part.push_str("  ");
            }
            msg_part.push_str(&format!("[{}]", b));
        }

        let mut sys_disp = crate::util::APP_SHORT_NAME.to_string();
        if !msg_part.is_empty() {
            sys_disp.push_str(" - ");
            sys_disp.push_str(&msg_part);
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
                color: thm.status,
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
                thm.accent_info,
                &[("📋", CHIP_COPY_INFO, true)],
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
                    color: thm.text,
                    bold: false,
                });
                app_row_cap_btn = Some((cap_w, y));
                need_w = need_w.max(PAD + tw + 12 + btns_w + PANEL_MARGIN + 6);
                y += row_h + 4;
            }

            // UIAパスの各ノードをボタン化。ネストが深く1行に収まらない場合は
            // ウィンドウ幅(MAXW)を広げず複数行に折り返す (SPECv0.4.8追補)。
            if !content.uia_nodes.is_empty() {
                let mut x = PAD;
                let mut max_x = PAD;
                for (i, node) in content.uia_nodes.iter().enumerate() {
                    let node_text = node.text.trim();
                    let has_text = !node_text.is_empty();
                    let lab = if has_text {
                        crate::util::truncate_chars(node_text, 10)
                    } else {
                        node.label.clone()
                    };
                    let (cw, _) = measure(hdc, &lab, FONT_CHIP, false, 600);
                    let chip_w = cw + 18;

                    let sep = "＞";
                    let (sep_w, sep_h) = measure(hdc, sep, FONT_CHIP, false, 40);
                    let needs_sep = i > 0;
                    let item_w = chip_w + if needs_sep { sep_w + 6 } else { 0 };

                    // 行末に収まらなければ次行へ折り返す。行頭の「＞」は描画しない。
                    if x > PAD && x + item_w > PAD + MAXW {
                        x = PAD;
                        y += CHIP_H + 6;
                    }

                    if needs_sep && x > PAD {
                        let sy = y + (CHIP_H - sep_h) / 2;
                        items.push(Item::Text {
                            rect: RECT { left: x, top: sy, right: x + sep_w, bottom: sy + sep_h },
                            text: sep.to_string(),
                            size: FONT_CHIP,
                            color: thm.label,
                            bold: false,
                        });
                        x += sep_w + 6;
                    }

                    items.push(Item::Chip {
                        rect: RECT { left: x, top: y, right: x + chip_w, bottom: y + CHIP_H },
                        label: lab,
                        id: CHIP_UIA_NODE_BASE + i,
                        active: has_text && node_text == content.source.trim(),
                        enabled: has_text,
                    });
                    x += chip_w + 6;
                    max_x = max_x.max(x);
                }
                need_w = need_w.max((max_x + PAD - 6).min(MAXW + PAD));
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
                    active: content.edit.is_some(),
                    enabled: true,
                });
                y += CHIP_H + 4;
            }
            y += 4;
            panel_spans.push((block_start, y, thm.accent_info));
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
            // OCR結果もUIA取得結果も同じ配色にする (取得経路による区別は見出し文言のみで示す)
            let heading_color = thm.uia_badge;
            let hh = heading_row(
                &mut items,
                y,
                &heading,
                heading_color,
                &[("📋", CHIP_COPY_SRC, true), ("✏️", CHIP_EDIT_SRC, true)],
                &mut need_w,
            );
            y += hh + 4;

            let (sw, sh) = measure(hdc, &content.source, FONT_BODY, false, MAXW);
            let text_h = sh.max(24);
            let rect = RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h };
            items.push(Item::Text {
                rect,
                text: content.source.clone(),
                size: FONT_BODY,
                color: thm.text,
                bold: false,
            });
            y += text_h + 6;
            need_w = need_w.max(sw + PAD * 2 + 4);

            // UIA経路はOCRを行っていないため、どのOCRエンジンもアクティブ表示にしない
            let ocr_cur: &str = if content.via_uia { "" } else { content.cur_ocr_chip_key.as_str() };
            chip_row(
                &mut items,
                &mut y,
                &content.ocr_keys,
                &content.ocr_labels,
                ocr_cur,
                &content.ocr_enabled,
                CHIP_OCR_BASE,
                &mut need_w,
            );
            let accent = thm.uia_badge;
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
                swap_btn = Some((lab, cw + 18, 0)); // y will be updated later
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
                thm.accent_tr,
                &[("📋", CHIP_COPY_TR, true), ("✏️", CHIP_EDIT_TR, true)],
                &mut need_w,
            );
            y += hh + 4;

            let (tw, th) = measure(hdc, t, FONT_BODY, true, MAXW);
            let text_h = th.max(24);
            let rect = RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h };
            items.push(Item::Text {
                rect,
                text: t.clone(),
                size: FONT_BODY,
                color: thm.text,
                bold: true,
            });
            y += text_h + 8;
            need_w = need_w.max(tw + PAD * 2 + 4);

            chip_row(
                &mut items,
                &mut y,
                &content.tr_keys,
                &content.tr_labels,
                &content.cur_tr_chip_key,
                &content.tr_enabled,
                CHIP_TR_BASE,
                &mut need_w,
            );
            if let Some((_, _, ref mut by)) = swap_btn {
                *by = y - CHIP_H;
            }
            panel_spans.push((block_start, y, thm.accent_tr));
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

        let copy_btns: &[(&str, usize, bool)] = &[("📋", CHIP_COPY, true), ("✏️", CHIP_EDIT_EXP, true)];

        let hh = heading_row(
            &mut items,
            y,
            &heading,
            thm.accent_explain,
            copy_btns,
            &mut need_w,
        );
        y += hh + 4;

        if content.explaining {
            let (tw, th) = measure(hdc, "解説を取得中...", FONT_HEADING, false, MAXW);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                text: "解説を取得中...".to_string(),
                size: FONT_HEADING,
                color: thm.status,
                bold: false,
            });
            y += th + 8;
            need_w = need_w.max(tw + PAD * 2 + 4);
        } else if let Some(expl) = &content.explanation {
            let (tw, th) = measure(hdc, expl, FONT_BODY, false, MAXW);
            let text_h = th.max(20);
            let rect = RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h };
            items.push(Item::Text {
                rect,
                text: expl.clone(),
                size: FONT_BODY,
                color: thm.text,
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

        panel_spans.push((block_start, y, thm.accent_explain));
        y += 6;

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

        // 画像編集中はOCR/翻訳エンジン切替・解説・言語反転・UIAノード・テキスト編集を禁止する
        // (編集セッションと再認識が衝突しUIが操作不能になるのを防ぐ)。
        // 編集用チップ(CHIP_EDIT_*)はこの後に追加されるため影響しない。
        if content.edit.is_some() {
            for item in &mut items {
                if let Item::Chip { id, enabled, .. } = item {
                    let forbidden = *id < CHIP_OCR_BASE + engine::OCR_KEYS.len()
                        || (*id >= CHIP_TR_BASE && *id < CHIP_COPY)
                        || matches!(
                            *id,
                            CHIP_EXPLAIN | CHIP_EXPLAIN_QUICK | CHIP_SWAP_LANG
                                | CHIP_EDIT_SRC | CHIP_EDIT_TR | CHIP_EDIT_EXP
                        )
                        || *id >= CHIP_UIA_NODE_BASE;
                    if forbidden {
                        *enabled = false;
                    }
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
                    active: content.edit.is_some(),
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

        // 画像編集パネル (SPECv0.4 §1-§4 + 追補): 既存コンテンツの右側に追加表示する。
        // ウィンドウ拡張 (§2.1) ・縮小表示 (§2.2) ・矩形/投げ輪トリミング用チップ (§3) に加え、
        // マウスホイールでの拡大表示に応じてウィンドウをモニタのワークエリアいっぱいまで広げる。
        let mut total_w = w;
        let mut total_h = display_h;
        let mut edit_preview: Option<(RECT, f32)> = None;
        if let Some(info) = &content.edit {
            let iw = (info.img_w.max(1)) as f32;
            let ih = (info.img_h.max(1)) as f32;

            // アンカー位置のモニタのワークエリアから、プレビューに使える最大サイズを動的に求める
            // (固定480x460ではなく、実際の画面サイズいっぱいまでウィンドウを拡張できるようにする)
            let (max_pw, max_ph) = {
                let pt = POINT { x: content.anchor.0, y: content.anchor.1 };
                let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(hmon, &mut mi);
                let wa = mi.rcWork;
                let screen_w = wa.right - wa.left;
                let screen_h = wa.bottom - wa.top;
                let mw = screen_w - w - PANEL_MARGIN * 2 - 10 - PAD * 2;
                let mh = screen_h - PAD * 2 - (CHIP_H + 8) - (CHIP_H + PAD) - 10;
                (mw.max(EDIT_PREVIEW_MAX_W).min(4000), mh.max(EDIT_PREVIEW_MAX_H).min(4000))
            };

            // 基準スケール: 拡大はせず(zoom=1.0時)最大サイズに収める。ズーム倍率を掛けた上で、
            // 最大サイズ(=モニタのワークエリアから求めた上限)を超えないようクランプする。
            let base_fit = (max_pw as f32 / iw).min(max_ph as f32 / ih).min(1.0);
            let hard_max = (max_pw as f32 / iw).min(max_ph as f32 / ih);
            let scale = (base_fit * info.zoom).clamp(base_fit * 0.2, hard_max.max(base_fit));
            let pw = ((iw * scale).round() as i32).max(40);
            let ph = ((ih * scale).round() as i32).max(40);

            let panel_left = w + PANEL_MARGIN * 2 + 10;
            let mut ey = PAD;
            let mut edit_max_x = panel_left;

            // ツール+選択操作チップ (矩形/投げ輪/選択解除/選択範囲を残す/選択範囲を消す)
            let mut ex = panel_left;
            let selection_enabled = info.has_selection && !content.busy;
            let tools: [(&str, usize, bool, bool); 5] = [
                ("矩形", CHIP_EDIT_RECT, info.tool == EditTool::Rect, !content.busy),
                ("投げ輪", CHIP_EDIT_LASSO, info.tool == EditTool::Lasso, !content.busy),
                ("選択解除", CHIP_EDIT_RESET, false, !content.busy),
                ("選択範囲を残す", CHIP_EDIT_APPLY, false, selection_enabled),
                ("選択範囲を消す", CHIP_EDIT_ERASE, false, selection_enabled),
            ];
            for (lab, id, active, enabled) in tools {
                let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                let cwid = cw + 18;
                items.push(Item::Chip {
                    rect: RECT { left: ex, top: ey, right: ex + cwid, bottom: ey + CHIP_H },
                    label: lab.to_string(),
                    id,
                    active,
                    enabled,
                });
                ex += cwid + 6;
            }
            edit_max_x = edit_max_x.max(ex - 6 + PAD);
            ey += CHIP_H + 8;

            // 画像プレビュー (縦横比維持。ズーム未操作時は拡大しない。ホイールで拡大縮小できる)
            let preview_rect = RECT { left: panel_left, top: ey, right: panel_left + pw, bottom: ey + ph };
            edit_preview = Some((preview_rect, scale));
            edit_max_x = edit_max_x.max(preview_rect.right + PAD);
            ey += ph + 10;

            // 元に戻す/編集終了
            let undo_enabled = info.has_undo && !content.busy;
            let mut ax = panel_left;
            let acts: [(&str, usize, bool); 2] = [
                ("元に戻す", CHIP_EDIT_UNDO, undo_enabled),
                ("編集終了", CHIP_EDIT_CANCEL, !content.busy),
            ];
            for (lab, id, enabled) in acts {
                let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                let cwid = cw + 18;
                items.push(Item::Chip {
                    rect: RECT { left: ax, top: ey, right: ax + cwid, bottom: ey + CHIP_H },
                    label: lab.to_string(),
                    id,
                    active: false,
                    enabled,
                });
                ax += cwid + 6;
            }
            edit_max_x = edit_max_x.max(ax - 6 + PAD);
            ey += CHIP_H + PAD;

            total_w = edit_max_x;
            total_h = total_h.max(ey);
        }

        let _ = ReleaseDC(Some(hwnd), hdc);
        Layout { w: total_w, h: total_h, content_h: y, items, panels, edit_preview }
    }
}
