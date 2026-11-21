//! 対象キー押下中のフォーカス表示と、解放時のコピー。
//! フック内では状態更新と通知だけを行い、UIA/OCRは単一ワーカーで処理する。
use crate::{
    config::Config,
    copy_recognition::{self, Target},
    detect,
};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::*,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub const WM_KEY: u32 = WM_APP + 60;
pub const WM_MOUSE: u32 = WM_APP + 61;
const TIMER: usize = 60;
static MAIN: AtomicIsize = AtomicIsize::new(0);
static KEY: AtomicI32 = AtomicI32::new(0);
static HELD: AtomicI32 = AtomicI32::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LEFT_DOWN: AtomicBool = AtomicBool::new(false);
static ESC_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Ticket {
    session: u64,
    revision: u64,
    final_copy: bool,
}
#[derive(Default)]
struct Flow {
    session: u64,
    revision: u64,
    holding: bool,
}
impl Flow {
    fn begin(&mut self) {
        self.session += 1;
        self.revision = 0;
        self.holding = true;
    }
    fn changed(&mut self) {
        self.revision += 1;
    }
    fn release(&mut self) {
        self.changed();
        self.holding = false;
    }
    fn cancel(&mut self) {
        self.session += 1;
        self.holding = false;
    }
    fn ticket(&self) -> Ticket {
        Ticket {
            session: self.session,
            revision: self.revision,
            final_copy: !self.holding,
        }
    }
    fn accepts(&self, ticket: Ticket) -> bool {
        ticket == self.ticket()
    }
}

struct Job {
    ticket: Ticket,
    point: POINT,
    selection: Option<RECT>,
    cfg: Config,
}
struct Answer {
    ticket: Ticket,
    target: Result<Option<Target>, String>,
}
struct MouseEvent {
    down: bool,
    point: POINT,
}
struct Controller {
    flow: Flow,
    instance: HINSTANCE,
    main: HWND,
    keyboard: HHOOK,
    mouse: HHOOK,
    jobs: mpsc::Sender<Job>,
    answers: mpsc::Receiver<Answer>,
    busy: bool,
    awaiting_final: bool,
    desired: Option<Job>,
    point: POINT,
    drag_start: Option<POINT>,
    selection: Option<RECT>,
    last_probe: Instant,
    clipboard: Option<(String, Instant)>,
}
thread_local! {static STATE:RefCell<Option<Controller>>=const{RefCell::new(None)};}

