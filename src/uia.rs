// UIA テキスト取得経路 (SPEC §6.1)
// ElementFromPoint → TextPattern → RangeFromPoint → 行単位に拡張 → GetText
// TextPattern が無ければ祖先を数段探索。取得不可なら text=None を返し OCR 経路へ。
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextRange, TextUnit_Line, TextUnit_Paragraph, UIA_TextPatternId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId,
};
use windows::core::Interface;

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

            if let Ok(unk) = e.GetCurrentPattern(UIA_TextPatternId)
                && let Ok(tp) = unk.cast::<IUIAutomationTextPattern>() {

                    let ctrl_type = e.CurrentControlType().unwrap_or(windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID(0));
                    let is_edit = ctrl_type == UIA_EditControlTypeId || ctrl_type == UIA_DocumentControlTypeId;

                    if is_edit {
                        if let Ok(doc_range) = tp.DocumentRange() {
                            if let Ok(full_text) = doc_range.GetText(-1) {
                                let full_str = full_text.to_string();
                                let is_multiline = full_str.contains('\n') || full_str.contains('\r');
                                if !is_multiline {
                                    let line = full_str.trim().to_string();
                                    if !line.is_empty() {
                                        return Some(TextHit { element: e, range: Some(doc_range), text: line });
                                    }
                                }
                            }
                        }
                    }

                    if let Ok(range) = tp.RangeFromPoint(pt) {
                        // まず段落単位で拡張する: 右端で折り返された文も改行を跨いで
                        // 1つの段落として正確に取れる (ユーザー要望)。
                        // 取れない・長すぎる場合は従来の行単位へフォールバック。
                        if let Ok(para) = range.Clone() {
                            let _ = para.ExpandToEnclosingUnit(TextUnit_Paragraph);
                            if let Ok(t) = para.GetText(1200) {
                                let s = t.to_string();
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
                            let s = text.to_string();
                            let line = first_meaningful_line(&s);
                            if !line.is_empty() {
                                return Some(TextHit { element: e, range: Some(range), text: line });
                            }
                        }
                    }
                }
            cur = walker.GetParentElement(&e).ok();
        }
        None
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
            p.text = Some(hit.text);
        } else if let Ok(name) = el.CurrentName() {
            // TextPattern なし: Name に要素の全テキストが入っていれば段落復元に使う
            let s = name.to_string();
            let t = s.trim();
            if t.chars().count() >= 8 && t.chars().count() <= 10000 {
                p.hover_text = Some(t.to_string());
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    /// 祖先要素 (AutomationId/Name/ControlType のパス階層)
    Ancestor,
    /// カーソル直下の末端要素の子孫テキストをすべて連結した合成ノード
    ChildrenConcat,
}

/// UIAパスの1ノード。ボタン化・クリック時のテキスト採用・ログ記録に使う。
#[derive(Clone, Debug)]
pub struct UiaPathNode {
    /// ボタン表示用の短い識別ラベル (AutomationId/Name/ControlType)。
    /// text が空の場合のフォールバック表示にも使う。
    pub label: String,
    /// このノードから抽出したテキスト (クリック時に原文として採用する全文)
    pub text: String,
    pub kind: NodeKind,
}

/// 子孫走査の上限(パフォーマンスと無関係テキスト混入の抑制のため)
const DESC_MAX_DEPTH: u32 = 6;
const DESC_MAX_NODES: usize = 200;
const DESC_MAX_CHARS: usize = 4000;

/// カーソル位置の要素のUIAパスノード列を取得する。
/// 祖先方向に最大5段(AutomationId/Name/ControlTypeの短い識別子 + そのノード自身から
/// 抽出したテキスト)を積み、末尾にカーソル直下の末端要素の子孫テキストを再帰的に
/// 走査して1行ずつ連結した合成ノードを追加する (ボタン化してOCRの代わりに採用するため)。
/// 解説機能の同一コンテキスト判定キーや認識ログにも使う (SPEC v0.2 §2.3.1)。
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
                path.push(UiaPathNode { label: name, text, kind: NodeKind::Ancestor });
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
            });
        }
        path
    }
}

/// 要素自身のテキストを抽出する。TextPattern があれば全文、無ければ Name を使う。
fn element_own_text(e: &IUIAutomationElement) -> String {
    unsafe {
        if let Ok(unk) = e.GetCurrentPattern(UIA_TextPatternId)
            && let Ok(tp) = unk.cast::<IUIAutomationTextPattern>()
            && let Ok(doc) = tp.DocumentRange()
            && let Ok(t) = doc.GetText(4000) {
                let s = t.to_string();
                let s = s.trim();
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        e.CurrentName().map(|n| n.to_string().trim().to_string()).unwrap_or_default()
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
