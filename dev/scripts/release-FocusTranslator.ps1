[CmdletBinding()]
param(
    [switch]$Publish,
    [switch]$AllowUnsigned,
    [string]$SigningCertificateThumbprint = $env:FOCUSTRANSLATOR_SIGNING_CERT_THUMBPRINT,
    [string]$ReleaseNotesPath
)

# FocusTranslator リリース成果物作成・GitHub draft作成スクリプト。
# 既定ではローカル成果物だけを作り、-Publish 指定時だけGitHubへdraftを作成する。
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Push-Location $ProjectRoot

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)][string]$Description,
        [Parameter(Mandatory = $true)][scriptblock]$Command
    )

    Write-Host $Description -ForegroundColor Yellow
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Description に失敗しました (exit code: $LASTEXITCODE)"
    }
}

function Find-OsvScanner {
    $command = Get-Command osv-scanner -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $wingetRoot = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    $candidate = Get-ChildItem $wingetRoot -Filter 'osv-scanner.exe' -File -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($candidate) {
        return $candidate.FullName
    }
    throw 'OSV Scannerが見つかりません。winget install --exact --id Google.OSVScanner を実行してください。'
}

function Find-Iscc {
    $candidates = @(
        'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        'C:\Program Files\Inno Setup 6\ISCC.exe'
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    throw 'Inno Setup Compiler (ISCC.exe) が見つかりません。Setup.exeを作成できないためリリースを中止します。'
}

function Get-ReleaseSigningCertificate {
    if ([string]::IsNullOrWhiteSpace($SigningCertificateThumbprint)) {
        if ($AllowUnsigned) {
            Write-Warning 'コード署名証明書が指定されていないため、明示指定に従い未署名で続行します。'
            return $null
        }
        throw 'コード署名証明書が指定されていません。FOCUSTRANSLATOR_SIGNING_CERT_THUMBPRINTを設定するか、未署名を明示的に許可する場合だけ-AllowUnsignedを指定してください。'
    }

    $thumbprint = $SigningCertificateThumbprint.Replace(' ', '')
    $certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$thumbprint" -ErrorAction SilentlyContinue
    if (-not $certificate) {
        throw "コード署名証明書が見つかりません: $thumbprint"
    }
    if (-not $certificate.HasPrivateKey) {
        throw "コード署名証明書に秘密鍵がありません: $thumbprint"
    }
    if ($certificate.NotAfter -le (Get-Date)) {
        throw "コード署名証明書の有効期限が切れています: $($certificate.NotAfter)"
    }
    if ($certificate.EnhancedKeyUsageList.ObjectId -notcontains '1.3.6.1.5.5.7.3.3') {
        throw '指定された証明書はコード署名用途ではありません。'
    }
    return $certificate
}

function Set-AndVerifyAuthenticodeSignature {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        $Certificate
    )

    if (-not $Certificate) {
        $unsigned = Get-AuthenticodeSignature -FilePath $Path
        if ($unsigned.Status -ne 'NotSigned') {
            throw "未署名許可時の署名状態が想定外です: $Path ($($unsigned.Status))"
        }
        return
    }

    $result = Set-AuthenticodeSignature `
        -FilePath $Path `
        -Certificate $Certificate `
        -HashAlgorithm SHA256 `
        -TimestampServer 'http://timestamp.digicert.com'
    if ($result.Status -ne 'Valid') {
        throw "Authenticode署名に失敗しました: $Path ($($result.Status): $($result.StatusMessage))"
    }

    $verified = Get-AuthenticodeSignature -FilePath $Path
    if ($verified.Status -ne 'Valid') {
        throw "Authenticode署名の再検証に失敗しました: $Path ($($verified.Status))"
    }
}

