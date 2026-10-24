# Windows x64 packaged lifecycle smoke test.
#
# Stages (each failure message is prefixed so CI logs pin the phase):
#   build     — caller already built; this script only consumes the installer
#   install   — passive NSIS install (/P)
#   launch    — first start of the installed main executable
#   upgrade   — re-run installer with /P /UPDATE (in-place upgrade path)
#   uninstall — passive uninstall of the installed product (always attempted in finally)
#   sign-verify — Authenticode probe on installer + installed PE files
#                 (optional/required/skip via -AuthenticodeMode; pass
#                 -ExpectedThumbprint to also assert a specific certificate
#                 signed them — this is the only place the uninstaller can be
#                 checked at all, since it does not exist as a file until
#                 install actually writes it)
#
# Usage (prefer in-process from CI shell:pwsh steps — nested `pwsh -File` breaks
# array binding for -Path on some runners):
#   & .\scripts\windows-packaged-smoke.ps1 -Installer path\to\*-setup.exe
#   & .\scripts\windows-packaged-smoke.ps1 -Installer path\to\*-setup.exe `
#       -AuthenticodeMode required -ExpectedThumbprint $thumbprint
#
# Safe for CI: currentUser installMode → %LOCALAPPDATA%\<configured product name>
# (no admin elevation). Kills the app between stages. Does not touch ~/.codex.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,

    [string]$ProductName = "",
    [string]$MainBinaryName = "codex-app-manager",
    [int]$LaunchSeconds = 12,
    [ValidateSet("optional", "required", "skip")]
    [string]$AuthenticodeMode = "optional",

    # Forwarded to verify-windows-authenticode.ps1's -ExpectedThumbprint on
    # both sign-verify probes (installer, and installed exe + uninstaller).
    # Lets a caller assert every PE — including the uninstaller, which only
    # exists once actually installed, which is exactly what this script
    # does — was signed by one specific certificate, not just "signed by
    # something" or "Valid". Empty (default) is a no-op, matching prior
    # behavior exactly.
    [string]$ExpectedThumbprint = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ProductName)) {
    $brandConfigPath = Join-Path (Split-Path -Parent $PSScriptRoot) "brand.config.json"
    $brand = Get-Content -LiteralPath $brandConfigPath -Raw -ErrorAction Stop |
        ConvertFrom-Json -ErrorAction Stop
    $ProductName = [string]$brand.productName
}
if ([string]::IsNullOrWhiteSpace($ProductName)) {
    throw "brand.config.json productName must be a non-empty string"
}

function Write-Stage([string]$Stage, [string]$Message) {
    Write-Host "::group::[$Stage] $Message"
}

function Close-Stage {
    Write-Host "::endgroup::"
}

function Fail-Stage([string]$Stage, [string]$Message) {
    Write-Host "::error::[$Stage] $Message"
    throw "[$Stage] $Message"
}

function Stop-AppProcesses([string]$BinaryName) {
    Get-Process -Name $BinaryName -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "Stopping process $($_.Id) ($($_.ProcessName))"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds 2
}

function Test-AbsoluteWindowsPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $false
    }
    return $Path -match '^[A-Za-z]:[\\/]' -or
        $Path -match '^[\\/]{2}[^\\/]+[\\/]+[^\\/]+(?:[\\/]|$)'
}

function Get-SharedFileHash([string]$Path) {
    $stream = [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
    )
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha256.ComputeHash($stream))).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

function Get-RolloutHeadHash([string]$Path) {
    $stream = [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
    )
    $head = [System.IO.MemoryStream]::new()
    try {
        $buffer = [byte[]]::new(8192)
        $done = $false
        while (-not $done) {
            $read = $stream.Read($buffer, 0, $buffer.Length)
            if ($read -eq 0) {
                break
            }
            $length = $read
            for ($index = 0; $index -lt $read; $index++) {
                if ($buffer[$index] -eq 10) {
                    $length = $index + 1
                    $done = $true
                    break
                }
            }
            $head.Write($buffer, 0, $length)
        }
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            return ([BitConverter]::ToString($sha256.ComputeHash($head.ToArray()))).Replace("-", "").ToLowerInvariant()
        }
        finally {
            $sha256.Dispose()
        }
    }
    finally {
        $head.Dispose()
        $stream.Dispose()
    }
}

