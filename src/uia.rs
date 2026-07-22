// UIA テキスト取得経路 (SPEC §6.1)
// ElementFromPoint → TextPattern → RangeFromPoint → 行単位に拡張 → GetText
// TextPattern が無ければ祖先を数段探索。取得不可なら text=None を返し OCR 経路へ。
use windows::Win32::Foundation::{LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextRange, IUIAutomationTextRangeArray, IUIAutomationValuePattern,
    TextUnit_Line, TextUnit_Paragraph, UIA_TextPatternId, UIA_ValuePatternId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_AppBarControlTypeId, UIA_ButtonControlTypeId, UIA_CalendarControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId, UIA_CustomControlTypeId,
    UIA_DataGridControlTypeId, UIA_DataItemControlTypeId, UIA_GroupControlTypeId,
    UIA_HeaderControlTypeId, UIA_HeaderItemControlTypeId, UIA_HyperlinkControlTypeId,
    UIA_ImageControlTypeId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SemanticZoomControlTypeId, UIA_SeparatorControlTypeId,
    UIA_SliderControlTypeId, UIA_SpinnerControlTypeId, UIA_SplitButtonControlTypeId,
    UIA_StatusBarControlTypeId, UIA_TabControlTypeId, UIA_TabItemControlTypeId,
    UIA_TableControlTypeId, UIA_TextControlTypeId, UIA_ThumbControlTypeId,
    UIA_TitleBarControlTypeId, UIA_ToolBarControlTypeId, UIA_ToolTipControlTypeId,
    UIA_TreeControlTypeId, UIA_TreeItemControlTypeId, UIA_WindowControlTypeId,
};
use windows::core::Interface;

/// UIA由来テキストに混入する不可視文字か (U+FFFC: 埋め込みオブジェクトの代替文字。
/// Teams等のリッチテキストではアイコン/アバター1個につき1文字入り、大量に混入することがある。
/// ゼロ幅文字・BOMも同様に翻訳対象として無意味なため除去する)
fn is_invisible_char(c: char) -> bool {
    matches!(c, '\u{FFFC}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}')
}

/// カーソル位置の段落/行が特定できない場合に、UIA全文をそのまま採用してよい上限文字数。
/// これを超える場合は誤って長文全体を読み取り結果にしてしまう(段落特定に失敗しているだけ
/// かもしれない)リスクが高いため、従来通りOCRへフォールバックする。
const UIA_FULLTEXT_MAX: usize = 200;

/// 段落/行が特定できない全文を、カーソル位置の特定を待たずそのままUIA経路のテキストとして
/// 採用してよいか (UIA_FULLTEXT_MAX 以下か)。
fn should_adopt_fulltext(char_count: usize) -> bool {
    char_count <= UIA_FULLTEXT_MAX
}

