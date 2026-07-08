// UIA テキスト取得経路 (SPEC §6.1)
// ElementFromPoint → TextPattern → RangeFromPoint → 行単位に拡張 → GetText
// TextPattern が無ければ祖先を数段探索。取得不可なら None を返し OCR 経路へ。
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, TextUnit_Line, UIA_TextPatternId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId,
};
use windows::core::Interface;

/// カーソル位置の1行テキストを UIA で取得する。呼び出し元スレッドは COM 初期化済みであること。
pub fn line_at_point(x: i32, y: i32) -> Option<String> {
    unsafe {
        let auto: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let pt = POINT { x, y };
        let el = auto.ElementFromPoint(pt).ok()?;
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
                                        return Some(line);
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
                                return Some(line);
                            }
                        }
                    }
                }
            cur = walker.GetParentElement(&e).ok();
        }
        None
    }
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
