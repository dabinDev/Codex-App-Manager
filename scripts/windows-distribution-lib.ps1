Set-StrictMode -Version Latest

function ConvertFrom-Utf8Base64 {
    param([Parameter(Mandatory = $true)][string]$Value)
    return [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Value))
}

function Get-DistributionBrandProfile {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("acid-orange", "rotten-tomato")]
        [string]$Brand
    )

    if ($Brand -eq "acid-orange") {
        return [pscustomobject]@{
            ProductName = ConvertFrom-Utf8Base64 "6YW45qmY5a2Q5a6J6KOF5Zmo"
            ServiceName = ConvertFrom-Utf8Base64 "dG9rZW4uY3lsb25haS5jbiDCtyBPcGVuQUk="
            ServiceUrl = "https://token.cylonai.cn"
        }
    }
    return [pscustomobject]@{
        ProductName = ConvertFrom-Utf8Base64 "54OC55Wq6IyE5a6J6KOF5Zmo"
        ServiceName = ConvertFrom-Utf8Base64 "dG9tYXRvLmJlaW5haS5jYyDCtyBPcGVuQUk="
        ServiceUrl = "https://tomato.beinai.cc"
    }
}

function ConvertTo-CanonicalJson {
    param([Parameter(Mandatory = $true)]$Value)
    return $Value | ConvertTo-Json -Depth 100 -Compress
}

function Assert-BrandedConfigDelta {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("acid-orange", "rotten-tomato")]
        [string]$Brand,
        [Parameter(Mandatory = $true)][string]$BaseBrandJson,
        [Parameter(Mandatory = $true)][string]$ActualBrandJson,
        [Parameter(Mandatory = $true)][string]$BaseTauriJson,
        [Parameter(Mandatory = $true)][string]$ActualTauriJson
    )

    $profile = Get-DistributionBrandProfile -Brand $Brand
    $expectedBrand = $BaseBrandJson | ConvertFrom-Json
    $expectedBrand.productName = $profile.ProductName
    $expectedBrand.serviceName = $profile.ServiceName
    foreach ($property in $expectedBrand.displayNames.PSObject.Properties) {
        $property.Value = $profile.ProductName
    }
    $expectedBrand.sites.service = $profile.ServiceUrl
    $expectedBrand.icons.frontend.light = "src/app/assets/orange-logo.png"
    $expectedBrand.icons.frontend.dark = "src/app/assets/orange-logo.png"

    $actualBrand = $ActualBrandJson | ConvertFrom-Json
    if ((ConvertTo-CanonicalJson $actualBrand) -cne (ConvertTo-CanonicalJson $expectedBrand)) {
        throw "[preflight][$Brand] brand.config.json is not the approved brand delta"
    }

    $expectedTauri = $BaseTauriJson | ConvertFrom-Json
    $expectedTauri.productName = $profile.ProductName
    foreach ($window in @($expectedTauri.app.windows)) {
        $window.title = $profile.ProductName
    }
    $expectedTauri.bundle.icon = @($expectedBrand.icons.bundle)
    $expectedTauri.bundle.windows.nsis.installerIcon = $expectedBrand.icons.windowsInstaller
    $expectedTauri.plugins.updater.endpoints = @($expectedBrand.sites.updater)

    $actualTauri = $ActualTauriJson | ConvertFrom-Json
    if ((ConvertTo-CanonicalJson $actualTauri) -cne (ConvertTo-CanonicalJson $expectedTauri)) {
        throw "[preflight][$Brand] tauri.conf.json is not the approved brand delta"
    }
    return $profile
}

