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
    IUIAutomationTextRange, TextUnit_Line, UIA_TextPatternId,
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
        if let Some(hit) = find_text_hit(&auto, el, pt) {
            p.element_rect = hit.element.CurrentBoundingRectangle().ok();
            p.node = element_node_name(&hit.element);
            if let Some(r) = &hit.range {
                p.line_rects = range_rects(r);
            }
            p.text = Some(hit.text);
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

/// カーソル位置の要素のUIAパス(親→子順に AutomationId / Name / ControlType を連結)を取得する。
/// 解説機能の同一コンテキスト判定キーに使う (SPEC v0.2 §2.3.1)。
pub fn path_at_point(x: i32, y: i32) -> String {
    unsafe {
        let Ok(auto) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return String::new();
        };
        let Ok(el) = auto.ElementFromPoint(POINT { x, y }) else {
            return String::new();
        };
        let Ok(walker) = auto.ControlViewWalker() else {
            return String::new();
        };
        let mut path = Vec::new();
        let mut cur = Some(el);
        for _ in 0..5 {
            let Some(e) = cur.clone() else { break };
            let name = element_node_name(&e);
            if !name.is_empty() {
                path.push(name);
            }
            cur = walker.GetParentElement(&e).ok();
        }
        path.reverse();
        path.join(" > ")
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
