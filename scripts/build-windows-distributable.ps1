[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("acid-orange", "rotten-tomato")]
    [string]$Brand,

    [Parameter(Mandatory = $true)][string]$Worktree,
    [Parameter(Mandatory = $true)][string]$ExpectedCommit,
    [Parameter(Mandatory = $true)][string]$DeliveryDirectory,
    [string]$ArtifactRoot = "E:\ForkProject\BuildArtifacts\Codex-App-Manager",
    [string]$ExpectedPublisher = $env:WINDOWS_EXPECTED_PUBLISHER,
    [string]$CertificateBase64 = $env:WINDOWS_CERTIFICATE,
    [string]$CertificatePassword = $env:WINDOWS_CERTIFICATE_PASSWORD,
    [string]$TimestampUrl = $(if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { "http://timestamp.digicert.com" })
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptRoot = $PSScriptRoot
. (Join-Path $scriptRoot "windows-distribution-lib.ps1")

function Fail-Distribution([string]$Stage, [string]$Message, [string]$Path = "") {
    $suffix = if ($Path) { " path=$Path" } else { "" }
    throw "[$Stage][$Brand] $Message$suffix"
}

function Assert-LastExit([string]$Stage, [string]$Action) {
    if ($LASTEXITCODE -ne 0) {
        Fail-Distribution $Stage "$Action failed (exit=$LASTEXITCODE)"
    }
}

function Get-PeMachine([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) { Fail-Distribution "sign-verify" "invalid DOS header" $Path }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) { Fail-Distribution "sign-verify" "invalid PE header" $Path }
        return $reader.ReadUInt16()
    }
    finally {
        $stream.Dispose()
    }
}

if ([string]::IsNullOrWhiteSpace($CertificateBase64)) {
    Fail-Distribution "preflight" "trusted code-signing certificate is required"
}
if ([string]::IsNullOrWhiteSpace($ExpectedPublisher)) {
    Fail-Distribution "preflight" "expected publisher identity is required"
}

$worktreePath = (Resolve-Path -LiteralPath $Worktree -ErrorAction Stop).Path
Assert-IsolatedGitWorktree -WorktreePath $worktreePath
$head = (& git -C $worktreePath rev-parse HEAD).Trim()
Assert-LastExit "preflight" "read worktree HEAD"
if ($head -ne $ExpectedCommit) {
    Fail-Distribution "preflight" "worktree HEAD '$head' does not equal requested commit '$ExpectedCommit'" $worktreePath
}

