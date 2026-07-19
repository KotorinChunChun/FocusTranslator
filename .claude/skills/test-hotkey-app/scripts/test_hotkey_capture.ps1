<#
.SYNOPSIS
FocusTranslator (or any cursor-position + GetAsyncKeyState driven hotkey app) の実機検証を、
フォアグラウンド/フォーカスを一切奪わずに1プロセスで完結させるテンプレート。

.DESCRIPTION
なぜフォーカスが不要か:
FocusTranslator の認識トリガーは WindowFromPoint(カーソル位置直下のウィンドウ) と
GetAsyncKeyState(物理キーの押下状態) だけに依存しており、どちらも Win32 の仕様として
ウィンドウフォーカス/フォアグラウンド状態と無関係。したがって検証用アプリ(既定は
メモ帳)を前面化する必要は本来ない。SetForegroundWindow はOSのセキュリティ機能で
ブロックされやすく、AttachThreadInput 等の回避策も環境によっては効かない
(このリポジトリを別IDEセッションで開いている場合など、ツール呼び出しの合間に
フォーカスが奪い返されることがある)。SendKeys でのテキスト入力は特に危険で、
過去に無関係な別ウィンドウへテスト文字列を誤注入した事故がある。

このスクリプトは代わりに:
- テキスト投入は SendMessageW + WM_SETTEXT で対象HWNDへ直接行う(フォーカス不要)
- キーを押す直前に WindowFromPoint + GetWindowThreadProcessId でカーソル直下が
  本当に目的のプロセスかを検証する(これが本当の安全確認であり、フォアグラウンド
  確認ではない)
- 後片付けも対象HWNDへの PostMessageW(WM_CLOSE) で行う(フォーカス不要)
という、フォーカス非依存な手段だけで全工程を完結させる。

全工程は必ず1回の呼び出し(1プロセス)内で完結させること。呼び出しを分割すると、
間で前のプロセスが終了するたびにフォーカスが他のウィンドウ(IDE等)へ戻り得る。

.PARAMETER TestText
メモ帳へ流し込むテキスト。既定値は「狭い幅で折り返されるほど長い文」を含む
複数段落(空行区切り)で、FocusTranslator の段落復元・折り返し結合・UIA子孫テキスト
連結ロジックを実際に検証できる構成になっている。単純な一行だけのテキストに
差し替えると、これらの機能の大部分がテスト対象から外れてしまうので注意。

.PARAMETER WindowWidth
メモ帳ウィンドウの幅(既定550px = 折り返しが確実に発生する狭さ)。

.PARAMETER CursorOffsetX / CursorOffsetY
ウィンドウの実測左上(GetWindowRectで測った値)からの相対カーソル位置。
既定は2段落目あたりを指すよう調整済み。

.PARAMETER HoldVk
押下する仮想キーコード。既定 0xA3 (右Ctrl = FocusTranslatorの既定キャプチャキー)。
プレビューキー(既定 左Ctrl)を試す場合は 0xA2 を指定する。

.PARAMETER ScreenshotPath
結果を保存するPNGのフルパス。省略時はスクリプトと同じ場所に test_capture.png。

.EXAMPLE
pwsh -File test_hotkey_capture.ps1
既定のテキスト・既定のキー(RCtrl)でメモ帳を使い、フォーカスを奪わずに検証する。

.EXAMPLE
pwsh -File test_hotkey_capture.ps1 -HoldVk 0xA2 -ScreenshotPath C:\tmp\preview_test.png
プレビューキー(LCtrl)側の領域表示をテストする。
#>
param(
    [string]$TestText = "This is a deliberately long sentence written specifically to wrap across multiple visual lines once the window is narrowed, so that the paragraph reconstruction logic actually has wrapped text to stitch back together.`r`n`r`nThis is the second paragraph, also long enough that it wraps at the right edge of a narrow window, and it should be treated as a separate paragraph from the first one because of the blank line gap between them.`r`n`r`nShort third paragraph.",
    [int]$WindowX = 100,
    [int]$WindowY = 100,
    [int]$WindowWidth = 550,
    [int]$WindowHeight = 500,
    [int]$CursorOffsetX = 200,
    [int]$CursorOffsetY = 200,
    [int]$HoldVk = 0xA3,
    [int]$HoldMs = 1500,
    [string]$ScreenshotPath = "$PSScriptRoot\test_capture.png"
)

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class ChildEnumTHA {
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hWndParent, EnumChildProc lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder sb, int max);
}
public static class WinTestHA {
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int he, bool repaint);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wParam, string lParam);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    public struct RECT { public int L, T, R, B; }
    public struct POINT { public int X, Y; }
}
'@

