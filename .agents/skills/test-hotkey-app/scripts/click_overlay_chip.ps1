<#
.SYNOPSIS
FocusTranslatorオーバーレイのチップボタンを、フォーカスを奪わずに座標クリックする。

.DESCRIPTION
チップの座標はオーバーレイのレイアウト(表示内容次第で変わる)に依存するため、事前に
ピン留め状態のオーバーレイをスクリーンショットで確認し、座標を目視で決めてから渡すこと。
このスクリプト自体はその座標でのクリックを、キー押下と同じ安全確認パターンで行う:
クリック直前に WindowFromPoint + GetClassNameW でカーソル直下が本当に
FocusTranslatorOverlay(既定)かを確認し、一致しなければ中止する。

.PARAMETER X / Y
クリックするスクリーン座標(物理ピクセル)。事前のスクリーンショットで確認した値を渡す。

.PARAMETER ExpectedClass
クリック対象として許容するウィンドウクラス名。既定はオーバーレイ本体。
ログビューア等の別ウィンドウ上のボタンをクリックする場合はここを変更する
(例: ログビューアなら "FocusTranslatorLogViewer" 等、実際のクラス名を確認して指定)。

.PARAMETER WaitAfterMs
クリック後、結果が反映されるまで待つ時間(ms)。ネットワーク呼び出しを伴う場合は長めに。

.PARAMETER ScreenshotPath
クリック後に保存するスクリーンショットのパス。

.EXAMPLE
pwsh -File click_overlay_chip.ps1 -X 498 -Y 307
#>
param(
    [Parameter(Mandatory=$true)][int]$X,
    [Parameter(Mandatory=$true)][int]$Y,
    [string]$ExpectedClass = "FocusTranslatorOverlay",
    [int]$WaitAfterMs = 3000,
    [string]$ScreenshotPath = "$PSScriptRoot\click_overlay_chip_result.png"
)

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class ChipClick {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    public struct POINT { public int X, Y; }
}
'@

[ChipClick]::SetCursorPos($X, $Y) | Out-Null
Start-Sleep -Milliseconds 250

$p = New-Object ChipClick+POINT; $p.X = $X; $p.Y = $Y
$hit = [ChipClick]::WindowFromPoint($p)
$sb = New-Object System.Text.StringBuilder 256
[ChipClick]::GetClassNameW($hit, $sb, 256) | Out-Null
$cls = $sb.ToString()
Write-Host "cursor-under class: $cls (hwnd=$hit)"

if ($cls -ne $ExpectedClass) {
    Write-Error "ABORT: カーソル直下が期待するクラス($ExpectedClass)と一致しません(実際: $cls)。座標を確認し直してください。"
    exit 1
}

# 安全確認できたのでクリック (LEFTDOWN=0x0002, LEFTUP=0x0004)
[ChipClick]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[ChipClick]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Write-Host "clicked at $X,$Y"

Start-Sleep -Milliseconds $WaitAfterMs

Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap 1300, 800
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0, 0, 0, 0, $bmp.Size)
$bmp.Save($ScreenshotPath)
$g.Dispose(); $bmp.Dispose()
Write-Host "screenshot: $ScreenshotPath"
