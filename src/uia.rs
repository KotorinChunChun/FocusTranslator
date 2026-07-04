// UIA テキスト取得経路 (SPEC §6.1)
// ElementFromPoint → TextPattern → RangeFromPoint → 行単位に拡張 → GetText
// TextPattern が無ければ祖先を数段探索。取得不可なら None を返し OCR 経路へ。
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, TextUnit_Line, UIA_TextPatternId,
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
            if let Ok(unk) = e.GetCurrentPattern(UIA_TextPatternId)
                && let Ok(tp) = unk.cast::<IUIAutomationTextPattern>()
                    && let Ok(range) = tp.RangeFromPoint(pt) {
                        let _ = range.ExpandToEnclosingUnit(TextUnit_Line);
                        if let Ok(text) = range.GetText(512) {
                            let s = text.to_string();
                            let line = first_meaningful_line(&s);
                            if !line.is_empty() {
                                return Some(line);
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