function Assert-IsolatedGitWorktree {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$WorktreePath)

    $resolved = (Resolve-Path -LiteralPath $WorktreePath -ErrorAction Stop).Path
    $common = (& git -C $resolved rev-parse --path-format=absolute --git-common-dir).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($common)) {
        throw "[preflight] unable to resolve git common directory"
    }
    if (-not [IO.Path]::IsPathRooted($common)) {
        $common = [IO.Path]::GetFullPath((Join-Path $resolved $common))
    }
    $primary = [IO.Path]::GetFullPath((Split-Path -Parent $common)).TrimEnd('\', '/')
    if ([IO.Path]::GetFullPath($resolved).TrimEnd('\', '/') -ieq $primary) {
        throw "[preflight] an isolated git worktree is required; the primary worktree is forbidden"
    }
}

function Get-Sha256 {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Path)

    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    return (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
}

function Copy-ToVerifiedArchive {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$SourcePath,
        [Parameter(Mandatory = $true)][string]$ArchivePath,
        [Parameter(Mandatory = $true)][string]$ExpectedHash
    )

    $source = (Get-Item -LiteralPath $SourcePath -ErrorAction Stop).FullName
    $archive = [IO.Path]::GetFullPath($ArchivePath)
    if ((Get-Sha256 -Path $source) -ne $ExpectedHash) {
        throw "[archive] source SHA-256 changed before archival path=$source"
    }
    if (Test-Path -LiteralPath $archive) {
        if ((Get-Sha256 -Path $archive) -ne $ExpectedHash) {
            throw "[archive] existing archive SHA-256 does not match path=$archive"
        }
        return Get-Item -LiteralPath $archive
    }

    $parent = Split-Path -Parent $archive
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $temporary = Join-Path $parent ("." + [IO.Path]::GetFileName($archive) + ".tmp-" + [guid]::NewGuid().ToString("n"))
    try {
        Copy-Item -LiteralPath $source -Destination $temporary -ErrorAction Stop
        if ((Get-Sha256 -Path $temporary) -ne $ExpectedHash) {
            throw "[archive] copied SHA-256 does not match source path=$temporary"
        }
        Move-Item -LiteralPath $temporary -Destination $archive -ErrorAction Stop
        if ((Get-Sha256 -Path $archive) -ne $ExpectedHash) {
            throw "[archive] finalized SHA-256 does not match source path=$archive"
        }
        return Get-Item -LiteralPath $archive
    }
    finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        }
    }
}

function Restore-VerifiedDelivery {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$DeliveryPath,
        [Parameter(Mandatory = $true)][string]$SwapBackupPath,
        [Parameter(Mandatory = $true)][string]$VerifiedBackupPath,
        [Parameter(Mandatory = $true)][string]$ExpectedOldHash
    )

    $delivery = [IO.Path]::GetFullPath($DeliveryPath)
    $restoreSource = $SwapBackupPath
    $restoreTemporary = $null
    if (-not (Test-Path -LiteralPath $restoreSource)) {
        if (-not (Test-Path -LiteralPath $VerifiedBackupPath)) {
            throw "no verified recovery source remains"
        }
        $restoreTemporary = "$delivery.restore-$([guid]::NewGuid().ToString('n'))"
        Copy-Item -LiteralPath $VerifiedBackupPath -Destination $restoreTemporary -ErrorAction Stop
        $restoreSource = $restoreTemporary
    }
    if ((Get-Sha256 -Path $restoreSource) -ne $ExpectedOldHash) {
        throw "recovery source SHA-256 mismatch path=$restoreSource"
    }

    $rejected = "$delivery.rejected-$([guid]::NewGuid().ToString('n'))"
    try {
        [IO.File]::Replace($restoreSource, $delivery, $rejected, $true)
        if ((Get-Sha256 -Path $delivery) -ne $ExpectedOldHash) {
            throw "restored delivery SHA-256 mismatch path=$delivery"
        }
        Remove-Item -LiteralPath $rejected -Force -ErrorAction SilentlyContinue
    }
    finally {
        if ($restoreTemporary -and (Test-Path -LiteralPath $restoreTemporary)) {
            Remove-Item -LiteralPath $restoreTemporary -Force -ErrorAction SilentlyContinue
        }
    }
}