/// UIAから取得したテキストの不可視文字を除去する。除去跡が単語同士の連結にならないよう
/// 一旦空白へ置換してから、行を保ったまま連続する空白を1つに畳む。
fn sanitize_uia_text(s: &str) -> String {
    let replaced: String = s.chars().map(|c| if is_invisible_char(c) { ' ' } else { c }).collect();
    replaced
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// TextPattern 探索の結果: テキストが見つかった要素と、その行(または文書)範囲
struct TextHit {
    element: IUIAutomationElement,
    range: Option<IUIAutomationTextRange>,
    text: String,
}

/// pt を含む要素から祖先方向へ TextPattern を探し、カーソル行のテキストと範囲を得る。
/// line_at_point(認識経路) と probe_at_point(領域検出モード) で共用する探索コア。
fn find_text_hit(auto: &IUIAutomation, el: IUIAutomationElement, pt: POINT) -> Option<TextHit> {
    unsafe {
        let walker = auto.RawViewWalker().ok()?;
        let mut cur = Some(el);
        for _ in 0..6 {
            let Some(e) = cur.clone() else { break };

            // カーソルが要素の領域から大きく外れている場合は誤検出を防ぐ
            if let Ok(rect) = e.CurrentBoundingRectangle() {
                let margin = 16;
                if pt.x < rect.left - margin || pt.x > rect.right + margin || pt.y < rect.top - margin || pt.y > rect.bottom + margin {
                    cur = walker.GetParentElement(&e).ok();
                    continue;
                }
            }

            let ctrl_type = e.CurrentControlType().unwrap_or(windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID(0));
            let is_edit = ctrl_type == UIA_EditControlTypeId || ctrl_type == UIA_DocumentControlTypeId;

            if let Ok(unk) = e.GetCurrentPattern(UIA_TextPatternId)
                && let Ok(tp) = unk.cast::<IUIAutomationTextPattern>() {

                    // 複数行Edit/Documentで、後段のRangeFromPointによる段落/行特定が
                    // 失敗した場合の最終フォールバック候補。単一行なら従来通り即採用する。
                    let mut fulltext_fallback: Option<(IUIAutomationTextRange, String)> = None;

                    if is_edit
                        && let Ok(doc_range) = tp.DocumentRange()
                        && let Ok(full_text) = doc_range.GetText(-1)
                    {
                        let full_str = sanitize_uia_text(&full_text.to_string());
                        let is_multiline = full_str.contains('\n') || full_str.contains('\r');
                        let line = full_str.trim().to_string();
                        if !is_multiline {
                            if !line.is_empty() {
                                return Some(TextHit { element: e, range: Some(doc_range), text: line });
                            }
                        } else if !line.is_empty() {
                            fulltext_fallback = Some((doc_range, line));
                        }
                    }

                    if let Ok(range) = tp.RangeFromPoint(pt) {
                        // まず段落単位で拡張する: 右端で折り返された文も改行を跨いで
                        // 1つの段落として正確に取れる (ユーザー要望)。
                        // 取れない・長すぎる場合は従来の行単位へフォールバック。
                        if let Ok(para) = range.Clone() {
                            let _ = para.ExpandToEnclosingUnit(TextUnit_Paragraph);
                            if let Ok(t) = para.GetText(1200) {
                                let s = sanitize_uia_text(&t.to_string());
                                let lines: Vec<String> =
                                    s.lines().map(|l| l.trim().to_string()).collect();
                                let joined = crate::ocr::join_paragraph(&lines);
                                if !joined.is_empty() && joined.chars().count() <= 800 {
                                    return Some(TextHit { element: e, range: Some(para), text: joined });
                                }
                            }
                        }
                        let _ = range.ExpandToEnclosingUnit(TextUnit_Line);
                        if let Ok(text) = range.GetText(512) {
                            let s = sanitize_uia_text(&text.to_string());
                            let line = first_meaningful_line(&s);
                            if !line.is_empty() {
                                return Some(TextHit { element: e, range: Some(range), text: line });
                            }
                        }
                    }

                    // 段落/行が特定できなかった。全文が短ければ(UIA_FULLTEXT_MAX以下)
                    // OCRへ落とさずそのまま採用する (SPECv0.5追補: UIA優先強化)。
                    if let Some((range, text)) = fulltext_fallback
                        && should_adopt_fulltext(text.chars().count()) {
                            return Some(TextHit { element: e, range: Some(range), text });
                        }
                }

            // Win32 EDIT 等 TextPattern 非対応のコントロールでは上のTextPattern経路が
            // 何も返さない (ヒットテストは正しく命中しているが GetCurrentPattern が失敗する)。
            // ValuePattern は多くの環境で全文を返すため、OCRへ落とす前にここで試す。
            if is_edit
                && let Ok(unk) = e.GetCurrentPattern(UIA_ValuePatternId)
                && let Ok(vp) = unk.cast::<IUIAutomationValuePattern>()
                && let Ok(val) = vp.CurrentValue()
            {
                let line = sanitize_uia_text(&val.to_string()).trim().to_string();
                if !line.is_empty() {
                    return Some(TextHit { element: e, range: None, text: line });
                }
            }
            cur = walker.GetParentElement(&e).ok();
        }
        None
    }
}

/// 選択範囲テキストとして扱う上限文字数 (段落全文と同じ基準)
const SELECTED_TEXT_MAX: usize = 800;

/// 要素のTextPatternから現在選択中のテキストを取得する。選択が無い(長さ0)、
/// TextPattern非対応、全選択範囲が空白のみの場合は None。
/// 標準Win32 EDITコントロールはCOM UIAでもTextPatternに非対応のため常にここで失敗する
/// (SPECv0.5.4 §12実測)。呼び出し元は失敗時に `selected_text_via_win32_edit` を試す。
fn selected_text_of(e: &IUIAutomationElement) -> Option<String> {
    unsafe {
        let unk = e.GetCurrentPattern(UIA_TextPatternId).ok()?;
        let tp = unk.cast::<IUIAutomationTextPattern>().ok()?;
        let sel: IUIAutomationTextRangeArray = tp.GetSelection().ok()?;
        let len = sel.Length().ok()?;
        if len <= 0 {
            return None;
        }
        let mut parts = Vec::new();
        let mut budget = SELECTED_TEXT_MAX;
        for i in 0..len {
            if budget == 0 {
                break;
            }
            if let Ok(range) = sel.GetElement(i)
                && let Ok(t) = range.GetText(budget as i32)
            {
                let s = sanitize_uia_text(&t.to_string());
                let s = s.trim();
                if !s.is_empty() {
                    budget = budget.saturating_sub(s.chars().count());
                    parts.push(s.to_string());
                }
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.join(" "))
    }
}

/// 標準Win32 EDITコントロール向けフォールバック (SPECv0.5.4 §12)。
/// TextPatternに非対応でも、要素がネイティブウィンドウ(EDITクラス等)を持っていれば
/// EM_GETSEL/WM_GETTEXTを直接送って選択範囲を取得できる。UIAパターンを介さないため
/// フォーカスの有無やUIAプロキシの実装差に左右されない。
fn selected_text_via_win32_edit(e: &IUIAutomationElement) -> Option<String> {
    unsafe {
        let hwnd = e.CurrentNativeWindowHandle().ok()?;
        if hwnd.is_invalid() {
            return None;
        }
        let mut start: u32 = 0;
        let mut end: u32 = 0;
        SendMessageW(
            hwnd,
            windows::Win32::UI::Controls::EM_GETSEL,
            Some(WPARAM(&mut start as *mut u32 as usize)),
            Some(LPARAM(&mut end as *mut u32 as isize)),
        );
        if end <= start {
            return None;
        }
        let text_len = SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::WM_GETTEXTLENGTH,
            None,
            None,
        )
        .0 as usize;
        if text_len == 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; text_len + 1];
        let copied = SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::WM_GETTEXT,
            Some(WPARAM(buf.len())),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        )
        .0 as usize;
        buf.truncate(copied.min(buf.len()));
        let (s, en) = (start as usize, (end as usize).min(buf.len()));
        if s >= en || s >= buf.len() {
            return None;
        }
        let selected = String::from_utf16_lossy(&buf[s..en]);
        let sanitized = sanitize_uia_text(&selected);
        let trimmed = sanitized.trim();
        if trimmed.chars().count() > SELECTED_TEXT_MAX {
            return Some(crate::util::truncate_chars(trimmed, SELECTED_TEXT_MAX));
        }
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    }
}