$allowedDirty = @("brand.config.json", "src-tauri/tauri.conf.json")
$dirty = @(& git -C $worktreePath status --porcelain=v1 | ForEach-Object { $_.Substring(3).Replace("\", "/") })
$unexpectedDirty = @($dirty | Where-Object { $_ -and $_ -notin $allowedDirty })
if ($unexpectedDirty.Count -gt 0) {
    Fail-Distribution "preflight" "worktree has unrelated changes: $($unexpectedDirty -join ', ')" $worktreePath
}

Push-Location $worktreePath
try {
    & npm.cmd run brand:sync
    Assert-LastExit "brand-sync" "brand synchronization"

    $baseBrandJson = (& git -C $worktreePath show "$ExpectedCommit`:brand.config.json") -join "`n"
    Assert-LastExit "preflight" "read base brand configuration"
    $baseTauriJson = (& git -C $worktreePath show "$ExpectedCommit`:src-tauri/tauri.conf.json") -join "`n"
    Assert-LastExit "preflight" "read base Tauri configuration"
    $expectedBrand = Assert-BrandedConfigDelta -Brand $Brand `
        -BaseBrandJson $baseBrandJson `
        -ActualBrandJson (Get-Content -LiteralPath (Join-Path $worktreePath "brand.config.json") -Raw -Encoding UTF8) `
        -BaseTauriJson $baseTauriJson `
        -ActualTauriJson (Get-Content -LiteralPath (Join-Path $worktreePath "src-tauri\tauri.conf.json") -Raw -Encoding UTF8)

    & npm.cmd run check
    Assert-LastExit "brand-sync" "brand validation"

    $package = Get-Content -LiteralPath (Join-Path $worktreePath "package.json") -Raw -Encoding UTF8 |
        ConvertFrom-Json
    $version = [string]$package.version
    $rustTarget = "x86_64-pc-windows-msvc"
    $targetRoot = Join-Path $worktreePath "src-tauri\target\$rustTarget\release"
    $buildStarted = [DateTime]::UtcNow.AddSeconds(-2)
    & npm.cmd run tauri -- build --no-bundle --target $rustTarget --features tauri/custom-protocol
    Assert-LastExit "build" "Tauri production binary build"

    $mainExecutable = Get-Item -LiteralPath (Join-Path $targetRoot "codex-app-manager.exe") `
        -ErrorAction Stop
    if ($mainExecutable.LastWriteTimeUtc -lt $buildStarted) {
        Fail-Distribution "build" "main executable was not produced by this build" $mainExecutable.FullName
    }
    if ((Get-PeMachine $mainExecutable.FullName) -ne 0x8664) {
        Fail-Distribution "sign-verify" "built main executable is not x64" $mainExecutable.FullName
    }

    $stringsTool = Get-Command llvm-strings.exe, llvm-strings -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $stringsTool) {
        Fail-Distribution "sign-verify" "llvm-strings is required for production embedding verification"
    }
    $embedded = (& $stringsTool.Source $mainExecutable.FullName) -join "`n"
    if (-not $embedded.Contains("/index.html") -or -not $embedded.Contains($expectedBrand.ServiceUrl)) {
        Fail-Distribution "sign-verify" "production renderer or brand URL is not embedded" $mainExecutable.FullName
    }

    & (Join-Path $scriptRoot "sign-windows-authenticode.ps1") -Path $mainExecutable.FullName `
        -Mode required -CertificateBase64 $CertificateBase64 `
        -CertificatePassword $CertificatePassword -TimestampUrl $TimestampUrl -Stage "sign"
    & (Join-Path $scriptRoot "verify-windows-authenticode.ps1") -Path $mainExecutable.FullName `
        -Mode required -ExpectedSubject $ExpectedPublisher -Stage "sign-verify"
    $mainSignature = Get-AuthenticodeSignature -LiteralPath $mainExecutable.FullName
    if (-not $mainSignature.TimeStamperCertificate) {
        Fail-Distribution "sign-verify" "main executable signature has no trusted timestamp" $mainExecutable.FullName
    }

    & npm.cmd run tauri -- bundle --bundles nsis --target $rustTarget --features tauri/custom-protocol
    Assert-LastExit "build" "Tauri NSIS bundle"

    $bundleDirectory = Join-Path $targetRoot "bundle\nsis"
    $installers = @(Get-ChildItem -LiteralPath $bundleDirectory -File -Filter "*-setup.exe" |
        Where-Object { $_.LastWriteTimeUtc -ge $mainExecutable.LastWriteTimeUtc })
    if ($installers.Count -ne 1) {
        Fail-Distribution "build" "expected exactly one NSIS installer from the verified main executable, found $($installers.Count)" $bundleDirectory
    }
    $installer = $installers[0]
    $installerInfo = [Diagnostics.FileVersionInfo]::GetVersionInfo($installer.FullName)
    if ($installerInfo.ProductName -cne $expectedBrand.ProductName -or
        $installerInfo.ProductVersion -notlike "$version*") {
        Fail-Distribution "sign-verify" "installer product name or version does not match the approved build" $installer.FullName
    }

    & (Join-Path $scriptRoot "sign-windows-authenticode.ps1") -Path $installer.FullName `
        -Mode required -CertificateBase64 $CertificateBase64 `
        -CertificatePassword $CertificatePassword -TimestampUrl $TimestampUrl -Stage "sign"
    & (Join-Path $scriptRoot "verify-windows-authenticode.ps1") -Path $installer.FullName `
        -Mode required -ExpectedSubject $ExpectedPublisher -Stage "sign-verify"
    $signature = Get-AuthenticodeSignature -LiteralPath $installer.FullName
    if (-not $signature.TimeStamperCertificate) {
        Fail-Distribution "sign-verify" "installer signature has no trusted timestamp" $installer.FullName
    }

    $sourceHash = Get-Sha256 -Path $installer.FullName
    $archiveDirectory = Join-Path (Join-Path (Join-Path $ArtifactRoot $ExpectedCommit) $Brand) "signed"
    $deliveryName = "$($expectedBrand.ProductName)-$version-setup.exe"
    $archivePath = Join-Path $archiveDirectory $deliveryName
    $deliveryPath = Join-Path ([IO.Path]::GetFullPath($DeliveryDirectory)) $deliveryName
    Copy-ToVerifiedArchive -SourcePath $installer.FullName -ArchivePath $archivePath `
        -ExpectedHash $sourceHash | Out-Null
    & (Join-Path $scriptRoot "verify-windows-authenticode.ps1") -Path $archivePath `
        -Mode required -ExpectedSubject $ExpectedPublisher -Stage "archive"
    $archiveSignature = Get-AuthenticodeSignature -LiteralPath $archivePath
    if (-not $archiveSignature.TimeStamperCertificate -or (Get-Sha256 -Path $archivePath) -ne $sourceHash) {
        Fail-Distribution "archive" "archived signature, timestamp, or SHA-256 does not match" $archivePath
    }
    $delivery = Publish-VerifiedDelivery -ArchivePath $archivePath -DeliveryPath $deliveryPath `
        -ExpectedHash $sourceHash

    [pscustomobject]@{
        productName = $expectedBrand.ProductName
        version = $version
        publisher = $signature.SignerCertificate.Subject
        commit = $ExpectedCommit
        archivePath = $archivePath
        deliveryPath = $delivery.DeliveryPath
        sha256 = $sourceHash
    } | ConvertTo-Json -Compress
}
finally {
    Pop-Location
}