try {
    $cargoToml = Get-Content 'Cargo.toml' -Raw
    if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
        throw 'Cargo.tomlからパッケージバージョンを取得できませんでした。'
    }
    $version = $Matches[1]
    $tagName = "v$version"

    $installerVersionLine = Select-String -Path 'installer.iss' -Pattern '^AppVersion=(.+)$' |
        Select-Object -First 1
    if (-not $installerVersionLine -or $installerVersionLine.Matches[0].Groups[1].Value -ne $version) {
        throw "Cargo.tomlとinstaller.issのバージョンが一致しません (Cargo: $version)"
    }

    if (-not $ReleaseNotesPath) {
        $ReleaseNotesPath = Join-Path $ProjectRoot ".github\RELEASE_NOTES_$tagName.md"
    }
    $ReleaseNotesPath = [System.IO.Path]::GetFullPath($ReleaseNotesPath)
    if (-not (Test-Path -LiteralPath $ReleaseNotesPath)) {
        throw "リリースノートが見つかりません: $ReleaseNotesPath"
    }
    $releaseNotes = Get-Content -LiteralPath $ReleaseNotesPath -Raw
    if ($releaseNotes -match '<バージョン>|<変更点|<修正内容') {
        throw "リリースノートに未置換のプレースホルダーがあります: $ReleaseNotesPath"
    }

    $gitRoot = (& git rev-parse --show-toplevel).Trim()
    if ($LASTEXITCODE -ne 0 -or [System.IO.Path]::GetFullPath($gitRoot) -ne [System.IO.Path]::GetFullPath($ProjectRoot)) {
        throw 'Gitリポジトリのルートとプロジェクトルートが一致しません。'
    }
    $dirty = @(& git status --porcelain)
    if ($LASTEXITCODE -ne 0 -or $dirty.Count -gt 0) {
        throw '作業ツリーに未コミット変更があります。すべてコミットしてから再実行してください。'
    }

    $branch = (& git branch --show-current).Trim()
    if ([string]::IsNullOrWhiteSpace($branch)) {
        throw 'detached HEADではリリースできません。リリース対象ブランチへ切り替えてください。'
    }
    $upstream = (& git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>$null).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($upstream)) {
        throw "現在のブランチにupstreamがありません: $branch"
    }
    $sync = ((& git rev-list --left-right --count "$upstream...HEAD").Trim() -split '\s+')
    if ($LASTEXITCODE -ne 0 -or $sync.Count -ne 2 -or $sync[0] -ne '0' -or $sync[1] -ne '0') {
        throw "現在のブランチがupstreamと同期していません: $branch / $upstream (behind=$($sync[0]), ahead=$($sync[1]))"
    }

    $confirm = Read-Host "バージョン $tagName の検証・成果物作成を開始します。続行しますか？ (y/N)"
    if ($confirm -notmatch '^[Yy]$') {
        Write-Host 'キャンセルしました。' -ForegroundColor Gray
        return
    }

    $running = Get-Process -Name focus-translator -ErrorAction SilentlyContinue
    if ($running) {
        $running | Stop-Process -Force
        Write-Host '実行中のFocusTranslatorを停止しました。' -ForegroundColor Gray
    }

    Invoke-NativeChecked 'rustfmtを確認中...' { cargo fmt --all -- --check }
    Invoke-NativeChecked 'debugテストを実行中...' { cargo test --locked }
    Invoke-NativeChecked 'clippyを実行中...' { cargo clippy --all-targets --locked -- -D warnings }
    Invoke-NativeChecked 'releaseビルドを実行中...' { cargo build --release --locked }
    Invoke-NativeChecked 'releaseテストを実行中...' { cargo test --release --locked }

    $osvScanner = Find-OsvScanner
    Invoke-NativeChecked 'OSV依存脆弱性監査を実行中...' {
        & $osvScanner scan source --lockfile Cargo.lock
    }

    $exePath = Join-Path $ProjectRoot 'target\release\focus-translator.exe'
    $directMlPath = Join-Path $ProjectRoot 'target\release\DirectML.dll'
    foreach ($required in @($exePath, $directMlPath, (Join-Path $ProjectRoot 'README.md'))) {
        if (-not (Test-Path -LiteralPath $required)) {
            throw "必須成果物が見つかりません: $required"
        }
    }

    $certificate = Get-ReleaseSigningCertificate
    Set-AndVerifyAuthenticodeSignature -Path $exePath -Certificate $certificate

    $releaseRoot = [System.IO.Path]::GetFullPath((Join-Path $ProjectRoot 'release'))
    $publishDir = [System.IO.Path]::GetFullPath((Join-Path $releaseRoot 'Publish'))
    $stagingDir = [System.IO.Path]::GetFullPath((Join-Path $releaseRoot "staging\focus-translator_$tagName"))
    if (-not $stagingDir.StartsWith($releaseRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "一時フォルダがrelease配下ではありません: $stagingDir"
    }
    if (Test-Path -LiteralPath $stagingDir) {
        Remove-Item -LiteralPath $stagingDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
    New-Item -ItemType Directory -Path $publishDir -Force | Out-Null

    Copy-Item -LiteralPath $exePath -Destination (Join-Path $stagingDir 'focus-translator.exe')
    Copy-Item -LiteralPath $directMlPath -Destination (Join-Path $stagingDir 'DirectML.dll')
    Copy-Item -LiteralPath (Join-Path $ProjectRoot 'README.md') -Destination (Join-Path $stagingDir 'README.md')

    $zipPath = Join-Path $publishDir "focus-translator_$tagName.zip"
    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }
    Compress-Archive -Path (Join-Path $stagingDir '*') -DestinationPath $zipPath -CompressionLevel Optimal

    $isccPath = Find-Iscc
    $setupBaseName = "focus-translator_$($tagName)_Setup"
    $setupPath = Join-Path $publishDir "$setupBaseName.exe"
    if (Test-Path -LiteralPath $setupPath) {
        Remove-Item -LiteralPath $setupPath -Force
    }
    Invoke-NativeChecked 'Inno Setupインストーラを作成中...' {
        & $isccPath "/O$publishDir" "/F$setupBaseName" 'installer.iss'
    }
    if (-not (Test-Path -LiteralPath $setupPath)) {
        throw "インストーラが生成されませんでした: $setupPath"
    }
    Set-AndVerifyAuthenticodeSignature -Path $setupPath -Certificate $certificate

    $checksumPath = Join-Path $publishDir "focus-translator_$($tagName)_SHA256SUMS.txt"
    $checksumLines = foreach ($artifact in @($zipPath, $setupPath)) {
        $hash = Get-FileHash -LiteralPath $artifact -Algorithm SHA256
        "$($hash.Hash.ToLowerInvariant())  $([System.IO.Path]::GetFileName($artifact))"
    }
    Set-Content -LiteralPath $checksumPath -Value $checksumLines -Encoding utf8NoBOM

    Remove-Item -LiteralPath $stagingDir -Recurse -Force
    Write-Host "成果物を作成しました: $publishDir" -ForegroundColor Green

    if ($Publish) {
        if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
            throw 'GitHub CLI (gh) が見つかりません。'
        }
        $headCommit = (& git rev-parse HEAD).Trim()
        $tagCommit = (& git rev-list -n 1 $tagName 2>$null).Trim()
        if ($LASTEXITCODE -ne 0 -or $tagCommit -ne $headCommit) {
            throw "$tagName が現在のHEADを指していません。ローカルタグを作成し、内容を確認してください。"
        }
        & git ls-remote --exit-code --tags origin "refs/tags/$tagName" | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "$tagName がoriginへpushされていません。タグをpushしてから再実行してください。"
        }
        & gh release view $tagName *> $null
        if ($LASTEXITCODE -eq 0) {
            throw "GitHubリリース $tagName は既に存在します。自動上書きは行いません。"
        }

        $assets = @($zipPath, $setupPath, $checksumPath)
        Invoke-NativeChecked 'GitHubへdraftリリースを作成中...' {
            gh release create $tagName $assets `
                --verify-tag `
                --draft `
                --title "なにこれ？（Focus Translator）$tagName" `
                --notes-file $ReleaseNotesPath
        }
        Write-Host "draftリリースを作成しました: $tagName" -ForegroundColor Green
    }
    else {
        Write-Host 'GitHub公開は行っていません。公開する場合は、タグをpush後に-Publishを指定してください。' -ForegroundColor Gray
    }
}
finally {
    Pop-Location
}