function Get-ProtectedCodexSnapshot([string[]]$CodexHomes) {
    $entries = [System.Collections.Generic.List[object]]::new()
    $resolvedHomes = @(
        $CodexHomes |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object { [System.IO.Path]::GetFullPath($_) } |
            Sort-Object -Unique
    )
    foreach ($codexHome in $resolvedHomes) {
        $homeExists = Test-Path -LiteralPath $codexHome -PathType Container
        $entries.Add([pscustomobject]@{
                Key = "root::$codexHome"
                Hash = if ($homeExists) { "present" } else { "missing" }
            })
        foreach ($name in @("config.toml", "auth.json")) {
            $path = Join-Path $codexHome $name
            $entries.Add([pscustomobject]@{
                    Key = "file::$path"
                    Hash = if (Test-Path -LiteralPath $path -PathType Leaf) {
                        Get-SharedFileHash $path
                    }
                    else {
                        "missing"
                    }
                })
        }
        if (-not $homeExists) {
            continue
        }
        foreach ($databaseRoot in @($codexHome, (Join-Path $codexHome "sqlite"))) {
            if (-not (Test-Path -LiteralPath $databaseRoot -PathType Container)) {
                continue
            }
            Get-ChildItem -LiteralPath $databaseRoot -Filter "state_*.sqlite" -File -Force -ErrorAction Stop |
                ForEach-Object {
                    $databasePath = $_.FullName
                    $entries.Add([pscustomobject]@{
                            Key = "database::$databasePath"
                            Hash = Get-SharedFileHash $databasePath
                        })
                    foreach ($sidecarSuffix in @("-wal", "-shm", "-journal")) {
                        $sidecarPath = "$databasePath$sidecarSuffix"
                        $entries.Add([pscustomobject]@{
                                Key = "database-sidecar::$sidecarPath"
                                Hash = if (Test-Path -LiteralPath $sidecarPath -PathType Leaf) {
                                    Get-SharedFileHash $sidecarPath
                                }
                                else {
                                    "missing"
                                }
                            })
                    }
                }
        }
        foreach ($rolloutRoot in @(
                (Join-Path $codexHome "sessions"),
                (Join-Path $codexHome "archived_sessions")
            )) {
            if (-not (Test-Path -LiteralPath $rolloutRoot -PathType Container)) {
                continue
            }
            Get-ChildItem -LiteralPath $rolloutRoot -Filter "rollout-*.jsonl" -File -Recurse -Force -ErrorAction Stop |
                ForEach-Object {
                    $entries.Add([pscustomobject]@{
                            Key = "rollout-head::$($_.FullName)"
                            Hash = Get-RolloutHeadHash $_.FullName
                        })
                }
        }
        $recoveryRoot = Join-Path $codexHome "recovery-backups"
        $recoveryRootIsDirectory = Test-Path -LiteralPath $recoveryRoot -PathType Container
        $entries.Add([pscustomobject]@{
                Key = "recovery-root::$recoveryRoot"
                Hash = if ($recoveryRootIsDirectory) {
                    "directory"
                }
                elseif (Test-Path -LiteralPath $recoveryRoot -PathType Leaf) {
                    "file::" + (Get-SharedFileHash $recoveryRoot)
                }
                else {
                    "missing"
                }
            })
        if ($recoveryRootIsDirectory) {
            Get-ChildItem -LiteralPath $recoveryRoot -Recurse -Force -ErrorAction Stop |
                ForEach-Object {
                    $entries.Add([pscustomobject]@{
                            Key = "recovery::$($_.FullName)"
                            Hash = if ($_.PSIsContainer) {
                                "directory"
                            }
                            else {
                                Get-SharedFileHash $_.FullName
                            }
                        })
                }
        }
    }
    return @($entries | Sort-Object Key)
}