function Find-EditControl {
    # モダンNotepad(RichEditD2DPT) / 従来のEdit / RichEdit(RICHEDIT50W) の順に子孫を探す。
    # 新しい編集コントロールクラスが見つかった場合は、末尾でクラス名一覧をここに追記する。
    #
    # 注意: コールバック内で $script: スコープに書き込む場合、関数ローカル変数とは別物になる。
    # ここでは $script:editControlResult を唯一の受け渡し場所として使い、必ず関数の先頭で
    # リセットしてから列挙し、関数末尾でそれを読んで返す(ローカル変数を混在させない)。
    param([IntPtr]$Parent)
    $candidates = @("RichEditD2DPT", "Edit", "RICHEDIT50W", "RichEdit20W")
    $script:editControlResult = [IntPtr]::Zero
    $cb = [ChildEnumTHA+EnumChildProc]{ param($h, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [ChildEnumTHA]::GetClassNameW($h, $sb, 256) | Out-Null
        if ($candidates -contains $sb.ToString() -and $script:editControlResult -eq [IntPtr]::Zero) {
            $script:editControlResult = $h
        }
        $true
    }
    [ChildEnumTHA]::EnumChildWindows($Parent, $cb, [IntPtr]::Zero) | Out-Null
    return $script:editControlResult
}

# 1) メモ帳を起動して1秒待つ(起動直後はウィンドウがまだ生成されていないため)
$proc = Start-Process notepad -PassThru
Start-Sleep -Seconds 1

# 2) 実ウィンドウのhwndを確定する(モダンNotepadはホストプロセスと実ウィンドウのpidが
#    分かれることがあるため、Get-Process のMainWindowHandleが立つまでポーリングする)
$hwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 15 -and $hwnd -eq [IntPtr]::Zero; $i++) {
    Get-Process -Name notepad -ErrorAction SilentlyContinue | ForEach-Object {
        if ($_.MainWindowHandle -ne [IntPtr]::Zero -and $script:hwnd -eq [IntPtr]::Zero) { $script:hwnd = $_.MainWindowHandle }
    }
    if ($hwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 300 }
}
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "FAILED: notepadのウィンドウを確定できませんでした"; exit 1 }
Write-Host "hwnd: $hwnd"

# 3) 編集コントロールを子孫から探し、WM_SETTEXT(0x000C)でテキストを直接設定する。
#    フォーカス・フォアグラウンド不要。対象HWND限定なので他ウィンドウに漏れる余地がない。
#    子コントロールの生成タイミングにはばらつきがあるため、見つかるまで少し待ってリトライする。
$editHwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 10 -and $editHwnd -eq [IntPtr]::Zero; $i++) {
    $editHwnd = Find-EditControl -Parent $hwnd
    if ($editHwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 300 }
}
if ($editHwnd -eq [IntPtr]::Zero) { Write-Error "FAILED: 編集コントロールが見つかりませんでした"; exit 1 }
Write-Host "edit control: $editHwnd"
[WinTestHA]::SendMessageW($editHwnd, 0x000C, [IntPtr]::Zero, $TestText) | Out-Null
Write-Host "text set via WM_SETTEXT (フォーカス不要)"

# 4) 狭い幅へ移動し、必ず GetWindowRect で実測する(想定座標を信じない)
[WinTestHA]::MoveWindow($hwnd, $WindowX, $WindowY, $WindowWidth, $WindowHeight, $true) | Out-Null
# 他ウィンドウに覆われているとカーソル直下検証(手順6)が失敗するため、フォーカスを
# 奪わずに最前面へ上げる (HWND_TOPMOST=-1, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE=0x13)。
[WinTestHA]::SetWindowPos($hwnd, [IntPtr]::new(-1), 0, 0, 0, 0, 0x13) | Out-Null
Start-Sleep -Milliseconds 300
$rect = New-Object WinTestHA+RECT
[WinTestHA]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
Write-Host "confirmed rect: $($rect.L),$($rect.T) - $($rect.R),$($rect.B)"

# 5) 実測矩形内にカーソルを置く
$cx = $rect.L + $CursorOffsetX
$cy = $rect.T + $CursorOffsetY
[WinTestHA]::SetCursorPos($cx, $cy) | Out-Null

# 6) 安全確認: キーを押す直前に、カーソル直下が本当に目的のウィンドウかを検証する。
#    これが唯一の本質的なリスクチェックであり、フォアグラウンド状態は関係ない。
#    注意: WindowFromPoint はモダンメモ帳の子 InputSiteWindowClass を返すことがあり、
#    その所有プロセスは別(テキスト入力ホスト)のため pid 照合は不安定。
#    GetAncestor(GA_ROOT=2) でルートへ辿り、hwnd 同士で照合する。
$p = New-Object WinTestHA+POINT; $p.X = $cx; $p.Y = $cy
$hit = [WinTestHA]::WindowFromPoint($p)
$hitRoot = [WinTestHA]::GetAncestor($hit, 2)
Write-Host "cursor check: hitRoot=$hitRoot target=$hwnd match=$($hitRoot -eq $hwnd)"
if ($hitRoot -ne $hwnd) {
    Write-Error "ABORT: カーソル直下が目的のウィンドウと一致しません。キー送信を中止します。"
    exit 1
}

# 7) 確認できて初めてキーを押す。既定は右Ctrl(FocusTranslatorのキャプチャキー)。
[WinTestHA]::keybd_event($HoldVk, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds $HoldMs
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap 1600, 1000
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0, 0, 0, 0, $bmp.Size)
$bmp.Save($ScreenshotPath)
$g.Dispose(); $bmp.Dispose()
[WinTestHA]::keybd_event($HoldVk, 0, 2, [UIntPtr]::Zero)  # KEYUP
Write-Host "screenshot saved: $ScreenshotPath"

# 8) 後片付け: 対象hwndへWM_CLOSE(0x0010)を送る(フォーカス不要)
[WinTestHA]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 300
Write-Host "SUCCESS: フォーカス/フォアグラウンドを一切奪わずに完了 (hwnd=$hwnd editHwnd=$editHwnd)"
