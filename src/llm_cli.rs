//! ログイン済みLLM CLIの非対話実行。
//! シェル文字列を組み立てず、要求ごとの一時ディレクトリをcwdにして読み取り権限だけで呼ぶ。

use crate::config::{ApiProfile, ApiType};
use crate::llm_api::{LlmRequest, LlmResponse};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const CLI_TIMEOUT: Duration = Duration::from_secs(120);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CAPTURE_BYTES: u64 = 4 * 1024 * 1024;
static JOB_SEQ: AtomicU64 = AtomicU64::new(1);

struct JobDir(PathBuf);

impl JobDir {
    fn create() -> Result<Self, String> {
        let seq = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "FocusTranslator-llm-cli-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir(&path)
            .map_err(|e| format!("CLI用一時フォルダを作成できません: {e}"))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for JobDir {
    fn drop(&mut self) {
        // 自分で生成した一意な直下だけを削除する。失敗しても次回起動へ影響しない。
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct CommandSpec {
    executable: PathBuf,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
    final_message: Option<PathBuf>,
}

struct ProcessOutput {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// 手動指定またはPATHからCLI実行ファイルを解決する。現在の作業フォルダは検索しない。
pub fn resolve_executable(prof: &ApiProfile) -> Result<PathBuf, String> {
    if !prof.api_type.is_cli() {
        return Err("CLI種別ではありません".into());
    }
    let configured = prof.cli_path.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.components().count() > 1 || p.is_absolute() {
            let full = if p.is_absolute() {
                p
            } else {
                std::env::current_dir()
                    .map_err(|e| format!("現在のフォルダを取得できません: {e}"))?
                    .join(p)
            };
            return full.is_file().then_some(full).ok_or_else(|| {
                format!("指定されたCLI実行ファイルが見つかりません: {configured}")
            });
        }
        return find_on_path(configured)
            .ok_or_else(|| format!("CLI実行ファイルがPATHに見つかりません: {configured}"));
    }
    let command = prof
        .api_type
        .cli_command()
        .ok_or("CLIのコマンド名が定義されていません")?;
    find_on_path(command).ok_or_else(|| {
        format!(
            "{} が見つかりません。CLIを導入してPATHへ追加するか、設定画面で実行ファイルを指定してください",
            command
        )
    })
}

fn find_on_path(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let has_ext = Path::new(command).extension().is_some();
    #[cfg(windows)]
    let extensions: Vec<String> = if has_ext {
        vec![String::new()]
    } else {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect()
    };
    #[cfg(not(windows))]
    let extensions = vec![String::new()];

    for dir in std::env::split_paths(&path) {
        if has_ext {
            let candidate = dir.join(command);
            if candidate.is_file() {
                return Some(candidate);
            }
        } else {
            for ext in &extensions {
                let candidate = dir.join(format!("{command}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// キャッシュキーとして使う、APIリクエストJSON相当の安定した表現。
pub fn build_request_json(prof: &ApiProfile, req: &LlmRequest<'_>) -> String {
    let image_sha256 = req.image_png_b64.map(|b64| {
        let mut h = Sha256::new();
        h.update(b64.as_bytes());
        format!("{:x}", h.finalize())
    });
    serde_json::json!({
        "transport": "cli",
        "provider": prof.api_type,
        "model": prof.model_name,
        "prompt": req.prompt,
        "image_sha256": image_sha256,
        "json_mode": req.json_mode,
    })
    .to_string()
}

pub fn call(prof: &ApiProfile, req: &LlmRequest<'_>) -> Result<LlmResponse, String> {
    let executable = resolve_executable(prof)?;
    let job = JobDir::create()?;
    let image_path = match req.image_png_b64 {
        Some(b64) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("CLIへ渡す画像の復号に失敗しました: {e}"))?;
            let path = job.path().join("input.png");
            std::fs::write(&path, bytes)
                .map_err(|e| format!("CLI用一時画像を書き込めません: {e}"))?;
            Some(path)
        }
        None => None,
    };
    let spec = build_command(prof, executable, job.path(), image_path.as_deref(), req.prompt)?;
    let out = run_process(&spec, job.path(), CLI_TIMEOUT)?;
    if !out.success {
        return Err(process_error(prof, &out));
    }

    let raw = out.stdout.trim().to_string();
    let (mut text, tokens_in, tokens_out) = parse_output(&prof.api_type, &raw);
    if let Some(path) = spec.final_message.as_ref() {
        let file_text = read_limited(path).unwrap_or_default();
        if !file_text.trim().is_empty() {
            text = file_text.trim().to_string();
        }
    }
    if text.trim().is_empty() {
        return Err(format!(
            "{} は正常終了しましたが、応答本文がありません",
            prof.name
        ));
    }
    Ok(LlmResponse {
        text,
        request_json: build_request_json(prof, req),
        response_json: raw,
        tokens_in,
        tokens_out,
    })
}

/// APIの疎通確認と異なりクォータを消費せず、実行ファイルとバージョン表示だけを確認する。
pub fn check_connection(prof: &ApiProfile) -> Result<String, String> {
    let executable = resolve_executable(prof)?;
    let job = JobDir::create()?;
    let spec = CommandSpec {
        executable,
        args: vec![OsString::from("--version")],
        envs: Vec::new(),
        final_message: None,
    };
    let out = run_process(&spec, job.path(), CHECK_TIMEOUT)?;
    if !out.success {
        return Err(process_error(prof, &out));
    }
    let version = if out.stdout.trim().is_empty() {
        out.stderr.trim()
    } else {
        out.stdout.trim()
    };
    let version = compact_message(version, 180);
    Ok(if version.is_empty() {
        "CLI検出成功（ログイン状態は初回実行時に確認します）".into()
    } else {
        format!("CLI検出成功: {version}（ログイン状態は初回実行時に確認）")
    })
}

fn prompt_with_image(api_type: &ApiType, prompt: &str, image: Option<&Path>) -> String {
    let Some(path) = image else {
        return prompt.to_string();
    };
    let reference = match api_type {
        ApiType::CopilotCli | ApiType::KimiCli => "@input.png".to_string(),
        _ => path.display().to_string(),
    };
    format!(
        "{prompt}\n\nFocusTranslatorからの添付画像は {reference} です。この画像だけを読み取り、ファイル変更・コマンド実行・Webアクセスは行わないでください。"
    )
}

fn push_model(args: &mut Vec<OsString>, flag: &str, model: &str) {
    if !model.trim().is_empty() {
        args.push(flag.into());
        args.push(model.trim().into());
    }
}

fn build_command(
    prof: &ApiProfile,
    executable: PathBuf,
    work_dir: &Path,
    image: Option<&Path>,
    prompt: &str,
) -> Result<CommandSpec, String> {
    let effective_prompt = prompt_with_image(&prof.api_type, prompt, image);
    let mut envs = Vec::new();
    let (args, final_message) = match prof.api_type {
        ApiType::CodexCli => {
            let final_path = work_dir.join("final.txt");
            let mut args: Vec<OsString> = vec![
                "exec".into(),
                "--skip-git-repo-check".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--ask-for-approval".into(),
                "never".into(),
                "--json".into(),
                "--output-last-message".into(),
                final_path.as_os_str().to_owned(),
                "-C".into(),
                work_dir.as_os_str().to_owned(),
            ];
            push_model(&mut args, "--model", &prof.model_name);
            if let Some(path) = image {
                args.push("--image".into());
                args.push(path.as_os_str().to_owned());
            }
            args.push(effective_prompt.into());
            (args, Some(final_path))
        }
        ApiType::ClaudeCli => {
            let mut args: Vec<OsString> = vec![
                "--print".into(),
                effective_prompt.into(),
                "--output-format".into(),
                "json".into(),
                "--permission-mode".into(),
                "dontAsk".into(),
                "--no-session-persistence".into(),
                "--tools".into(),
                if image.is_some() { "Read".into() } else { "".into() },
                "--disallowedTools".into(),
                "Bash,Write,Edit,WebFetch,WebSearch,Agent".into(),
            ];
            push_model(&mut args, "--model", &prof.model_name);
            (args, None)
        }
        ApiType::CopilotCli => {
            let mut args: Vec<OsString> = vec![
                "--prompt".into(),
                effective_prompt.into(),
                "--silent".into(),
                "--no-ask-user".into(),
                "--allow-tool=read".into(),
                "--deny-tool=shell,write,url,memory".into(),
            ];
            push_model(&mut args, "--model", &prof.model_name);
            (args, None)
        }
        ApiType::GeminiCli => {
            let mut args: Vec<OsString> = vec![
                "--prompt".into(),
                effective_prompt.into(),
                "--output-format".into(),
                "json".into(),
                "--approval-mode".into(),
                "default".into(),
            ];
            if image.is_some() {
                args.push("--allowed-tools".into());
                args.push("read_file".into());
            }
            push_model(&mut args, "--model", &prof.model_name);
            (args, None)
        }
        ApiType::KimiCli => {
            let agent_path = work_dir.join("focus-translator-agent.md");
            std::fs::write(
                &agent_path,
                "---\nname: focus-translator\ndescription: Read-only OCR, translation, and explanation agent\ntools:\n  - ReadMediaFile\nsubagents: []\n---\nAnswer the user request directly. Never modify files, run commands, delegate, or access the web.\n",
            )
            .map_err(|e| format!("Kimi CLI用の権限制限ファイルを書き込めません: {e}"))?;
            let mut args: Vec<OsString> = vec![
                "--agent-file".into(),
                agent_path.as_os_str().to_owned(),
                "--prompt".into(),
                effective_prompt.into(),
                "--output-format".into(),
                "stream-json".into(),
            ];
            push_model(&mut args, "--model", &prof.model_name);
            envs.push(("KIMI_CODE_EXPERIMENTAL_FLAG".into(), "1".into()));
            (args, None)
        }
        _ => return Err("CLI以外のプロファイルがCLI実行経路へ渡されました".into()),
    };
    Ok(CommandSpec { executable, args, envs, final_message })
}

fn run_process(spec: &CommandSpec, cwd: &Path, timeout: Duration) -> Result<ProcessOutput, String> {
    let stdout_path = cwd.join("stdout.txt");
    let stderr_path = cwd.join("stderr.txt");
    let stdout_file = File::create(&stdout_path)
        .map_err(|e| format!("CLI標準出力ファイルを作成できません: {e}"))?;
    let stderr_file = File::create(&stderr_path)
        .map_err(|e| format!("CLI標準エラーファイルを作成できません: {e}"))?;
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .env("NO_COLOR", "1")
        .env("TERM", "dumb");
    for (key, value) in &spec.envs {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| {
        format!(
            "CLIを起動できません ({}): {e}",
            spec.executable.display()
        )
    })?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(40)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "CLIが{}秒以内に完了しなかったため終了しました",
                    timeout.as_secs()
                ));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("CLIプロセスの状態確認に失敗しました: {e}"));
            }
        }
    };
    Ok(ProcessOutput {
        success: status.success(),
        code: status.code(),
        stdout: read_limited(&stdout_path).unwrap_or_default(),
        stderr: read_limited(&stderr_path).unwrap_or_default(),
    })
}

fn read_limited(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("{}を読み込めません: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_CAPTURE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}を読み込めません: {e}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn process_error(prof: &ApiProfile, out: &ProcessOutput) -> String {
    let detail = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    format!(
        "{}の実行に失敗しました (終了コード: {}): {}",
        prof.name,
        out.code.map(|c| c.to_string()).unwrap_or_else(|| "不明".into()),
        compact_message(detail, 1200)
    )
}

fn compact_message(text: &str, max_chars: usize) -> String {
    let clean = strip_ansi(text).replace('\r', "");
    let joined = clean.lines().map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    let mut out: String = joined.chars().take(max_chars).collect();
    if joined.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_output(api_type: &ApiType, raw: &str) -> (String, Option<i64>, Option<i64>) {
    if matches!(api_type, ApiType::CopilotCli) {
        return (raw.trim().to_string(), None, None);
    }
    let values = parse_json_values(raw);
    let mut text = String::new();
    let mut tokens_in = None;
    let mut tokens_out = None;
    for value in &values {
        if let Some(candidate) = extract_text(value)
            && !candidate.trim().is_empty()
        {
            text = candidate.trim().to_string();
        }
        update_usage(value, &mut tokens_in, &mut tokens_out);
    }
    if text.is_empty() && values.is_empty() {
        text = raw.trim().to_string();
    }
    (text, tokens_in, tokens_out)
}

fn parse_json_values(raw: &str) -> Vec<serde_json::Value> {
    if let Ok(value) = serde_json::from_str(raw) {
        return vec![value];
    }
    raw.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .collect()
}

fn extract_text(value: &serde_json::Value) -> Option<String> {
    let obj = value.as_object()?;
    for key in ["result", "response", "output_text"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    if obj.get("role").and_then(|v| v.as_str()) == Some("assistant")
        && let Some(content) = obj.get("content")
    {
        return content_text(content);
    }
    if let Some(item) = obj.get("item")
        && let Some(text) = extract_text(item)
    {
        return Some(text);
    }
    if let Some(message) = obj.get("message") {
        if let Some(text) = message.as_str() {
            return Some(text.to_string());
        }
        if let Some(text) = extract_text(message) {
            return Some(text);
        }
    }
    if let Some(content) = obj.get("content") {
        return content_text(content);
    }
    None
}

fn content_text(value: &serde_json::Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = value.as_object() {
        return obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    let parts = value.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(|v| v.as_str())
                .or_else(|| part.get("content").and_then(|v| v.as_str()))
        })
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

fn update_usage(value: &serde_json::Value, input: &mut Option<i64>, output: &mut Option<i64>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if let Some(n) = child.as_i64() {
                    if matches!(
                        key.as_str(),
                        "input_tokens"
                            | "inputTokens"
                            | "prompt_tokens"
                            | "promptTokenCount"
                            | "prompt_tokens_count"
                    ) {
                        *input = Some(input.unwrap_or(0).max(n));
                    } else if matches!(
                        key.as_str(),
                        "output_tokens"
                            | "outputTokens"
                            | "completion_tokens"
                            | "candidatesTokenCount"
                            | "completion_tokens_count"
                    ) {
                        *output = Some(output.unwrap_or(0).max(n));
                    }
                }
                update_usage(child, input, output);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                update_usage(child, input, output);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(api_type: ApiType) -> ApiProfile {
        ApiProfile {
            name: "test".into(),
            api_type,
            model_name: String::new(),
            api_url: String::new(),
            api_key_enc: String::new(),
            cli_path: String::new(),
            ocr_prompt: String::new(),
            translate_prompt: String::new(),
            explain_prompt: String::new(),
            max_tokens: 4096,
        }
    }

    #[test]
    fn claude_jsonから本文とusageを取得する() {
        let raw = r#"{"type":"result","result":"翻訳結果","usage":{"input_tokens":12,"output_tokens":7}}"#;
        assert_eq!(
            parse_output(&ApiType::ClaudeCli, raw),
            ("翻訳結果".into(), Some(12), Some(7))
        );
    }

    #[test]
    fn kimi_jsonlは最後のassistant本文を使う() {
        let raw = "{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"途中\"}]}\n{\"role\":\"tool\",\"content\":\"x\"}\n{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"最終\"}]}";
        assert_eq!(parse_output(&ApiType::KimiCli, raw).0, "最終");
    }

    #[test]
    fn cliキャッシュキーは画像とモデルを含む() {
        let mut p = profile(ApiType::CodexCli);
        let req = LlmRequest { prompt: "OCR", image_png_b64: Some("AAAA"), json_mode: true };
        let a = build_request_json(&p, &req);
        p.model_name = "別モデル".into();
        let b = build_request_json(&p, &req);
        assert_ne!(a, b);
        assert!(a.contains("image_sha256"));
    }

    #[test]
    fn copilotは読み取りだけを許可し副作用ツールを拒否する() {
        let p = profile(ApiType::CopilotCli);
        let cwd = Path::new("C:\\Temp\\ft-cli-test");
        let spec = build_command(&p, "copilot.exe".into(), cwd, Some(&cwd.join("input.png")), "OCR")
            .unwrap();
        let args = spec.args.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");
        assert!(args.contains("--allow-tool=read"));
        assert!(args.contains("--deny-tool=shell,write,url,memory"));
        assert!(args.contains("@input.png"));
    }

    #[test]
    fn ansiエスケープをエラー表示から除去する() {
        assert_eq!(compact_message("\u{1b}[31merror\u{1b}[0m\n detail", 100), "error detail");
    }

    #[test]
    fn 手動指定した実行ファイルを優先する() {
        let mut p = profile(ApiType::ClaudeCli);
        p.cli_path = std::env::current_exe().unwrap().display().to_string();
        assert_eq!(resolve_executable(&p).unwrap(), std::env::current_exe().unwrap());
    }
}
