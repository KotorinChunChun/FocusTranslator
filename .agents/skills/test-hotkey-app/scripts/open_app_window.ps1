<#
.SYNOPSIS
FocusTranslator本体(常駐プロセス)の設定画面・ログビューアを、フォーカスを奪わずに開いて
スクリーンショットを撮る。

.DESCRIPTION
本体のメインウィンドウ(クラス FocusTranslatorMain, 非表示)は EnumWindows で見つける
(FindWindowW はこのウィンドウに対して 0 を返すことがあるため使わない)。そこへ
WM_COMMAND(0x0111) を tray.rs の CMD_* 定数の値で PostMessage すると、対応する
ウィンドウ(設定/ログビューア)が開く。フォーカスや前面化は一切不要。

対象ウィンドウはクラス名で見つけ、フォーカスを奪わずに最前面へ(SWP_NOACTIVATE)上げて
からスクリーンショットする。

.PARAMETER Which
"Settings" または "LogViewer"。

.PARAMETER Close
既に開いている対象ウィンドウを WM_CLOSE(0x0010) で閉じるだけの動作にする場合に指定。

.PARAMETER ScreenshotPath
保存先。省略時はスクリプトと同じ場所。

.EXAMPLE
pwsh -File open_app_window.ps1 -Which Settings

.EXAMPLE
pwsh -File open_app_window.ps1 -Which LogViewer

.EXAMPLE
pwsh -File open_app_window.ps1 -Which Settings -Close
#>
param(
    [Parameter(Mandatory=$true)][ValidateSet("Settings", "LogViewer")][string]$Which,
    [switch]$Close,
    [string]$ScreenshotPath
)

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class AppWin {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    public struct RECT { public int L, T, R, B; }
}
'@

function Find-WindowByClass {
    param([string]$Class)
    $script:foundHwnd = [IntPtr]::Zero
    $cb = [AppWin+EnumProc]{ param($h, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [AppWin]::GetClassNameW($h, $sb, 256) | Out-Null
        if ($sb.ToString() -eq $Class -and $script:foundHwnd -eq [IntPtr]::Zero) { $script:foundHwnd = $h }
        $true
    }
    [AppWin]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
    return $script:foundHwnd
}

# tray.rs の CMD_* 定数 (WM_COMMAND の wParam)
$cmdMap = @{ Settings = 1; LogViewer = 3 }
$classMap = @{ Settings = "FocusTranslatorSettings"; LogViewer = "FocusTranslatorLogViewer" }
$targetClass = $classMap[$Which]

if ($Close) {
    $target = Find-WindowByClass -Class $targetClass
    if ($target -eq [IntPtr]::Zero) { Write-Host "既に閉じています ($targetClass)"; exit 0 }
    [AppWin]::PostMessageW($target, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    Write-Host "closed: $targetClass"
    exit 0
}

$main = Find-WindowByClass -Class "FocusTranslatorMain"
if ($main -eq [IntPtr]::Zero) {
    Write-Error "FAILED: FocusTranslator本体が起動していません(FocusTranslatorMainが見つかりません)"
    exit 1
}

[AppWin]::PostMessageW($main, 0x0111, [IntPtr]$cmdMap[$Which], [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 1200

$target = [IntPtr]::Zero
for ($i = 0; $i -lt 10 -and $target -eq [IntPtr]::Zero; $i++) {
    $target = Find-WindowByClass -Class $targetClass
    if ($target -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 300 }
}
if ($target -eq [IntPtr]::Zero) {
    Write-Error "FAILED: $targetClass のウィンドウが開きませんでした"
    exit 1
}
Write-Host "$Which window: $target"

# フォーカスを奪わずに最前面へ (HWND_TOPMOST=-1, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE=0x13)
[AppWin]::SetWindowPos($target, [IntPtr]::new(-1), 0, 0, 0, 0, 0x13) | Out-Null
Start-Sleep -Milliseconds 300

$r = New-Object AppWin+RECT
[AppWin]::GetWindowRect($target, [ref]$r) | Out-Null
$w = $r.R - $r.L; $h = $r.B - $r.T
Write-Host "rect: $($r.L),$($r.T) - $($r.R),$($r.B)"

if (-not $ScreenshotPath) { $ScreenshotPath = "$PSScriptRoot\$($Which.ToLower())_window.png" }
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
$bmp.Save($ScreenshotPath)
$g.Dispose(); $bmp.Dispose()
Write-Host "screenshot: $ScreenshotPath"