function Publish-VerifiedDelivery {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$ArchivePath,
        [Parameter(Mandatory = $true)][string]$DeliveryPath,
        [Parameter(Mandatory = $true)][string]$ExpectedHash,
        [scriptblock]$AfterReplaceForTest,
        [scriptblock]$RestoreActionForTest
    )

    $archive = (Get-Item -LiteralPath $ArchivePath -ErrorAction Stop).FullName
    if ((Get-Sha256 -Path $archive) -ne $ExpectedHash) {
        throw "[desktop-copy] archive SHA-256 changed before delivery path=$archive"
    }
    $delivery = [IO.Path]::GetFullPath($DeliveryPath)
    $parent = Split-Path -Parent $delivery
    New-Item -ItemType Directory -Path $parent -Force | Out-Null

    $backup = $null
    $oldHash = $null
    if (Test-Path -LiteralPath $delivery) {
        $oldHash = Get-Sha256 -Path $delivery
        $backup = "$delivery.bak-$(Get-Date -Format 'yyyyMMdd-HHmmssfff')"
        Copy-Item -LiteralPath $delivery -Destination $backup -ErrorAction Stop
        if ((Get-Sha256 -Path $backup) -ne $oldHash) {
            Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
            throw "[desktop-copy] existing delivery backup SHA-256 mismatch path=$backup"
        }
    }

    $temporary = Join-Path $parent ("." + [IO.Path]::GetFileName($delivery) + ".tmp-" + [guid]::NewGuid().ToString("n"))
    $swapBackup = $null
    $preserveRecovery = $false
    try {
        Copy-Item -LiteralPath $archive -Destination $temporary -ErrorAction Stop
        if ((Get-Sha256 -Path $temporary) -ne $ExpectedHash) {
            throw "[desktop-copy] staged SHA-256 mismatch path=$temporary"
        }
        if (Test-Path -LiteralPath $delivery) {
            $swapBackup = "$delivery.swap-$([guid]::NewGuid().ToString('n'))"
            [IO.File]::Replace($temporary, $delivery, $swapBackup, $true)
        }
        else {
            Move-Item -LiteralPath $temporary -Destination $delivery -ErrorAction Stop
        }
        if ($AfterReplaceForTest) {
            & $AfterReplaceForTest $delivery
        }
        if ((Get-Sha256 -Path $delivery) -ne $ExpectedHash) {
            if ($oldHash) {
                try {
                    if ($RestoreActionForTest) {
                        & $RestoreActionForTest $delivery $swapBackup $backup $oldHash
                    }
                    else {
                        Restore-VerifiedDelivery -DeliveryPath $delivery `
                            -SwapBackupPath $swapBackup -VerifiedBackupPath $backup `
                            -ExpectedOldHash $oldHash
                    }
                    if ((Get-Sha256 -Path $delivery) -ne $oldHash) {
                        throw "restored delivery SHA-256 mismatch"
                    }
                }
                catch {
                    $preserveRecovery = $true
                    throw "[desktop-copy] delivered SHA-256 mismatch and automatic restore failed: $($_.Exception.Message) backup=$backup swap=$swapBackup"
                }
            }
            else {
                try {
                    Remove-Item -LiteralPath $delivery -Force -ErrorAction Stop
                }
                catch {
                    throw "[desktop-copy] delivered SHA-256 mismatch and unverified first delivery could not be removed: $($_.Exception.Message) path=$delivery"
                }
                if (Test-Path -LiteralPath $delivery) {
                    throw "[desktop-copy] delivered SHA-256 mismatch and unverified first delivery still exists path=$delivery"
                }
            }
            throw "[desktop-copy] delivered SHA-256 mismatch path=$delivery"
        }
        if (-not $preserveRecovery -and $swapBackup -and (Test-Path -LiteralPath $swapBackup)) {
            Remove-Item -LiteralPath $swapBackup -Force -ErrorAction SilentlyContinue
        }
    }
    finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        }
        if (-not $preserveRecovery -and $swapBackup -and (Test-Path -LiteralPath $swapBackup)) {
            Remove-Item -LiteralPath $swapBackup -Force -ErrorAction SilentlyContinue
        }
    }

    return [pscustomobject]@{
        DeliveryPath = $delivery
        BackupPath = $backup
        Sha256 = $ExpectedHash
    }
}
