//! 本体管理用の利用者専用・ローカル限定・時間制限付きバイトストリーム。

use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject,
};
use windows::core::{HRESULT, PCWSTR, PWSTR};

pub const MAX_REQUEST_BYTES: usize = 8192;

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct LocalMemory(HLOCAL);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(self.0)) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub fn current_user_sid() -> windows::core::Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;
    let _token = OwnedHandle(token);
    let mut length = 0;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut length) };
    if length == 0 {
        return Err(windows::core::Error::from_thread());
    }
    // TOKEN_USERへのcastに必要なアライメントを確保する。
    let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        )
    }?;
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }?;
    let _sid = LocalMemory(HLOCAL(sid.0.cast()));
    Ok(unsafe { sid.to_string()? })
}

/// 所有者も明示し、.NETのCurrentUserOnlyクライアントと相互運用する。
pub struct ManagementPipe {
    handle: OwnedHandle,
}

impl ManagementPipe {
    pub fn create(id: &str) -> windows::core::Result<Self> {
        let sid = current_user_sid()?;
        let name = wide(&format!(r"\\.\pipe\KotorinToys.{sid}.{id}"));
        let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }?;
        let _descriptor = LocalMemory(HLOCAL(descriptor.0));
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                MAX_REQUEST_BYTES as u32,
                MAX_REQUEST_BYTES as u32,
                0,
                Some(&security),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(windows::core::Error::from_thread());
        }
        Ok(Self {
            handle: OwnedHandle(handle),
        })
    }

    /// 未接続時も定期的に呼出し側へ戻り、終了を阻害しない。
    pub fn accept(&self, timeout: Duration) -> windows::core::Result<()> {
        let event = OwnedHandle(unsafe { CreateEventW(None, true, false, None) }?);
        let mut operation = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let result = unsafe { ConnectNamedPipe(self.handle.0, Some(&mut operation)) };
        if result
            .as_ref()
            .is_err_and(|e| e.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0))
        {
            return Ok(());
        }
        self.finish(result, &mut operation, timeout).map(|_| ())
    }

    pub fn same_session(&self) -> windows::core::Result<bool> {
        let mut client = 0;
        unsafe { GetNamedPipeClientProcessId(self.handle.0, &mut client) }?;
        let mut client_session = 0;
        let mut own_session = 0;
        unsafe {
            ProcessIdToSessionId(client, &mut client_session)?;
            ProcessIdToSessionId(std::process::id(), &mut own_session)?;
        }
        Ok(client_session == own_session)
    }

    pub fn read_line(&self, deadline: Instant) -> windows::core::Result<String> {
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0u8; 512];
            let event = OwnedHandle(unsafe { CreateEventW(None, true, false, None) }?);
            let mut operation = OVERLAPPED {
                hEvent: event.0,
                ..Default::default()
            };
            let result =
                unsafe { ReadFile(self.handle.0, Some(&mut buffer), None, Some(&mut operation)) };
            let count = self.finish(
                result,
                &mut operation,
                deadline.saturating_duration_since(Instant::now()),
            )? as usize;
            if count == 0 {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x8007000Du32 as i32,
                )));
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > MAX_REQUEST_BYTES {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x8007000Du32 as i32,
                )));
            }
            if let Some(end) = bytes.iter().position(|b| *b == b'\n') {
                return String::from_utf8(bytes[..end].to_vec())
                    .map(|s| {
                        s.trim_end_matches('\r')
                            .trim_start_matches('\u{feff}')
                            .to_owned()
                    })
                    .map_err(|_| {
                        windows::core::Error::from_hresult(windows::core::HRESULT(
                            0x8007000Du32 as i32,
                        ))
                    });
            }
        }
    }

    pub fn write_line(&self, line: &str, deadline: Instant) -> windows::core::Result<()> {
        let bytes = format!("{line}\n").into_bytes();
        let mut written = 0;
        while written < bytes.len() {
            let event = OwnedHandle(unsafe { CreateEventW(None, true, false, None) }?);
            let mut operation = OVERLAPPED {
                hEvent: event.0,
                ..Default::default()
            };
            let result = unsafe {
                WriteFile(
                    self.handle.0,
                    Some(&bytes[written..]),
                    None,
                    Some(&mut operation),
                )
            };
            let count = self.finish(
                result,
                &mut operation,
                deadline.saturating_duration_since(Instant::now()),
            )?;
            if count == 0 {
                return Err(windows::core::Error::from_thread());
            }
            written += count as usize;
        }
        Ok(())
    }

    // DisconnectNamedPipeで未読応答を失わないよう、クライアントの切断を短時間待つ。
    pub fn wait_for_client_close(&self, deadline: Instant) {
        while Instant::now() < deadline {
            if unsafe { PeekNamedPipe(self.handle.0, None, 0, None, None, None) }.is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn disconnect(&self) {
        let _ = unsafe { DisconnectNamedPipe(self.handle.0) };
    }

    fn finish(
        &self,
        started: windows::core::Result<()>,
        operation: &mut OVERLAPPED,
        timeout: Duration,
    ) -> windows::core::Result<u32> {
        if let Err(error) = started {
            if error.code() != HRESULT::from_win32(ERROR_IO_PENDING.0) {
                return Err(error);
            }
            let wait = unsafe {
                WaitForSingleObject(
                    operation.hEvent,
                    timeout.as_millis().min(u32::MAX as u128) as u32,
                )
            };
            if wait != WAIT_OBJECT_0 {
                // バッファ／OVERLAPPEDを破棄する前にキャンセル完了まで回収する。
                let _ = unsafe { CancelIoEx(self.handle.0, Some(operation)) };
                let mut ignored = 0;
                let _ =
                    unsafe { GetOverlappedResult(self.handle.0, operation, &mut ignored, true) };
                return Err(windows::core::Error::from_hresult(HRESULT(
                    0x800705B4u32 as i32,
                )));
            }
        }
        let mut count = 0;
        unsafe { GetOverlappedResult(self.handle.0, operation, &mut count, false) }?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn current_user_pipe_is_exclusive_and_timeout_does_not_block() {
        let id = format!("shortcut-pipe-test-{}", std::process::id());
        let pipe = ManagementPipe::create(&id).expect("利用者専用パイプ");
        assert!(ManagementPipe::create(&id).is_err());
        assert!(current_user_sid().unwrap().starts_with("S-1-"));
        let started = Instant::now();
        assert!(pipe.accept(Duration::from_millis(40)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn byte_stream_accepts_split_utf8_lines_and_reuses_the_same_pipe() {
        let id = format!("shortcut-lines-test-{}", std::process::id());
        let (ready, wait) = std::sync::mpsc::channel();
        let server_id = id.clone();
        let server = std::thread::spawn(move || {
            let pipe = ManagementPipe::create(&server_id).unwrap();
            ready.send(()).unwrap();
            for _ in 0..2 {
                pipe.accept(Duration::from_secs(3)).unwrap();
                assert!(pipe.same_session().unwrap());
                let deadline = Instant::now() + Duration::from_secs(2);
                assert_eq!(pipe.read_line(deadline).unwrap(), "状態");
                pipe.write_line("ok", deadline).unwrap();
                pipe.wait_for_client_close(deadline);
                pipe.disconnect();
            }
        });
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        let name = format!(r"\\.\pipe\KotorinToys.{}.{id}", current_user_sid().unwrap());
        for _ in 0..2 {
            // 直前クライアントの切断がサーバーに届くまでの短い間隔。
            std::thread::sleep(Duration::from_millis(30));
            let mut client = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
                .unwrap();
            let input = "状態\r\n".as_bytes();
            client.write_all(&input[..1]).unwrap();
            client.write_all(&input[1..]).unwrap();
            let mut reply = String::new();
            BufReader::new(&client).read_line(&mut reply).unwrap();
            assert_eq!(reply, "ok\n");
        }
        server.join().unwrap();
    }

    #[test]
    fn connected_client_cannot_hold_a_read_indefinitely() {
        let id = format!("shortcut-idle-test-{}", std::process::id());
        let (ready, wait) = std::sync::mpsc::channel();
        let server_id = id.clone();
        let server = std::thread::spawn(move || {
            let pipe = ManagementPipe::create(&server_id).unwrap();
            ready.send(()).unwrap();
            pipe.accept(Duration::from_secs(3)).unwrap();
            let started = Instant::now();
            assert!(pipe.read_line(started + Duration::from_millis(60)).is_err());
            assert!(started.elapsed() < Duration::from_secs(2));
            pipe.disconnect();
        });
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        let name = format!(r"\\.\pipe\KotorinToys.{}.{id}", current_user_sid().unwrap());
        let _client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .unwrap();
        server.join().unwrap();
    }
}
