$scriptPath = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) `
    "scripts\sign-windows-authenticode.ps1"

Describe "Windows Authenticode signing modes" {
    It "fails when required signing has no certificate" {
        $fixture = Join-Path $TestDrive "candidate.exe"
        Set-Content -LiteralPath $fixture -Value "fixture" -NoNewline

        { & $scriptPath -Path $fixture -Mode required -CertificateBase64 "" } |
            Should Throw
    }

    It "keeps optional development signing non-blocking" {
        $fixture = Join-Path $TestDrive "development.exe"
        Set-Content -LiteralPath $fixture -Value "fixture" -NoNewline

        { & $scriptPath -Path $fixture -Mode optional -CertificateBase64 "" } |
            Should Not Throw
    }
}
