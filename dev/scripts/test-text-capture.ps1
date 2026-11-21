param([switch]$Interactive)
$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot)
Push-Location $root
try {
    # 今回追加した状態遷移・矩形・設定互換だけを実行する。
    cargo test --locked --offline --target-dir target/management text_capture
    if ($LASTEXITCODE) { throw 'テキストキャプチャの対象限定試験に失敗しました。' }
    if (-not $Interactive) { return }
    cargo build --locked --offline --target-dir target/management
    if ($LASTEXITCODE) { throw 'アプリのビルドに失敗しました。' }
    dotnet build tests/CaptureSmoke/CaptureSmoke.csproj -o target/capture-smoke --nologo
    if ($LASTEXITCODE) { throw '実入力試験のビルドに失敗しました。' }
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "([0-9.]+)"').Matches[0].Groups[1].Value
    $logs = Join-Path $root ('dev/history/v' + $version + '/logs/text-capture-' + [Guid]::NewGuid().ToString('N'))
    & ./target/capture-smoke/CaptureSmoke.exe (Join-Path $root 'target/management/debug/focus-translator.exe') $logs
    if ($LASTEXITCODE) { throw "実入力試験が完了しませんでした。記録: $logs" }
} finally { Pop-Location }
