use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

/// プライマリースレッドを止めたままジョブに登録し、CLIが子を先に起動する競合を防ぐ。
pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, OwnedHandle)> {
    let job = unsafe {
        let handle = CreateJobObjectW(None, None)?;
        let job = OwnedHandle::from_raw_handle(handle.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
        job
    };

    let mut child = command
        .creation_flags((CREATE_NO_WINDOW | CREATE_SUSPENDED).0)
        .spawn()?;
    if let Err(error) = assign_and_resume(&child, &job) {
        drop(job);
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok((child, job))
}

fn assign_and_resume(child: &Child, job: &OwnedHandle) -> io::Result<()> {
    unsafe {
        AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle()))?;
        // std::process::Childは初期スレッドのハンドルを公開していない。
        // サスペンド状態の対象PIDに属する唯一のスレッドを取得する。
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let _guard = OwnedHandle::from_raw_handle(snapshot.0);
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        Thread32First(snapshot, &mut entry)?;
        loop {
            if entry.th32OwnerProcessID == child.id() {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)?;
                let _thread = OwnedHandle::from_raw_handle(thread.0);
                if ResumeThread(thread) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            Thread32Next(snapshot, &mut entry)?;
        }
    }
}