/// UIA 検出結果 (認識経路と領域検出モードで共用)
pub struct UiaProbe {
    /// ElementFromPoint 直下要素の矩形 (TextPattern の有無に関わらず)
    pub hover_rect: Option<RECT>,
    /// TextPattern が見つかった(=実際に認識へ使われる)要素の矩形
    pub element_rect: Option<RECT>,
    /// カーソル行テキスト範囲の矩形群
    pub line_rects: Vec<RECT>,
    /// 要素のノード名 (AutomationId / Name / ControlType)
    pub node: String,
    /// 認識経路が返すはずのテキスト (UIA不可なら None → OCR経路)
    pub text: Option<String>,
    /// TextPattern が無い直下要素の全テキスト (Name プロパティ)。
    /// ブラウザ等ではテキストブロックの内容が入るため、OCRしたカーソル行を
    /// この中から検索して正確な1段落を復元するのに使う (worker::paragraph_from_text)。
    pub hover_text: Option<String>,
    /// カーソル位置要素で現在選択されているテキスト (SPECv0.5追補)。
    /// 選択が無い/TextPattern非対応なら None。認識サイクルではこれが最優先で採用される。
    pub selected_text: Option<String>,
}

/// カーソル位置で line_at_point と同じ探索を行い、検出領域の矩形を返す (領域検出モード)。
/// 呼び出し元スレッドは COM 初期化済みであること。
pub fn probe_at_point(x: i32, y: i32) -> UiaProbe {
    let mut p = UiaProbe {
        hover_rect: None,
        element_rect: None,
        line_rects: Vec::new(),
        node: String::new(),
        text: None,
        hover_text: None,
        selected_text: None,
    };
    unsafe {
        let Ok(auto) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return p;
        };
        let pt = POINT { x, y };
        let Ok(el) = auto.ElementFromPoint(pt) else {
            return p;
        };
        p.hover_rect = el.CurrentBoundingRectangle().ok();
        p.node = element_node_name(&el);
        if let Some(hit) = find_text_hit(&auto, el.clone(), pt) {
            p.element_rect = hit.element.CurrentBoundingRectangle().ok();
            p.node = element_node_name(&hit.element);
            if let Some(r) = &hit.range {
                p.line_rects = range_rects(r);
            }
            // TextPattern非対応(標準EDIT等)ならEM_GETSEL経由のフォールバックを試す (SPECv0.5.4 §12)
            p.selected_text = selected_text_of(&hit.element)
                .or_else(|| selected_text_via_win32_edit(&hit.element));
            p.text = Some(hit.text);
        } else if let Ok(name) = el.CurrentName() {
            p.selected_text = selected_text_of(&el).or_else(|| selected_text_via_win32_edit(&el));
            // TextPattern なし: Name に要素の全テキストが入っていれば段落復元に使う
            let s = sanitize_uia_text(&name.to_string());
            let t = s.trim();
            if t.chars().count() >= 8 && t.chars().count() <= 10000 {
                // 全文がUIA_FULLTEXT_MAX以下なら、カーソル位置の段落特定を待たず
                // このままUIA経路のテキストとして確定させる (SPECv0.5追補: UIA優先強化)。
                // 超える場合は従来通りOCR結果の段落復元用に保持する。
                if should_adopt_fulltext(t.chars().count()) {
                    p.text = Some(t.to_string());
                } else {
                    p.hover_text = Some(t.to_string());
                }
            }
        }
    }
    p
}

