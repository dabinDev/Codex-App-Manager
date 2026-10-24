$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$scriptPath = Join-Path $repoRoot "scripts\build-windows-distributable.ps1"
$libraryPath = Join-Path $repoRoot "scripts\windows-distribution-lib.ps1"

Describe "Signed Windows distribution" {
    BeforeEach {
        . $libraryPath
    }

    It "leaves existing delivery files untouched when the certificate is missing" {
        $delivery = Join-Path $TestDrive "desktop"
        New-Item -ItemType Directory -Path $delivery | Out-Null
        $existing = Join-Path $delivery "酸橘子安装器.exe"
        Set-Content -LiteralPath $existing -Value "old-installer" -NoNewline

        { & $scriptPath -Brand acid-orange -Worktree $repoRoot `
            -ExpectedCommit (git -C $repoRoot rev-parse HEAD) `
            -DeliveryDirectory $delivery -CertificateBase64 "" } | Should Throw

        (Get-Content -LiteralPath $existing -Raw) | Should Be "old-installer"
        @(Get-ChildItem -LiteralPath $delivery).Count | Should Be 1
    }

    It "requires an explicit publisher identity before touching the worktree" {
        $missingWorktree = Join-Path $TestDrive "does-not-exist"
        $message = ""

        try {
            & $scriptPath -Brand acid-orange -Worktree $missingWorktree `
                -ExpectedCommit "deadbeef" -DeliveryDirectory $TestDrive `
                -CertificateBase64 "not-a-real-certificate" -ExpectedPublisher ""
        }
        catch {
            $message = $_.Exception.Message
        }

        $message | Should Match "publisher.*required"
    }

    It "rejects the primary worktree before brand synchronization" {
        $message = ""
        try {
            & $scriptPath -Brand acid-orange -Worktree $repoRoot `
                -ExpectedCommit (git -C $repoRoot rev-parse HEAD) `
                -DeliveryDirectory $TestDrive -CertificateBase64 "not-a-real-certificate" `
                -ExpectedPublisher "CN=Expected Publisher"
        }
        catch {
            $message = $_.Exception.Message
        }

        $message | Should Match "isolated.*worktree"
    }

    It "rejects forbidden Tauri identity changes in a brand delta" {
        $baseBrand = '{"productName":"Base","serviceName":"Base","displayNames":{"en":"Base"},"sites":{"service":"https://base.invalid","updater":["https://updates.invalid/latest.json"]},"icons":{"frontend":{"light":"old-light.png","dark":"old-dark.png"},"bundle":["icons/icon.ico"],"windowsInstaller":"icons/icon.ico"}}'
        $actualBrand = '{"productName":"\u9178\u6a58\u5b50\u5b89\u88c5\u5668","serviceName":"token.cylonai.cn \u00b7 OpenAI","displayNames":{"en":"\u9178\u6a58\u5b50\u5b89\u88c5\u5668"},"sites":{"service":"https://token.cylonai.cn","updater":["https://updates.invalid/latest.json"]},"icons":{"frontend":{"light":"src/app/assets/orange-logo.png","dark":"src/app/assets/orange-logo.png"},"bundle":["icons/icon.ico"],"windowsInstaller":"icons/icon.ico"}}'
        $baseTauri = '{"productName":"Base","identifier":"io.example.fixed","mainBinaryName":"fixed","app":{"windows":[{"title":"Base"}]},"bundle":{"icon":["icons/icon.ico"],"windows":{"nsis":{"installerIcon":"icons/icon.ico"}}},"plugins":{"updater":{"endpoints":["https://updates.invalid/latest.json"]}}}'
        $tamperedTauri = '{"productName":"\u9178\u6a58\u5b50\u5b89\u88c5\u5668","identifier":"io.attacker.changed","mainBinaryName":"fixed","app":{"windows":[{"title":"\u9178\u6a58\u5b50\u5b89\u88c5\u5668"}]},"bundle":{"icon":["icons/icon.ico"],"windows":{"nsis":{"installerIcon":"icons/icon.ico"}}},"plugins":{"updater":{"endpoints":["https://updates.invalid/latest.json"]}}}'

        $message = ""
        try {
            Assert-BrandedConfigDelta -Brand acid-orange `
                -BaseBrandJson $baseBrand -ActualBrandJson $actualBrand `
                -BaseTauriJson $baseTauri -ActualTauriJson $tamperedTauri
        }
        catch {
            $message = $_.Exception.Message
        }
        $message | Should Match "tauri.conf.json.*approved brand delta"
    }

    It "archives and delivers identical bytes after backing up an existing file" {
        $caseRoot = Join-Path $TestDrive "success"
        $source = Join-Path $caseRoot "signed-setup.exe"
        $archive = Join-Path $caseRoot "archive\signed-setup.exe"
        $delivery = Join-Path $caseRoot "desktop\installer.exe"
        New-Item -ItemType Directory -Path (Split-Path -Parent $delivery) -Force | Out-Null
        Set-Content -LiteralPath $source -Value "signed-installer" -NoNewline
        Set-Content -LiteralPath $delivery -Value "old-installer" -NoNewline

        $expectedHash = Get-Sha256 -Path $source
        Copy-ToVerifiedArchive -SourcePath $source -ArchivePath $archive `
            -ExpectedHash $expectedHash
        $result = Publish-VerifiedDelivery -ArchivePath $archive -DeliveryPath $delivery `
            -ExpectedHash $expectedHash

        (Get-Sha256 -Path $archive) | Should Be $expectedHash
        (Get-Sha256 -Path $delivery) | Should Be $expectedHash
        $result.BackupPath | Should Exist
        (Get-Content -LiteralPath $result.BackupPath -Raw) | Should Be "old-installer"
    }

    It "preserves the old delivery when archived bytes no longer match" {
        $caseRoot = Join-Path $TestDrive "tamper"
        $source = Join-Path $caseRoot "source.exe"
        $archive = Join-Path $caseRoot "archive\source.exe"
        $delivery = Join-Path $caseRoot "desktop\installer.exe"
        New-Item -ItemType Directory -Path (Split-Path -Parent $delivery) -Force | Out-Null
        Set-Content -LiteralPath $source -Value "signed-installer" -NoNewline
        Set-Content -LiteralPath $delivery -Value "old-installer" -NoNewline
        $expectedHash = Get-Sha256 -Path $source
        Copy-ToVerifiedArchive -SourcePath $source -ArchivePath $archive `
            -ExpectedHash $expectedHash
        Set-Content -LiteralPath $archive -Value "tampered" -NoNewline

        { Publish-VerifiedDelivery -ArchivePath $archive -DeliveryPath $delivery `
            -ExpectedHash $expectedHash } | Should Throw

        (Get-Content -LiteralPath $delivery -Raw) | Should Be "old-installer"
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $delivery) -Filter "*.bak-*").Count |
            Should Be 0
    }

    It "atomically restores the old delivery after post-replace corruption" {
        $caseRoot = Join-Path $TestDrive "post-replace-corruption"
        $archive = Join-Path $caseRoot "archive\installer.exe"
        $delivery = Join-Path $caseRoot "desktop\installer.exe"
        New-Item -ItemType Directory -Path (Split-Path -Parent $archive) -Force | Out-Null
        New-Item -ItemType Directory -Path (Split-Path -Parent $delivery) -Force | Out-Null
        Set-Content -LiteralPath $archive -Value "signed-installer" -NoNewline
        Set-Content -LiteralPath $delivery -Value "old-installer" -NoNewline
        $expectedHash = Get-Sha256 -Path $archive
        $message = ""

        try {
            Publish-VerifiedDelivery -ArchivePath $archive -DeliveryPath $delivery `
                -ExpectedHash $expectedHash -AfterReplaceForTest {
                    param($path)
                    Set-Content -LiteralPath $path -Value "corrupt-new-bytes" -NoNewline
                }
        }
        catch {
            $message = $_.Exception.Message
        }

        $message | Should Match "delivered SHA-256 mismatch"
        (Get-Content -LiteralPath $delivery -Raw) | Should Be "old-installer"
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $delivery) -Filter "*.bak-*").Count |
            Should Be 1
    }

    It "removes an unverified first delivery after post-replace corruption" {
        $caseRoot = Join-Path $TestDrive "first-delivery-corruption"
        $archive = Join-Path $caseRoot "archive\installer.exe"
        $delivery = Join-Path $caseRoot "desktop\installer.exe"
        New-Item -ItemType Directory -Path (Split-Path -Parent $archive) -Force | Out-Null
        New-Item -ItemType Directory -Path (Split-Path -Parent $delivery) -Force | Out-Null
        Set-Content -LiteralPath $archive -Value "signed-installer" -NoNewline
        $expectedHash = Get-Sha256 -Path $archive

        { Publish-VerifiedDelivery -ArchivePath $archive -DeliveryPath $delivery `
            -ExpectedHash $expectedHash -AfterReplaceForTest {
                param($path)
                Set-Content -LiteralPath $path -Value "corrupt-new-bytes" -NoNewline
            } } | Should Throw

        $delivery | Should Not Exist
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $delivery) -Filter "*.bak-*").Count |
            Should Be 0
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $delivery) -Filter "*.swap-*").Count |
            Should Be 0
    }

    It "retains recovery files when automatic restore fails" {
        $caseRoot = Join-Path $TestDrive "restore-failure"
        $archive = Join-Path $caseRoot "archive\installer.exe"
        $delivery = Join-Path $caseRoot "desktop\installer.exe"
        New-Item -ItemType Directory -Path (Split-Path -Parent $archive) -Force | Out-Null
        New-Item -ItemType Directory -Path (Split-Path -Parent $delivery) -Force | Out-Null
        Set-Content -LiteralPath $archive -Value "signed-installer" -NoNewline
        Set-Content -LiteralPath $delivery -Value "old-installer" -NoNewline
        $expectedHash = Get-Sha256 -Path $archive
        $message = ""

        try {
            Publish-VerifiedDelivery -ArchivePath $archive -DeliveryPath $delivery `
                -ExpectedHash $expectedHash `
                -AfterReplaceForTest {
                    param($path)
                    Set-Content -LiteralPath $path -Value "corrupt-new-bytes" -NoNewline
                } `
                -RestoreActionForTest { throw "injected restore failure" }
        }
        catch {
            $message = $_.Exception.Message
        }

        $message | Should Match "automatic restore failed"
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $delivery) -Filter "*.bak-*").Count |
            Should Be 1
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $delivery) -Filter "*.swap-*").Count |
            Should Be 1
    }
}
