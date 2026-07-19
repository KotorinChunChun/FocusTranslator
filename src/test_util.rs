// テスト専用ユーティリティ (paddle_ocr / oneocr のスモークテストで共用)
use crate::capture::Captured;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DIB_RGB_COLORS, DT_NOPREFIX, DT_SINGLELINE, DeleteDC, DeleteObject, DrawTextW, FillRect, GetDC, HGDIOBJ, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};

/// GDIでテキストを白背景に黒文字で描画したBGRA画像を作る(実推論の疎通確認用)
pub fn render_text(text: &str, w: i32, h: i32) -> Captured {
    unsafe {
        let screen_dc = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen_dc));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("DIBセクションの作成に失敗しました");
        let old = SelectObject(mem, HGDIOBJ(bmp.0));

        let rect = RECT { left: 0, top: 0, right: w, bottom: h };
        let bg = CreateSolidBrush(COLORREF(0x00FFFFFF));
        FillRect(mem, &rect, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        SetBkMode(mem, TRANSPARENT);
        SetTextColor(mem, COLORREF(0x00000000));
        let font = crate::ui_helpers::make_font(28, false);
        let old_font = SelectObject(mem, HGDIOBJ(font.0));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut r = rect;
        DrawTextW(mem, &mut wide, &mut r, DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(mem, old_font);
        let _ = DeleteObject(HGDIOBJ(font.0));

        SelectObject(mem, old);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen_dc);

        let len = (w * h * 4) as usize;
        let bgra = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        Captured { width: w as u32, height: h as u32, bgra }
    }
}
