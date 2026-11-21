//! コピー専用のUIA／ローカルOCR。翻訳、外部API、画像ログは使用しない。
use crate::{capture::Captured, config::Config};
use windows::{
    Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Security::Cryptography::CryptographicBuffer,
    Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::*,
        UI::WindowsAndMessaging::*,
    },
};

pub struct Target {
    pub text: String,
    pub rects: Vec<RECT>,
}

pub fn rectangle(a: POINT, b: POINT) -> Option<RECT> {
    let r = RECT {
        left: a.x.min(b.x),
        top: a.y.min(b.y),
        right: a.x.max(b.x),
        bottom: a.y.max(b.y),
    };
    (r.right - r.left >= 3 && r.bottom - r.top >= 3).then_some(r)
}

pub fn contains(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

pub fn recognize(
    point: POINT,
    selection: Option<RECT>,
    cfg: &Config,
    engine: &mut Option<OcrEngine>,
) -> Result<Option<Target>, String> {
    let target = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
    if target.is_invalid() {
        return Ok(None);
    }
    let (exe, _) = crate::util::get_app_context(target);
    if exe.as_deref().is_some_and(|name| cfg.is_disabled_app(name)) {
        return Ok(None);
    }
    if selection.is_none()
        && !exe
            .as_deref()
            .is_some_and(|name| cfg.is_ocr_priority_app(name))
    {
        if let Some((text, rects)) = crate::uia::copy_text_at_point(point.x, point.y) {
            return Ok(Some(Target { text, rects }));
        }
    }
    let screen = screen_bounds();
    let rect = selection.unwrap_or(RECT {
        left: point.x - 600,
        top: point.y - 120,
        right: point.x + 600,
        bottom: point.y + 120,
    });
    let rect = RECT {
        left: rect.left.max(screen.left),
        top: rect.top.max(screen.top),
        right: rect.right.min(screen.right),
        bottom: rect.bottom.min(screen.bottom),
    };
    let image = screen_image(rect)?;
    if engine.is_none() {
        *engine = Some(OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| e.to_string())?);
    }
    let ocr = engine.as_ref().unwrap();
    let maximum = OcrEngine::MaxImageDimension().unwrap_or(4096);
    let image = crate::capture::downscale_max(&image, maximum);
    let scale_x = (rect.right - rect.left) as f32 / image.width as f32;
    let scale_y = (rect.bottom - rect.top) as f32 / image.height as f32;
    let bytes = CryptographicBuffer::CreateFromByteArray(&image.bgra).map_err(|e| e.to_string())?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &bytes,
        BitmapPixelFormat::Bgra8,
        image.width as i32,
        image.height as i32,
    )
    .map_err(|e| e.to_string())?;
    let result = ocr
        .RecognizeAsync(&bitmap)
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|e| e.to_string())?;
    let lines = result.Lines().map_err(|e| e.to_string())?;
    let mut texts = Vec::new();
    let mut rects = Vec::new();
    for i in 0..lines.Size().unwrap_or(0) {
        let line = lines.GetAt(i).map_err(|e| e.to_string())?;
        let text = line.Text().map_err(|e| e.to_string())?.to_string();
        if text.trim().is_empty() {
            continue;
        }
        let words = line.Words().map_err(|e| e.to_string())?;
        let mut bounds: Option<RECT> = None;
        for j in 0..words.Size().unwrap_or(0) {
            let word = words.GetAt(j).map_err(|e| e.to_string())?;
            let r = word.BoundingRect().map_err(|e| e.to_string())?;
            let r = RECT {
                left: rect.left + (r.X * scale_x).floor() as i32,
                top: rect.top + (r.Y * scale_y).floor() as i32,
                right: rect.left + ((r.X + r.Width) * scale_x).ceil() as i32,
                bottom: rect.top + ((r.Y + r.Height) * scale_y).ceil() as i32,
            };
            bounds = Some(match bounds {
                None => r,
                Some(b) => RECT {
                    left: b.left.min(r.left),
                    top: b.top.min(r.top),
                    right: b.right.max(r.right),
                    bottom: b.bottom.max(r.bottom),
                },
            });
        }
        if let Some(bounds) = bounds {
            if selection.is_some() || contains(&bounds, point) {
                texts.push(text);
                rects.push(bounds);
            }
        }
    }
    if texts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Target {
            text: texts.join("\r\n"),
            rects,
        }))
    }
}

fn screen_bounds() -> RECT {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// 小さな矩形を直接取得する。既存の翻訳用WGC経路のフォールバックは変更しない。
fn screen_image(rect: RECT) -> Result<Captured, String> {
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 || i64::from(w) * i64::from(h) > 64_000_000 {
        return Err("キャプチャ範囲が不正です".into());
    }
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("画面を取得できません".into());
        }
        let memory = CreateCompatibleDC(Some(screen));
        if memory.is_invalid() {
            let _ = ReleaseDC(None, screen);
            return Err("画面の取得領域を作成できません".into());
        }
        let info = BITMAPINFO {
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
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(memory);
                let _ = ReleaseDC(None, screen);
                return Err(e.to_string());
            }
        };
        let old = SelectObject(memory, HGDIOBJ(bitmap.0));
        let copied = BitBlt(
            memory,
            0,
            0,
            w,
            h,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY,
        );
        let result = if copied.is_ok() && !bits.is_null() {
            Ok(Captured {
                width: w as u32,
                height: h as u32,
                bgra: std::slice::from_raw_parts(bits as *const u8, w as usize * h as usize * 4)
                    .to_vec(),
            })
        } else {
            Err("画面を取得できません".into())
        };
        let _ = SelectObject(memory, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        let _ = ReleaseDC(None, screen);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_capture_reverse_drag_and_negative_monitor_coordinates() {
        let r = rectangle(POINT { x: 30, y: 20 }, POINT { x: -100, y: -80 }).unwrap();
        assert_eq!((r.left, r.top, r.right, r.bottom), (-100, -80, 30, 20));
        assert!(contains(&r, POINT { x: -50, y: 0 }));
        assert!(!contains(&r, POINT { x: 30, y: 0 }));
        assert!(rectangle(POINT { x: 0, y: 0 }, POINT { x: 1, y: 100 }).is_none());
    }
}