pub fn configure(cfg: &Config, instance: HINSTANCE, main: HWND) {
    cancel();
    KEY.store(cfg.text_capture_vk(), Ordering::Release);
    MAIN.store(main.0 as isize, Ordering::Release);
    STATE.with(|state| {
        if state.borrow().is_some() {
            return;
        }
        let (jobs, incoming) = mpsc::channel::<Job>();
        let (outgoing, answers) = mpsc::channel();
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                );
            }
            let mut engine = None;
            while let Ok(job) = incoming.recv() {
                let target =
                    copy_recognition::recognize(job.point, job.selection, &job.cfg, &mut engine);
                if outgoing
                    .send(Answer {
                        ticket: job.ticket,
                        target,
                    })
                    .is_err()
                {
                    break;
                }
            }
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        });
        let keyboard =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance), 0) };
        let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0) };
        match (keyboard, mouse) {
            (Ok(keyboard), Ok(mouse)) => {
                *state.borrow_mut() = Some(Controller {
                    flow: Flow::default(),
                    instance,
                    main,
                    keyboard,
                    mouse,
                    jobs,
                    answers,
                    busy: false,
                    awaiting_final: false,
                    desired: None,
                    point: POINT::default(),
                    drag_start: None,
                    selection: None,
                    last_probe: Instant::now(),
                    clipboard: None,
                });
                unsafe {
                    SetTimer(Some(main), TIMER, 30, None);
                }
            }
            (keyboard, mouse) => {
                unsafe {
                    if let Ok(h) = keyboard {
                        let _ = UnhookWindowsHookEx(h);
                    }
                    if let Ok(h) = mouse {
                        let _ = UnhookWindowsHookEx(h);
                    }
                }
                KEY.store(0, Ordering::Release);
                crate::util::app_log("テキストキャプチャの入力フックを登録できませんでした。");
            }
        }
    });
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = event.vkCode as i32;
        let up = event.flags.contains(LLKHF_UP);
        if vk == KEY.load(Ordering::Acquire) && vk != 0
            || vk == HELD.load(Ordering::Acquire) && vk != 0
        {
            if up {
                if HELD.swap(0, Ordering::AcqRel) != 0 {
                    ACTIVE.store(false, Ordering::Release);
                    post_key(0);
                }
            } else if HELD
                .compare_exchange(0, vk, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                ACTIVE.store(true, Ordering::Release);
                post_key(1);
            }
            return LRESULT(1);
        }
        if vk == VK_ESCAPE.0 as i32
            && (ACTIVE.load(Ordering::Acquire) || ESC_DOWN.load(Ordering::Acquire))
        {
            ESC_DOWN.store(!up, Ordering::Release);
            if !up {
                ACTIVE.store(false, Ordering::Release);
                post_key(2);
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
fn post_key(kind: usize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(MAIN.load(Ordering::Acquire) as *mut _)),
            WM_KEY,
            WPARAM(kind),
            LPARAM(0),
        );
    }
}
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let down = wparam.0 as u32 == WM_LBUTTONDOWN;
        let up = wparam.0 as u32 == WM_LBUTTONUP;
        if down && ACTIVE.load(Ordering::Acquire) || up && LEFT_DOWN.load(Ordering::Acquire) {
            LEFT_DOWN.store(down, Ordering::Release);
            let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let event = Box::into_raw(Box::new(MouseEvent {
                down,
                point: event.pt,
            }));
            if unsafe {
                PostMessageW(
                    Some(HWND(MAIN.load(Ordering::Acquire) as *mut _)),
                    WM_MOUSE,
                    WPARAM(0),
                    LPARAM(event as isize),
                )
            }
            .is_err()
            {
                unsafe {
                    drop(Box::from_raw(event));
                }
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn cursor() -> Option<POINT> {
    let mut p = POINT::default();
    unsafe {
        GetCursorPos(&mut p).ok()?;
    }
    Some(p)
}

pub fn handle_key(kind: usize, cfg: &Config) {
    if kind == 2 {
        cancel();
        return;
    }
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(st) = state.as_mut() else {
            return;
        };
        if kind == 1 {
            st.flow.begin();
            st.selection = None;
            st.drag_start = None;
            st.clipboard = None;
            st.awaiting_final = false;
            st.desired = None;
            if let Some(point) = cursor() {
                st.point = point;
                detect::hide();
                detect::show(st.instance);
                st.request(point, None, cfg);
            } else {
                st.cancel();
            }
        } else if st.flow.holding {
            if let Some(point) = cursor() {
                if let Some(start) = st.drag_start.filter(|_| LEFT_DOWN.load(Ordering::Acquire)) {
                    st.selection = copy_recognition::rectangle(start, point);
                }
                let had_drag = st.drag_start.is_some();
                st.flow.release();
                st.awaiting_final = true;
                st.clipboard = None;
                detect::hide();
                // 小さすぎるドラッグを点取得へ戻して誤コピーしない。
                if had_drag && st.selection.is_none() {
                    st.cancel();
                    return;
                }
                st.request(point, st.selection, cfg);
            } else {
                st.cancel();
            }
        }
    });
}
pub fn handle_mouse(lparam: LPARAM) {
    let event = unsafe { Box::from_raw(lparam.0 as *mut MouseEvent) };
    STATE.with(|state| {
        if let Some(st) = state.borrow_mut().as_mut() {
            if !st.flow.holding {
                return;
            }
            st.flow.changed();
            st.desired = None;
            if event.down {
                st.drag_start = Some(event.point);
                st.selection = None;
            } else if let Some(start) = st.drag_start {
                st.selection = copy_recognition::rectangle(start, event.point);
            }
            st.paint(st.selection.into_iter().collect());
        }
    });
}
pub fn tick(cfg: &Config) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(st) = state.as_mut() else {
            return;
        };
        while let Ok(answer) = st.answers.try_recv() {
            st.busy = false;
            if !st.flow.accepts(answer.ticket) {
                continue;
            }
            if answer.ticket.final_copy {
                st.awaiting_final = false;
            }
            match answer.target {
                Ok(Some(target)) if !target.text.trim().is_empty() => {
                    if answer.ticket.final_copy {
                        st.clipboard =
                            Some((target.text, Instant::now() + Duration::from_millis(600)));
                    } else {
                        st.paint(target.rects);
                    }
                }
                Ok(_) => {
                    if st.flow.holding {
                        st.paint(Vec::new());
                    }
                }
                Err(error) => {
                    if st.flow.holding {
                        st.paint(Vec::new());
                    }
                    crate::util::app_log(&format!("テキストキャプチャ: {error}"));
                }
            }
        }
        if let Some((text, deadline)) = &st.clipboard {
            if crate::util::set_clipboard_text(st.main, text) || Instant::now() >= *deadline {
                st.clipboard = None;
            }
        }
        if st.flow.holding {
            if let Some(point) = cursor() {
                if let Some(start) = st.drag_start {
                    // 左ボタン解放後は最後の選択矩形を維持する。
                    if LEFT_DOWN.load(Ordering::Acquire) {
                        st.selection = copy_recognition::rectangle(start, point);
                        st.paint(st.selection.into_iter().collect());
                    }
                } else if point.x != st.point.x || point.y != st.point.y {
                    st.flow.changed();
                    st.point = point;
                    st.paint(Vec::new());
                    st.request(point, None, cfg);
                } else if st.desired.is_none()
                    && !st.busy
                    && st.last_probe.elapsed() >= Duration::from_millis(240)
                {
                    st.flow.changed();
                    st.request(point, None, cfg);
                }
            } else {
                st.cancel();
            }
        }
        if !st.busy {
            if let Some(job) = st.desired.take() {
                st.last_probe = Instant::now();
                if st.jobs.send(job).is_ok() {
                    st.busy = true;
                }
            }
        }
    });
}
impl Controller {
    fn request(&mut self, point: POINT, selection: Option<RECT>, cfg: &Config) {
        let point = selection
            .map(|r| POINT {
                x: r.left + (r.right - r.left) / 2,
                y: r.top + (r.bottom - r.top) / 2,
            })
            .unwrap_or(point);
        self.desired = Some(Job {
            ticket: self.flow.ticket(),
            point,
            selection,
            cfg: cfg.clone(),
        });
    }
    fn paint(&self, rects: Vec<RECT>) {
        detect::update(detect::DetectInfo {
            cursor: self.point,
            window_rect: None,
            band_rect: None,
            uia_element: None,
            uia_lines: rects,
            hover_rect: None,
            label: String::new(),
        });
    }
    fn cancel(&mut self) {
        self.flow.cancel();
        self.desired = None;
        self.clipboard = None;
        self.awaiting_final = false;
        self.selection = None;
        self.drag_start = None;
        detect::hide();
    }
}
pub fn active() -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|s| {
            s.flow.holding || s.awaiting_final || s.desired.is_some() || s.clipboard.is_some()
        })
    })
}
pub fn cancel() {
    ACTIVE.store(false, Ordering::Release);
    STATE.with(|state| {
        if let Some(st) = state.borrow_mut().as_mut() {
            st.cancel();
        }
    });
}
pub fn shutdown() {
    cancel();
    KEY.store(0, Ordering::Release);
    STATE.with(|state| {
        if let Some(st) = state.borrow_mut().take() {
            unsafe {
                let _ = UnhookWindowsHookEx(st.keyboard);
                let _ = UnhookWindowsHookEx(st.mouse);
                let _ = KillTimer(Some(st.main), TIMER);
            }
        }
    });
}

pub fn is_timer(id: usize) -> bool {
    id == TIMER
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_hover_cannot_become_release_copy() {
        let mut f = Flow::default();
        f.begin();
        let hover = f.ticket();
        f.changed();
        let moved = f.ticket();
        f.release();
        let released = f.ticket();
        assert!(!f.accepts(hover));
        assert!(!f.accepts(moved));
        assert!(f.accepts(released));
        assert!(released.final_copy);
    }
    #[test]
    fn cancel_and_next_capture_reject_late_results() {
        let mut f = Flow::default();
        f.begin();
        f.release();
        let old = f.ticket();
        f.cancel();
        assert!(!f.accepts(old));
        f.begin();
        assert!(!f.accepts(old));
        assert!(!f.ticket().final_copy);
    }
    #[test]
    fn same_point_is_revalidated_on_release() {
        let mut f = Flow::default();
        f.begin();
        let before = f.ticket();
        f.release();
        assert_ne!(before, f.ticket());
    }
}
