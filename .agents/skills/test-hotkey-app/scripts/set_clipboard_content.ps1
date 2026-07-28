<#
.SYNOPSIS
クリップボードへテキストまたは画像を設定する(FocusTranslatorの「コピー中の内容」
ボタン検証用)。

.DESCRIPTION
テキストは Set-Clipboard で即座に設定できる(特別なアパートメント状態は不要)。
画像 (Clipboard::SetImage) は STA 必須。pwsh は既定 MTA のため、-ImageLines 指定時は
自分自身のアパートメント状態を確認し、STA でなければ `pwsh -STA -File $PSCommandPath ...`
で自己再実行する(呼び出し側は意識しなくてよい)。

同一プロセス内で System.Threading.Thread を起こし SetApartmentState(STA) してから
ScriptBlock を実行する方法は失敗する(PSInvalidOperationException: There is no Runspace
available — ScriptBlock実行にはそのスレッドに紐づくRunspaceが必要なため)。また
powershell.exe(Windows PowerShell 5.1)の -STA は既定コードページで.ps1を読むため、
日本語コメントを含むスクリプトを渡すと文字化けしてパースエラーになる。pwsh -STA は
UTF-8既定のためこの問題が起きない(実機で確認済み)。

.PARAMETER Text
クリップボードに設定するテキスト。指定時は画像設定より優先される。

.PARAMETER Line1
画像に描画する1行目の英文。

.PARAMETER Line2
画像に描画する2行目の英文。

.PARAMETER ImageWidth / ImageHeight
生成するビットマップのサイズ。

.EXAMPLE
pwsh -File set_clipboard_content.ps1 -Text "sample text"

.EXAMPLE
pwsh -File set_clipboard_content.ps1 -Line1 "Clipboard image OCR test." -Line2 "This picture came from the clipboard."
#>
param(
    [string]$Text,
    [string]$Line1 = "Clipboard image OCR test.",
    [string]$Line2 = "This picture came from the clipboard.",
    [int]$ImageWidth = 520,
    [int]$ImageHeight = 90
)

if ($Text) {
    Set-Clipboard -Value $Text
    Write-Host "clipboard text set: $Text"
    exit 0
}

# 画像設定はSTA必須。現在のアパートメント状態を見て、MTAなら pwsh -STA で自己再実行する。
$apt = [System.Threading.Thread]::CurrentThread.GetApartmentState()
if ($apt -ne [System.Threading.ApartmentState]::STA) {
    & pwsh -STA -NoProfile -File $PSCommandPath -Line1 $Line1 -Line2 $Line2 -ImageWidth $ImageWidth -ImageHeight $ImageHeight
    exit $LASTEXITCODE
}

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$bmp = New-Object System.Drawing.Bitmap $ImageWidth, $ImageHeight
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$font = New-Object System.Drawing.Font("Segoe UI", 16)
$g.DrawString($Line1, $font, [System.Drawing.Brushes]::Black, 12, 12)
$g.DrawString($Line2, $font, [System.Drawing.Brushes]::Black, 12, 46)
$g.Dispose()
[System.Windows.Forms.Clipboard]::SetImage($bmp)
$bmp.Dispose()
Write-Host "clipboard image set ($ImageWidth x $ImageHeight): '$Line1' / '$Line2'"