/// テキスト範囲の GetBoundingRectangles (SAFEARRAY of f64 [l,t,w,h,...]) を RECT 群へ変換
fn range_rects(range: &IUIAutomationTextRange) -> Vec<RECT> {
    let mut out = Vec::new();
    unsafe {
        let Ok(sa) = range.GetBoundingRectangles() else { return out };
        if sa.is_null() {
            return out;
        }
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        if SafeArrayAccessData(sa, &mut data).is_ok() {
            if let Ok(ub) = SafeArrayGetUBound(sa, 1) {
                let vals = data as *const f64;
                let n = (ub + 1).max(0) as usize;
                let mut i = 0;
                while i + 3 < n {
                    let l = *vals.add(i) as i32;
                    let t = *vals.add(i + 1) as i32;
                    let w = *vals.add(i + 2) as i32;
                    let h = *vals.add(i + 3) as i32;
                    out.push(RECT { left: l, top: t, right: l + w, bottom: t + h });
                    i += 4;
                }
            }
            let _ = SafeArrayUnaccessData(sa);
        }
        let _ = SafeArrayDestroy(sa);
    }
    out
}

fn first_meaningful_line(s: &str) -> String {
    s.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

/// パスノードの種別。祖先ノード(パス階層)か、末端ノードの子孫テキストを連結した合成ノードか。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum NodeKind {
    /// 祖先要素 (AutomationId/Name/ControlType のパス階層)
    #[default]
    Ancestor,
    /// カーソル直下の末端要素の子孫テキストをすべて連結した合成ノード
    ChildrenConcat,
}

