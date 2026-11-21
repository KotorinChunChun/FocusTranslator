//! KotorinToys管理契約2。取得文字列や個人の設定内容を応答に含めない。
use crate::{management_pipe::ManagementPipe, window_title::APP_VERSION};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
pub const TOOL_ID: &str = "focus-translator";

/// 試験用の明示設定だけを、利用者の常駐インスタンスと分離する。
pub fn tool_id() -> String {
    use sha2::{Digest, Sha256};
    match settings_directory() {
        Some(path) => format!("{TOOL_ID}-{:x}", Sha256::digest(path.as_bytes()))
            [..TOOL_ID.len() + 17]
            .to_string(),
        None => TOOL_ID.into(),
    }
}

pub fn settings_directory() -> Option<String> {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|a| a[0] == "--settings-dir")
        .map(|a| a[1].clone())
}

pub fn launch_origin() -> String {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|a| a[0] == "--launch-origin")
        .map(|a| a[1].as_str())
        .filter(|a| matches!(*a, "host_manual" | "direct_task" | "via_kotorintoys"))
        .unwrap_or("standalone")
        .into()
}

pub fn window_class() -> String {
    format!("FocusTranslatorMain.{}", tool_id())
}

pub fn passive() -> bool {
    std::env::args().any(|arg| arg == "--passive")
}

pub fn hidden_startup() -> bool {
    std::env::args().any(|arg| matches!(arg.as_str(), "--startup" | "--hidden"))
}

pub fn show_existing() {
    use windows::{
        Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_COMMAND},
        },
        core::PCWSTR,
    };
    let class = crate::util::to_wide(&window_class());
    unsafe {
        let hwnd = FindWindowW(PCWSTR(class.as_ptr()), None).or_else(|error| {
            if settings_directory().is_none() {
                FindWindowW(windows::core::w!("FocusTranslatorMain"), None)
            } else {
                Err(error)
            }
        });
        if let Ok(hwnd) = hwnd {
            let _ = PostMessageW(
                Some(hwnd),
                WM_COMMAND,
                WPARAM(crate::tray::CMD_SETTINGS),
                LPARAM(0),
            );
        }
    }
}
#[derive(Default, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub command: String,
    #[serde(default)]
    pub expected_pid: Option<u32>,
    #[serde(default)]
    pub request_id: String,
}
pub fn parse(line: &str) -> Result<Request, serde_json::Error> {
    if line == "status" {
        Ok(Request {
            command: "status".into(),
            ..Default::default()
        })
    } else {
        serde_json::from_str(line)
    }
}
pub fn validate(request: &Request, blocked: bool) -> (&'static str, &'static str) {
    if request
        .expected_pid
        .is_some_and(|p| p != std::process::id())
    {
        return ("refused", "対象PIDが変わりました。");
    }
    if request.request_id.len() > 128 {
        return ("refused", "要求IDが長すぎます。");
    }
    match request.command.as_str() {
        "show" | "stop" if request.expected_pid.is_none() => {
            ("refused", "操作には対象PIDが必要です。")
        }
        "status" | "show" => ("completed", ""),
        "stop" if blocked => (
            "refused",
            "設定画面を閉じ、適用処理の完了を確認してから停止してください。",
        ),
        "stop" => ("accepted", ""),
        _ => ("unsupported", "未対応の操作です。"),
    }
}
pub struct Call {
    pub request: Request,
    pub deadline: Instant,
    reply: SyncSender<Value>,
}
impl Call {
    pub fn respond(self, result: &str, detail: &str, origin: &str) {
        if Instant::now() >= self.deadline {
            return;
        }
        let _ = self.reply.send(
            json!({"protocolVersion":2,"requestId":self.request.request_id,
            "pid":std::process::id(),"ready":true,"version":APP_VERSION,"launchOrigin":origin,
            "capabilities":["status","show","stop"],"result":result,"detail":detail}),
        );
    }
}
pub enum Event {
    Request(Call),
    Stop(Instant),
}
pub struct Endpoint {
    incoming: Receiver<Event>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Endpoint {
    pub fn start() -> Result<Self, String> {
        let (sender, incoming) = mpsc::channel();
        let (ready, ready_rx) = mpsc::sync_channel(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let ending = shutdown.clone();
        let worker = std::thread::Builder::new()
            .name("focus-management".into())
            .spawn(move || {
                let pipe = match ManagementPipe::create(&tool_id()) {
                    Ok(pipe) => {
                        let _ = ready.send(Ok(()));
                        pipe
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e.to_string()));
                        return;
                    }
                };
                while !ending.load(Ordering::Acquire) {
                    if pipe.accept(Duration::from_millis(250)).is_err() {
                        continue;
                    }
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let operation = (|| -> Option<bool> {
                        if !pipe.same_session().ok()? {
                            return None;
                        }
                        let request = parse(&pipe.read_line(deadline).ok()?).ok()?;
                        let (reply, receive) = mpsc::sync_channel(1);
                        sender
                            .send(Event::Request(Call {
                                request,
                                deadline,
                                reply,
                            }))
                            .ok()?;
                        let response = receive
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            .ok()?;
                        let accepted = response["result"] == "accepted";
                        pipe.write_line(&response.to_string(), deadline).ok()?;
                        Some(accepted)
                    })();
                    pipe.wait_for_client_close(deadline);
                    pipe.disconnect();
                    if operation == Some(true) {
                        let _ = sender.send(Event::Stop(deadline));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                incoming,
                shutdown,
                worker: Some(worker),
            }),
            other => {
                shutdown.store(true, Ordering::Release);
                let _ = worker.join();
                Err(format!("管理パイプを開始できません: {other:?}"))
            }
        }
    }
    pub fn poll(&self) -> Vec<Event> {
        self.incoming.try_iter().collect()
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutations_require_pid_and_stop_protects_editor() {
        let mut req = Request {
            command: "stop".into(),
            expected_pid: Some(std::process::id()),
            ..Default::default()
        };
        assert_eq!(validate(&req, false).0, "accepted");
        assert_eq!(validate(&req, true).0, "refused");
        req.expected_pid = None;
        assert_eq!(validate(&req, false).0, "refused");
        req.expected_pid = Some(std::process::id() + 1);
        assert_eq!(validate(&req, false).0, "refused");
        req.command = "show".into();
        assert_eq!(validate(&req, false).0, "refused");
        req.expected_pid = Some(std::process::id());
        assert_eq!(validate(&req, false).0, "completed");
        req.request_id = "x".repeat(129);
        assert_eq!(validate(&req, false).0, "refused");
    }
    #[test]
    fn status_and_json_contracts_are_bounded_and_unknown_actions_unsupported() {
        assert_eq!(parse("status").unwrap().command, "status");
        let req = parse(r#"{"command":"shortcuts","requestId":"a"}"#).unwrap();
        assert_eq!(validate(&req, false).0, "unsupported");
        assert!(parse("{").is_err());
    }
}
