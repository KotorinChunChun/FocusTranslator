// ローカルLLMサーバー (llama-server.exe) の起動・停止・状態確認 (SPECv0.5.2追補)
// llama-serverは1プロセスでテキストのみ/画像付きの両リクエストを同一ポートで受け付ける
// (--mmproj を渡した場合のみ画像入力対応になる。テキスト処理には影響しない) ため、
// サーバーは常に1つだけ管理する。このプロセスから起動した子プロセスは CHILD に保持し、
// 停止ボタンで直接終了できる。別セッションや手動で起動されたサーバーは Child を
// 保持していないため、停止時は同名プロセス(llama-server.exe)をイメージ名指定で終了させる
// フォールバックを使う(専用にバンドルしたバイナリのみを対象とするため、単一ユーザー向け
// ツールとして許容する)。
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Child;
use std::sync::Mutex;
use std::time::Duration;

static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// 既定ポート (Ollama等でも広く使われる値。設定で変更可)
pub const DEFAULT_PORT: u32 = 11434;
/// llama.cpp種別プロファイルの既定URL (DEFAULT_PORTと値を合わせておくこと)
pub const DEFAULT_URL: &str = "http://localhost:11434/v1/chat/completions";
/// コンソールウィンドウを作らずに子プロセスを起動するフラグ (Win32 CREATE_NO_WINDOW)
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// llama-server.exeの標準出力/標準エラーを書き出すログファイル。コンソールを隠す代わりに、
/// 何が起きているか追えるようここへリダイレクトする (SPECv0.5.2追補)。
fn server_log_path() -> std::path::PathBuf {
    crate::llama_install::bin_dir().join("server.log")
}

fn health_url(port: u32) -> String {
    format!("http://127.0.0.1:{port}/health")
}

/// 指定ポートでサーバーが応答するか (このプロセスが起動したものか否かを問わない)
pub fn is_running(port: u32) -> bool {
    ureq::get(health_url(port))
        .config()
        .timeout_global(Some(Duration::from_millis(800)))
        .build()
        .call()
        .is_ok()
}

/// サーバーを起動する。既に応答があれば何もしない(重複起動防止)。
/// model には resolve_model_path() で解決した実パスを渡す。mmproj が Some なら
/// --mmproj を併せて渡し、同一ポートのまま画像入力(VLM)にも対応させる。
pub fn start(port: u32, model: &Path, mmproj: Option<&Path>) -> Result<(), String> {
    if is_running(port) {
        return Ok(());
    }
    let exe = crate::llama_install::server_exe_path();
    if !exe.is_file() {
        return Err("llama-server.exeが導入されていません".into());
    }
    if !model.is_file() {
        return Err("モデルファイルが見つかりません".into());
    }
    if let Some(mp) = mmproj
        && !mp.is_file()
    {
        return Err("mmprojファイルが見つかりません".into());
    }

    let log_path = server_log_path();
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let header = format!(
        "\n===== [{ts_ms}] llama-server起動 =====\n実行ファイル: {}\nモデル: {}\nmmproj: {}\nポート: {port}\n============================\n",
        exe.display(),
        model.display(),
        mmproj.map(|p| p.display().to_string()).unwrap_or_else(|| "(なし: テキスト専用)".into()),
    );
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        use std::io::Write;
        let _ = f.write_all(header.as_bytes());
    }
    let stdout_stdio = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null());
    let stderr_stdio = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null());

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("-m")
        .arg(model)
        .arg("--port")
        .arg(port.to_string())
        .arg("--host")
        .arg("127.0.0.1");
    if let Some(mp) = mmproj {
        cmd.arg("--mmproj").arg(mp);
    }
    let child = cmd
        .current_dir(crate::llama_install::bin_dir())
        .stdin(std::process::Stdio::null())
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        // GUIアプリからコンソールサブシステムの子プロセスを起動すると新しいコンソール
        // ウィンドウが表示されてしまうため、非表示で起動する (SPECv0.5.2追補)。
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("サーバーの起動に失敗しました: {e}"))?;
    crate::util::app_log(&format!(
        "llama_server: starting exe={} model={} mmproj={:?} port={port} (log: {})",
        exe.display(), model.display(), mmproj, log_path.display()
    ));
    *CHILD.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);

    // 起動直後は健全性確認に応答しないため、短い間隔でポーリングする。
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if is_running(port) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("サーバーの起動を確認できませんでした(モデル読み込みに時間がかかっている可能性があります)".into())
}

/// サーバーを停止する。このプロセスが起動した子プロセスがあればそれを終了し、
/// 無ければ同名プロセスをイメージ名指定で終了させる(外部/前回セッションで起動された場合)。
pub fn stop() -> Result<(), String> {
    let child = CHILD.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
        return Ok(());
    }
    let status = std::process::Command::new("taskkill")
        .args(["/IM", "llama-server.exe", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err("サーバーの停止に失敗しました(既に停止している可能性があります)".into()),
    }
}