/// UIAパスの1ノード。ボタン化・クリック時のテキスト採用・ログ記録に使う。
/// SPECv0.5.2追補: 取得元プロパティを落とさずJSONで記録するため各値を保持する
/// (ChildrenConcatノードは合成ノードのため control_type 等は空のまま)。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct UiaPathNode {
    /// ボタン表示用の短い識別ラベル (AutomationId/Name/ControlType)。
    /// text が空の場合のフォールバック表示にも使う。
    pub label: String,
    /// このノードから抽出したテキスト (クリック時に原文として採用する全文)
    pub text: String,
    pub kind: NodeKind,
    /// UIA ControlType の表示名 (例: "Edit", "Pane")
    #[serde(default)]
    pub control_type: String,
    /// UIA AutomationId
    #[serde(default)]
    pub automation_id: String,
    /// UIA Name プロパティ
    #[serde(default)]
    pub name: String,
    /// UIA ClassName プロパティ
    #[serde(default)]
    pub class_name: String,
    /// スクリーン座標での矩形 (left, top, width, height)
    #[serde(default)]
    pub rect: Option<(i32, i32, i32, i32)>,
}


/// 子孫走査の上限(パフォーマンスと無関係テキスト混入の抑制のため)
const DESC_MAX_DEPTH: u32 = 6;
const DESC_MAX_NODES: usize = 200;
const DESC_MAX_CHARS: usize = 4000;

/// カーソル位置の要素のUIAパスノード列を取得する。
/// 祖先方向に最大5段(AutomationId/Name/ControlTypeの短い識別子 + そのノード自身から
/// 抽出したテキスト)を積み、末尾にカーソル直下の末端要素の子孫テキストを再帰的に
/// 走査して1行ずつ連結した合成ノードを追加する (ボタン化してOCRの代わりに採用するため)。
/// 解説機能の同一コンテキスト判定キーや認識ログにも使う。
pub fn path_nodes_at_point(x: i32, y: i32) -> Vec<UiaPathNode> {
    unsafe {
        let Ok(auto) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return Vec::new();
        };
        let Ok(el) = auto.ElementFromPoint(POINT { x, y }) else {
            return Vec::new();
        };
        let Ok(walker) = auto.ControlViewWalker() else {
            return Vec::new();
        };
        let leaf = el.clone();
        let mut path = Vec::new();
        let mut cur = Some(el);
        for _ in 0..5 {
            let Some(e) = cur.clone() else { break };
            let name = element_node_name(&e);
            if !name.is_empty() {
                let text = element_own_text(&e);
                let (control_type, automation_id, elem_name, class_name, rect) = element_props(&e);
                path.push(UiaPathNode {
                    label: name,
                    text,
                    kind: NodeKind::Ancestor,
                    control_type,
                    automation_id,
                    name: elem_name,
                    class_name,
                    rect,
                });
            }
            cur = walker.GetParentElement(&e).ok();
        }
        path.reverse();

        let mut lines = Vec::new();
        let mut visited = DESC_MAX_NODES;
        let mut budget = DESC_MAX_CHARS;
        let mut seen = std::collections::HashSet::new();
        collect_descendant_texts(&walker, &leaf, DESC_MAX_DEPTH, &mut visited, &mut budget, &mut seen, &mut lines);
        if !lines.is_empty() {
            path.push(UiaPathNode {
                label: "子要素".into(),
                text: lines.join("\n"),
                kind: NodeKind::ChildrenConcat,
                ..Default::default()
            });
        }
        path
    }
}

/// UIAパスノード列をJSON文字列へシリアライズする (SPECv0.5.2追補: ログDB記録用)。
/// 失敗時(通常は起きない)は空配列文字列を返す。
pub fn nodes_to_json(nodes: &[UiaPathNode]) -> String {
    serde_json::to_string(nodes).unwrap_or_else(|_| "[]".into())
}