function Assert-ProtectedCodexSnapshot([object[]]$Expected, [string[]]$CodexHomes) {
    $actual = @(Get-ProtectedCodexSnapshot $CodexHomes)
    $expectedJson = ConvertTo-Json -InputObject @($Expected) -Depth 3 -Compress
    $actualJson = ConvertTo-Json -InputObject $actual -Depth 3 -Compress
    if ($actualJson -cne $expectedJson) {
        Fail-Stage "alignment" "real Codex config, auth, database, rollout head, or recovery backup changed during isolated smoke"
    }
}

function New-AlignmentFixture(
    [string]$CodexHome,
    [string]$Database,
    [string]$Rollout,
    [string]$InvalidRollout
) {
    New-Item -ItemType Directory -Path $CodexHome -ErrorAction Stop | Out-Null
    New-Item -ItemType Directory -Path (Split-Path -Parent $Rollout) -ErrorAction Stop | Out-Null
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText(
        (Join-Path $CodexHome "config.toml"),
        "model_provider = `"openai`"`n",
        $utf8NoBom
    )
    $sqliteFixture = @'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
connection.executescript("""
CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    model_provider TEXT,
    archived INTEGER NOT NULL DEFAULT 0
);
INSERT INTO threads (id, model_provider, archived)
VALUES ('packaged-smoke-thread', 'legacy', 7);
""")
connection.commit()
connection.close()
'@
    $sqliteFixture | & $script:SmokePython @script:SmokePythonPrefix - $Database
    if ($LASTEXITCODE -ne 0) {
        throw "failed to create packaged alignment SQLite fixture"
    }
    $rolloutFixture = @(
        '{"timestamp":"2026-07-30T00:00:00Z","type":"session_meta","payload":{"id":"packaged-smoke-session","model_provider":"legacy","cwd":"C:/packaged-smoke"}}',
        '{"type":"response_item","payload":{"sentinel":"tail-preserved"}}'
    ) -join "`n"
    [System.IO.File]::WriteAllText($Rollout, "$rolloutFixture`n", $utf8NoBom)
    [System.IO.File]::WriteAllBytes(
        $InvalidRollout,
        $utf8NoBom.GetBytes("{invalid-packaged-smoke-rollout`nexact-invalid-tail`r`n")
    )
}