/// 要素自身のテキストを抽出する。TextPattern があれば全文、無ければ ValuePattern、
/// それも無ければ Name を使う (Win32 EDIT 等はTextPatternが無く、Nameはラベルの
/// 流用になっていることがあるため ValuePattern を優先する)。
fn element_own_text(e: &IUIAutomationElement) -> String {
    unsafe {
        if let Ok(unk) = e.GetCurrentPattern(UIA_TextPatternId)
            && let Ok(tp) = unk.cast::<IUIAutomationTextPattern>()
            && let Ok(doc) = tp.DocumentRange()
            && let Ok(t) = doc.GetText(4000) {
                let s = sanitize_uia_text(&t.to_string());
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
        if let Ok(unk) = e.GetCurrentPattern(UIA_ValuePatternId)
            && let Ok(vp) = unk.cast::<IUIAutomationValuePattern>()
            && let Ok(val) = vp.CurrentValue() {
                let s = sanitize_uia_text(&val.to_string());
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
        e.CurrentName()
            .map(|n| sanitize_uia_text(&n.to_string()).trim().to_string())
            .unwrap_or_default()
    }
}

/// parent の子孫を深さ優先で走査し、各ノード自身のテキスト(element_own_text)を1行ずつ集める。
/// depth・件数・総文字数の上限で打ち切り、親と同一のテキストは重複除外する
/// (アクセシビリティツリーでは親のNameが単一の子の集約になっていることが多いため)。
#[allow(clippy::too_many_arguments)]
fn collect_descendant_texts(
    walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    parent: &IUIAutomationElement,
    depth: u32,
    visited: &mut usize,
    budget: &mut usize,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    if depth == 0 || *visited == 0 || *budget == 0 {
        return;
    }
    let mut cur = unsafe { walker.GetFirstChildElement(parent).ok() };
    while let Some(e) = cur {
        if *visited == 0 || *budget == 0 {
            break;
        }
        *visited -= 1;
        let t = element_own_text(&e);
        if !t.is_empty() && seen.insert(t.clone()) {
            let take = t.chars().count().min(*budget);
            let s: String = t.chars().take(take).collect();
            *budget -= take;
            if !s.is_empty() {
                out.push(s);
            }
        }
        collect_descendant_texts(walker, &e, depth - 1, visited, budget, seen, out);
        cur = unsafe { walker.GetNextSiblingElement(&e).ok() };
    }
}

/// カーソル位置の要素のControlType名 (Edit/Document/Text/Button 等) を取得する。
/// TextPatternが見つかった要素があればその種類、無ければ ElementFromPoint 直下要素の種類を返す
/// (入力内容ログへの記録用: SPECv0.4.8追補)。
pub fn control_type_at_point(x: i32, y: i32) -> Option<String> {
    unsafe {
        let auto = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let pt = POINT { x, y };
        let el = auto.ElementFromPoint(pt).ok()?;
        let target = find_text_hit(&auto, el.clone(), pt).map(|h| h.element).unwrap_or(el);
        control_type_name(&target)
    }
}

fn control_type_name(e: &IUIAutomationElement) -> Option<String> {
    unsafe {
        let ctrl = e.CurrentControlType().ok()?;
        Some(control_type_label(ctrl).to_string())
    }
}

/// UIA_CONTROLTYPE_ID → 表示用の短い英語名
#[allow(non_upper_case_globals)]
fn control_type_label(id: windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID) -> &'static str {
    match id {
        UIA_ButtonControlTypeId => "Button",
        UIA_CalendarControlTypeId => "Calendar",
        UIA_CheckBoxControlTypeId => "CheckBox",
        UIA_ComboBoxControlTypeId => "ComboBox",
        UIA_EditControlTypeId => "Edit",
        UIA_HyperlinkControlTypeId => "Hyperlink",
        UIA_ImageControlTypeId => "Image",
        UIA_ListItemControlTypeId => "ListItem",
        UIA_ListControlTypeId => "List",
        UIA_MenuControlTypeId => "Menu",
        UIA_MenuBarControlTypeId => "MenuBar",
        UIA_MenuItemControlTypeId => "MenuItem",
        UIA_ProgressBarControlTypeId => "ProgressBar",
        UIA_RadioButtonControlTypeId => "RadioButton",
        UIA_ScrollBarControlTypeId => "ScrollBar",
        UIA_SliderControlTypeId => "Slider",
        UIA_SpinnerControlTypeId => "Spinner",
        UIA_StatusBarControlTypeId => "StatusBar",
        UIA_TabControlTypeId => "Tab",
        UIA_TabItemControlTypeId => "TabItem",
        UIA_TextControlTypeId => "Text",
        UIA_ToolBarControlTypeId => "ToolBar",
        UIA_ToolTipControlTypeId => "ToolTip",
        UIA_TreeControlTypeId => "Tree",
        UIA_TreeItemControlTypeId => "TreeItem",
        UIA_CustomControlTypeId => "Custom",
        UIA_GroupControlTypeId => "Group",
        UIA_ThumbControlTypeId => "Thumb",
        UIA_DataGridControlTypeId => "DataGrid",
        UIA_DataItemControlTypeId => "DataItem",
        UIA_DocumentControlTypeId => "Document",
        UIA_SplitButtonControlTypeId => "SplitButton",
        UIA_WindowControlTypeId => "Window",
        UIA_PaneControlTypeId => "Pane",
        UIA_HeaderControlTypeId => "Header",
        UIA_HeaderItemControlTypeId => "HeaderItem",
        UIA_TableControlTypeId => "Table",
        UIA_TitleBarControlTypeId => "TitleBar",
        UIA_SeparatorControlTypeId => "Separator",
        UIA_SemanticZoomControlTypeId => "SemanticZoom",
        UIA_AppBarControlTypeId => "AppBar",
        _ => "Unknown",
    }
}

/// element_props() の戻り値: (ControlType表示名, AutomationId, Name, ClassName,
/// スクリーン矩形(left,top,w,h))
type ElemProps = (String, String, String, String, Option<(i32, i32, i32, i32)>);

/// SPECv0.5.2追補: JSON記録用にノードのUIAプロパティ一式を取得する。
/// RuntimeIdはセッション限りの値で保存価値が低いため取得しない。
fn element_props(e: &IUIAutomationElement) -> ElemProps {
    unsafe {
        let control_type = control_type_name(e).unwrap_or_default();
        let automation_id = e.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default();
        let name = e.CurrentName().map(|s| s.to_string()).unwrap_or_default();
        let class_name = e.CurrentClassName().map(|s| s.to_string()).unwrap_or_default();
        let rect = e.CurrentBoundingRectangle().ok().map(|r| (r.left, r.top, r.right - r.left, r.bottom - r.top));
        (control_type, automation_id, name, class_name, rect)
    }
}

/// UIAパスの1ノード名: AutomationId → Name → ControlType の優先順で採用
fn element_node_name(e: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> String {
    unsafe {
        if let Ok(id) = e.CurrentAutomationId() {
            let s = id.to_string();
            if !s.is_empty() {
                return s;
            }
        }
        if let Ok(name) = e.CurrentName() {
            let s = name.to_string();
            if !s.is_empty() {
                return s;
            }
        }
        match e.CurrentControlType() {
            Ok(ctrl) => format!("Type{}", ctrl.0),
            Err(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UIA_FULLTEXT_MAX, sanitize_uia_text, should_adopt_fulltext};

    #[test]
    fn adopts_fulltext_at_or_below_threshold() {
        assert!(should_adopt_fulltext(0));
        assert!(should_adopt_fulltext(UIA_FULLTEXT_MAX));
        assert!(!should_adopt_fulltext(UIA_FULLTEXT_MAX + 1));
    }

    #[test]
    fn removes_scattered_object_replacement_chars() {
        let s = "Lists all the teams in Microsoft Teams that you are a member of \u{FFFC} \u{FFFC} \u{FFFC}";
        assert_eq!(sanitize_uia_text(s), "Lists all the teams in Microsoft Teams that you are a member of");
    }

    #[test]
    fn removes_adjacent_object_replacement_chars_without_merging_words() {
        let s = "word\u{FFFC}\u{FFFC}\u{FFFC}word";
        assert_eq!(sanitize_uia_text(s), "word word");
    }

    #[test]
    fn removes_zero_width_and_bom_chars() {
        let s = "hello\u{200B}\u{FEFF}world";
        assert_eq!(sanitize_uia_text(s), "hello world");
    }

    #[test]
    fn preserves_line_breaks_while_collapsing_internal_spaces() {
        let s = "first  line\u{FFFC}\nsecond\u{FFFC} line";
        assert_eq!(sanitize_uia_text(s), "first line\nsecond line");
    }

    #[test]
    fn leaves_normal_text_untouched() {
        let s = "Hello, world!";
        assert_eq!(sanitize_uia_text(s), "Hello, world!");
    }
}