function Test-AlignmentFixtureRestored(
    [string]$Database,
    [string]$Rollout,
    [string]$InvalidRollout,
    [string]$InvalidRolloutHash,
    [string]$BackupRoot
) {
    $verifyFixture = @'
import glob
import hashlib
import json
import os
import sqlite3
import sys

database, rollout, invalid_rollout, invalid_rollout_hash, backups = sys.argv[1:]
if not glob.glob(os.path.join(backups, "*", "manifest.json")):
    raise SystemExit(1)
connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
row = connection.execute(
    "SELECT model_provider, archived FROM threads WHERE id = ?",
    ("packaged-smoke-thread",),
).fetchone()
connection.close()
if row != ("legacy", 7):
    raise SystemExit(1)
with open(rollout, "r", encoding="utf-8") as handle:
    lines = handle.read().splitlines()
if len(lines) != 2:
    raise SystemExit(1)
head = json.loads(lines[0])
tail = json.loads(lines[1])
if head.get("payload", {}).get("model_provider") != "legacy":
    raise SystemExit(1)
if tail.get("payload", {}).get("sentinel") != "tail-preserved":
    raise SystemExit(1)
with open(invalid_rollout, "rb") as handle:
    if hashlib.sha256(handle.read()).hexdigest() != invalid_rollout_hash:
        raise SystemExit(1)
'@
    $verifyFixture | & $script:SmokePython @script:SmokePythonPrefix - `
        $Database $Rollout $InvalidRollout $InvalidRolloutHash $BackupRoot 2>$null
    return $LASTEXITCODE -eq 0
}

function Wait-AlignmentSmokeResult(
    [string]$ResultPath,
    [string]$RunId,
    [int]$TimeoutSeconds = 35
) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $ResultPath -PathType Leaf) {
            try {
                $result = Get-Content -LiteralPath $ResultPath -Raw -ErrorAction Stop |
                    ConvertFrom-Json -ErrorAction Stop
            }
            catch {
                Fail-Stage "alignment" "invalid packaged alignment result: $($_.Exception.Message)"
            }
            $schema = $result.PSObject.Properties["schemaVersion"]
            $resultRunId = $result.PSObject.Properties["runId"]
            $status = $result.PSObject.Properties["status"]
            if ($null -eq $schema -or $schema.Value -ne 1 -or
                $null -eq $resultRunId -or $resultRunId.Value -cne $RunId -or
                $null -eq $status -or $status.Value -notin @("passed", "failed")) {
                Fail-Stage "alignment" "packaged alignment result did not match this smoke run"
            }
            if ($status.Value -eq "failed") {
                $message = $result.PSObject.Properties["message"]
                $detail = if ($null -ne $message -and $message.Value -is [string]) {
                    $message.Value
                }
                else {
                    "frontend reported an unspecified failure"
                }
                Fail-Stage "alignment" $detail
            }
            return $result
        }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

function Invoke-Installer([string]$Stage, [string]$Exe, [string[]]$InstallerArgs) {
    Write-Host "[$Stage] Running: $Exe $($InstallerArgs -join ' ')"
    $p = Start-Process -FilePath $Exe -ArgumentList $InstallerArgs -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        Fail-Stage $Stage "installer/uninstaller exited $($p.ExitCode)"
    }
}

function Invoke-UninstallBestEffort([string]$Stage, [string]$UninstallerPath, [string]$BinaryName, [string]$MainExePath) {
    Stop-AppProcesses $BinaryName
    if (-not (Test-Path -LiteralPath $UninstallerPath)) {
        Write-Host "[$Stage] uninstaller not present; nothing to clean"
        return
    }
    Write-Host "[$Stage] Running: $UninstallerPath /P"
    $p = Start-Process -FilePath $UninstallerPath -ArgumentList @("/P") -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        Write-Host "::warning::[$Stage] uninstaller exited $($p.ExitCode)"
    }
    Start-Sleep -Seconds 2
    if (Test-Path -LiteralPath $MainExePath) {
        Write-Host "::warning::[$Stage] main executable still present after uninstall attempt: $MainExePath"
    }
    else {
        Write-Host "[$Stage] main executable removed"
    }
}

$installerItem = Get-Item -LiteralPath $Installer -ErrorAction Stop
$installDir = Join-Path $env:LOCALAPPDATA $ProductName
$mainExe = Join-Path $installDir "$MainBinaryName.exe"
$uninstaller = Join-Path $installDir "uninstall.exe"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$verifyScript = Join-Path $scriptRoot "verify-windows-authenticode.ps1"
$didInstall = $false
$smokeFailed = $false
$smokeError = $null
$runId = "$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())-$PID-$(Get-Random)"
$smokeTempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
)
$smokeDataDir = Join-Path $smokeTempRoot "codex-app-manager-smoke-$runId"
$smokeCodexHome = Join-Path $smokeDataDir "codex-home"
$smokeDatabase = Join-Path $smokeCodexHome "state-packaged-smoke.sqlite"
$smokeRollout = Join-Path $smokeCodexHome "sessions\rollout-packaged-smoke.jsonl"
$smokeInvalidRollout = Join-Path $smokeCodexHome "sessions\rollout-invalid-packaged-smoke.jsonl"
$smokeResult = Join-Path $smokeDataDir "alignment-smoke-result.json"
$smokeDirCreated = $false
$invalidRolloutHash = $null
$protectedCodexSnapshot = $null
$protectedCodexHomes = [System.Collections.Generic.List[string]]::new()
if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
    $protectedCodexHomes.Add((Join-Path $env:USERPROFILE ".codex"))
}
if (-not [string]::IsNullOrWhiteSpace($env:CODEX_HOME) -and
    (Test-AbsoluteWindowsPath $env:CODEX_HOME)) {
    $protectedCodexHomes.Add($env:CODEX_HOME)
}
$smokeEnvironmentNames = @(
    "CAM_PACKAGED_SMOKE_RUN",
    "CAM_PACKAGED_SMOKE_DATA_DIR",
    "CAM_PACKAGED_SMOKE_CODEX_HOME"
)
$previousSmokeEnvironment = @{}
foreach ($name in $smokeEnvironmentNames) {
    $previousSmokeEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

Write-Host "Installer: $($installerItem.FullName)"
Write-Host "InstallDir: $installDir"
Write-Host "MainExe: $mainExe"

try {
    $python = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($python) {
        $script:SmokePython = $python.Source
        $script:SmokePythonPrefix = @()
    }
    else {
        $python = Get-Command py.exe -ErrorAction SilentlyContinue
        if (-not $python) {
            Fail-Stage "alignment" "Python 3 is required to create the isolated SQLite fixture"
        }
        $script:SmokePython = $python.Source
        $script:SmokePythonPrefix = @("-3")
    }
    New-Item -ItemType Directory -Path $smokeDataDir -ErrorAction Stop | Out-Null
    $smokeDirCreated = $true
    $resolvedSmokeDataDir = [System.IO.Path]::GetFullPath($smokeDataDir)
    if ([System.IO.Path]::GetDirectoryName($resolvedSmokeDataDir) -ine $smokeTempRoot -or
        [System.IO.Path]::GetFileName($resolvedSmokeDataDir) -cne "codex-app-manager-smoke-$runId") {
        Fail-Stage "alignment" "isolated smoke root did not match the validated temp child"
    }
    New-AlignmentFixture $smokeCodexHome $smokeDatabase $smokeRollout $smokeInvalidRollout
    $smokeRootItem = Get-Item -LiteralPath $smokeDataDir -Force -ErrorAction Stop
    $smokeHomeItem = Get-Item -LiteralPath $smokeCodexHome -Force -ErrorAction Stop
    if (($smokeRootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($smokeHomeItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $smokeHomeItem.Parent.FullName -ine $smokeRootItem.FullName) {
        Fail-Stage "alignment" "isolated Codex home was not a direct non-link child of the smoke root"
    }
    $invalidRolloutHash = (
        Get-FileHash -LiteralPath $smokeInvalidRollout -Algorithm SHA256 -ErrorAction Stop
    ).Hash.ToLowerInvariant()
    [Environment]::SetEnvironmentVariable("CAM_PACKAGED_SMOKE_RUN", $runId, "Process")
    [Environment]::SetEnvironmentVariable("CAM_PACKAGED_SMOKE_DATA_DIR", $smokeDataDir, "Process")
    [Environment]::SetEnvironmentVariable("CAM_PACKAGED_SMOKE_CODEX_HOME", $smokeCodexHome, "Process")
    if ([Environment]::GetEnvironmentVariable("CAM_PACKAGED_SMOKE_RUN", "Process") -cne $runId -or
        [Environment]::GetEnvironmentVariable("CAM_PACKAGED_SMOKE_DATA_DIR", "Process") -cne $smokeDataDir -or
        [Environment]::GetEnvironmentVariable("CAM_PACKAGED_SMOKE_CODEX_HOME", "Process") -cne $smokeCodexHome) {
        Fail-Stage "alignment" "isolated smoke markers were not installed in the launch environment"
    }
    $protectedCodexSnapshot = @(
        Get-ProtectedCodexSnapshot $protectedCodexHomes.ToArray()
    )

    # ── sign-verify (installer artifact, pre-install) ───────────────────────
    if ($AuthenticodeMode -ne "skip" -and (Test-Path $verifyScript)) {
        Write-Stage "sign-verify" "Probe Authenticode on installer ($AuthenticodeMode)"
        & $verifyScript -Path $installerItem.FullName -Mode $AuthenticodeMode -ExpectedThumbprint $ExpectedThumbprint -Stage "sign-verify"
        Close-Stage
    }

    # Clean slate if a previous run left leftovers.
    if (Test-Path $uninstaller) {
        Write-Stage "install" "Removing leftover install from a previous run"
        Invoke-UninstallBestEffort "install" $uninstaller $MainBinaryName $mainExe
        Close-Stage
    }

    # ── install ─────────────────────────────────────────────────────────────
    Write-Stage "install" "Passive install (/P)"
    Stop-AppProcesses $MainBinaryName
    Invoke-Installer "install" $installerItem.FullName @("/P")
    $didInstall = $true

    if (-not (Test-Path -LiteralPath $mainExe)) {
        Fail-Stage "install" "main executable missing after install: $mainExe"
    }
    if (-not (Test-Path -LiteralPath $uninstaller)) {
        Fail-Stage "install" "uninstaller missing after install: $uninstaller"
    }

    $vi = (Get-Item -LiteralPath $mainExe).VersionInfo
    Write-Host "[install] FileVersion=$($vi.FileVersion) ProductVersion=$($vi.ProductVersion)"
    Write-Host "[install] installed PE size=$((Get-Item -LiteralPath $mainExe).Length) bytes"
    Close-Stage

    # ── sign-verify (installed PE) ──────────────────────────────────────────
    if ($AuthenticodeMode -ne "skip" -and (Test-Path $verifyScript)) {
        Write-Stage "sign-verify" "Probe Authenticode on installed executable + uninstaller"
        & $verifyScript -Path @($mainExe, $uninstaller) -Mode $AuthenticodeMode -ExpectedThumbprint $ExpectedThumbprint -Stage "sign-verify"
        Close-Stage
    }

    # ── launch ──────────────────────────────────────────────────────────────
    Write-Stage "launch" "First launch (${LaunchSeconds}s observe window)"
    Stop-AppProcesses $MainBinaryName

    $proc = Start-Process -FilePath $mainExe -PassThru -WindowStyle Minimized
    Start-Sleep -Seconds $LaunchSeconds

    if ($proc.HasExited) {
        # A GUI app exiting immediately with non-zero is a real failure. Exit 0 can
        # happen if single-instance hands off, but a brand-new install should stay up.
        if ($proc.ExitCode -ne 0) {
            Fail-Stage "launch" "process exited early with code $($proc.ExitCode)"
        }
        Write-Host "::warning::[launch] process exited during observe window with code 0 — treating as soft pass"
    }
    else {
        Write-Host "[launch] process still running (pid=$($proc.Id)) — first launch OK"
    }
    Close-Stage

    Write-Stage "alignment" "Exercise packaged scan, verified write, and verified restore IPC"
    $alignmentResult = Wait-AlignmentSmokeResult $smokeResult $runId
    if ($null -eq $alignmentResult) {
        Fail-Stage "alignment" "timed out waiting for an explicit alignment smoke result"
    }
    if (-not (Test-AlignmentFixtureRestored `
            $smokeDatabase `
            $smokeRollout `
            $smokeInvalidRollout `
            $invalidRolloutHash `
            (Join-Path $smokeCodexHome "recovery-backups"))) {
        Fail-Stage "alignment" "passed result did not leave the isolated fixture fully restored"
    }
    Assert-ProtectedCodexSnapshot $protectedCodexSnapshot $protectedCodexHomes.ToArray()
    Write-Host "[alignment] codex_home=$smokeCodexHome database=restored rollouts=restored real_codex=unchanged"
    Close-Stage

    # Ensure nothing leftover holds files open for upgrade.
    Stop-AppProcesses $MainBinaryName

    # ── upgrade ─────────────────────────────────────────────────────────────
    Write-Stage "upgrade" "Re-run installer with /P /UPDATE"
    Stop-AppProcesses $MainBinaryName
    Invoke-Installer "upgrade" $installerItem.FullName @("/P", "/UPDATE")

    if (-not (Test-Path -LiteralPath $mainExe)) {
        Fail-Stage "upgrade" "main executable missing after upgrade: $mainExe"
    }
    if (-not (Test-Path -LiteralPath $uninstaller)) {
        Fail-Stage "upgrade" "uninstaller missing after upgrade: $uninstaller"
    }
    Write-Host "[upgrade] post-upgrade PE size=$((Get-Item -LiteralPath $mainExe).Length) bytes"
    Close-Stage

    # ── uninstall (happy path) ──────────────────────────────────────────────
    Write-Stage "uninstall" "Passive uninstall (/P)"
    Stop-AppProcesses $MainBinaryName
    if (-not (Test-Path -LiteralPath $uninstaller)) {
        Fail-Stage "uninstall" "uninstaller missing: $uninstaller"
    }
    Invoke-Installer "uninstall" $uninstaller @("/P")
    Start-Sleep -Seconds 2

    if (Test-Path -LiteralPath $mainExe) {
        Fail-Stage "uninstall" "main executable still present after uninstall: $mainExe"
    }
    Write-Host "[uninstall] main executable removed"
    $didInstall = $false
    Close-Stage

    Write-Host "Packaged lifecycle smoke passed: install → launch → upgrade → uninstall"
}
catch {
    $smokeFailed = $true
    $smokeError = $_
    Write-Host "::error::[smoke] $($_.Exception.Message)"
}
finally {
    # Always attempt cleanup so a failed launch/upgrade does not leave the product
    # installed on the runner (and so retry jobs start clean).
    if ($didInstall -or (Test-Path -LiteralPath $mainExe) -or (Test-Path -LiteralPath $uninstaller)) {
        Write-Stage "uninstall" "finally: best-effort uninstall after smoke"
        try {
            Invoke-UninstallBestEffort "uninstall" $uninstaller $MainBinaryName $mainExe
        }
        catch {
            Write-Host "::warning::[uninstall] finally cleanup failed: $_"
        }
        Close-Stage
    }
    foreach ($name in $smokeEnvironmentNames) {
        [Environment]::SetEnvironmentVariable(
            $name,
            $previousSmokeEnvironment[$name],
            "Process"
        )
    }
    if ($null -ne $protectedCodexSnapshot) {
        try {
            Assert-ProtectedCodexSnapshot $protectedCodexSnapshot $protectedCodexHomes.ToArray()
        }
        catch {
            Write-Host "::error::[alignment] $($_.Exception.Message)"
            if (-not $smokeFailed) {
                $smokeFailed = $true
                $smokeError = $_
            }
        }
    }
    if ($smokeDirCreated) {
        $resolvedSmokeDataDir = [System.IO.Path]::GetFullPath($smokeDataDir)
        $resolvedSmokeParent = [System.IO.Path]::GetDirectoryName($resolvedSmokeDataDir).TrimEnd(
            [System.IO.Path]::DirectorySeparatorChar,
            [System.IO.Path]::AltDirectorySeparatorChar
        )
        $expectedSmokeLeaf = "codex-app-manager-smoke-$runId"
        if ($resolvedSmokeParent -ine $smokeTempRoot -or
            [System.IO.Path]::GetFileName($resolvedSmokeDataDir) -cne $expectedSmokeLeaf) {
            $cleanupMessage = "refusing to remove unexpected smoke directory: $resolvedSmokeDataDir"
            Write-Host "::error::[alignment] $cleanupMessage"
            if (-not $smokeFailed) {
                $smokeFailed = $true
                $smokeError = [System.InvalidOperationException]::new($cleanupMessage)
            }
        }
        else {
            Remove-Item -LiteralPath $resolvedSmokeDataDir -Recurse -Force -ErrorAction SilentlyContinue
            if (Test-Path -LiteralPath $resolvedSmokeDataDir) {
                $cleanupMessage = "isolated smoke directory was not removed: $resolvedSmokeDataDir"
                Write-Host "::error::[alignment] $cleanupMessage"
                if (-not $smokeFailed) {
                    $smokeFailed = $true
                    $smokeError = [System.IO.IOException]::new($cleanupMessage)
                }
            }
        }
    }
}

if ($smokeFailed) {
    throw $smokeError
}

# Do not `exit` — CI invokes this in-process with `&`.
return
