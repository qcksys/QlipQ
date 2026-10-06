param([switch] $ValidateWithWinGet)

$ErrorActionPreference = 'Stop'
$testDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "qlipq-winget-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $testDirectory | Out-Null

function Assert-Contains([string] $Content, [string] $Expected) {
    if (-not $Content.Contains($Expected)) {
        throw "Expected manifest to contain: $Expected"
    }
}

try {
    $installerPath = Join-Path $testDirectory 'qlipq-setup-x64.exe'
    Set-Content -LiteralPath $installerPath -Value 'installer fixture' -NoNewline
    $arguments = @{
        InstallerPath = $installerPath
        OutputDirectory = $testDirectory
    }
    $generate = Join-Path $PSScriptRoot 'New-WinGetManifest.ps1'
    $directory = & $generate -Tag 'v1.2.3' @arguments
    $files = @(Get-ChildItem -LiteralPath $directory -Filter '*.yaml')
    if ($files.Count -ne 3) { throw 'Expected three manifest files.' }

    foreach ($file in $files) {
        $content = Get-Content -LiteralPath $file.FullName -Raw
        Assert-Contains $content 'PackageIdentifier: qcksys.qlipq'
        Assert-Contains $content 'PackageVersion: "1.2.3"'
        if ($content -match '\{\{') { throw "Unresolved placeholder in $($file.Name)." }
    }

    $installer = Get-Content -LiteralPath (Join-Path $directory 'qcksys.qlipq.installer.yaml') -Raw
    Assert-Contains $installer 'https://github.com/qcksys/qlipq/releases/download/v1.2.3/qlipq-setup-x64.exe'
    Assert-Contains $installer (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash
    # Match the installer identity so WinGet can detect installs and upgrades.
    $inno = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../../apps/desktop/installer/qlipq.iss') -Raw
    if ($inno -notmatch '(?m)^AppId=\{(\{[^\r\n]+\})\r?$') { throw 'Could not read Inno AppId.' }
    Assert-Contains $installer "ProductCode: `"$($Matches[1])_is1`""

    $locale = Get-Content -LiteralPath (Join-Path $directory 'qcksys.qlipq.locale.en-US.yaml') -Raw
    Assert-Contains $locale 'ReleaseNotesUrl: https://github.com/qcksys/qlipq/releases/tag/v1.2.3'

    if ($ValidateWithWinGet) {
        winget validate --manifest $directory --disable-interactivity
        if ($LASTEXITCODE -ne 0) { throw "WinGet validation failed: $LASTEXITCODE" }
    }

    Set-Content -LiteralPath $installerPath -Value 'next installer fixture' -NoNewline
    $nextDirectory = & $generate -Tag 'v1.2.4' @arguments
    $nextInstaller = Get-Content -LiteralPath (Join-Path $nextDirectory 'qcksys.qlipq.installer.yaml') -Raw
    Assert-Contains $nextInstaller 'PackageVersion: "1.2.4"'
    Assert-Contains $nextInstaller (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash
    if ($nextInstaller -eq $installer) { throw 'The next release must use its own version and hash.' }

    foreach ($tag in @('1.2.3', 'v1.2.3-beta.1', 'v1.2.3+build', 'v01.2.3', "v1.2.3`n", "v1.2.3`nInjected: true")) {
        $rejected = $false
        try { & $generate -Tag $tag @arguments | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw "Invalid tag was accepted: $tag" }
    }

    $rejected = $false
    try {
        & $generate -Tag 'v1.2.3' -InstallerPath (Join-Path $testDirectory 'missing.exe') -OutputDirectory $testDirectory | Out-Null
    } catch { $rejected = $true }
    if (-not $rejected) { throw 'A missing installer must fail generation.' }

    function Invoke-TestWingetCreate {
        $wingetTest.Arguments = @($args)
        $global:LASTEXITCODE = $wingetTest.ExitCode
        if ($wingetTest.ExitCode -ne 0) { return }
        $output = & $generate -Tag 'v1.2.5' @arguments
        $localePath = Join-Path $output 'qcksys.qlipq.locale.en-US.yaml'
        $content = (Get-Content -LiteralPath $localePath -Raw).Replace('Publisher: qcksys', 'Publisher: Accepted publisher')
        Set-Content -LiteralPath $localePath -Value $content
        if ($wingetTest.WrongHash) {
            $installerManifest = Join-Path $output 'qcksys.qlipq.installer.yaml'
            Set-Content -LiteralPath $installerManifest -Value 'InstallerSha256: incorrect'
        }
        Write-Output 'WingetCreate diagnostic output'
    }

    $wingetTest = @{ ExitCode = 0; WrongHash = $false; Arguments = @() }
    $updated = & $generate -Tag 'v1.2.5' @arguments -Update -WingetCreatePath Invoke-TestWingetCreate
    $expectedArguments = @(
        'update', 'qcksys.qlipq', '--version', '1.2.5',
        '--urls', 'https://github.com/qcksys/qlipq/releases/download/v1.2.5/qlipq-setup-x64.exe|x64',
        '--release-notes-url', 'https://github.com/qcksys/qlipq/releases/tag/v1.2.5',
        '--out', $testDirectory
    )
    if (($wingetTest.Arguments -join "`n") -cne ($expectedArguments -join "`n")) { throw 'Incorrect WingetCreate update arguments.' }
    if ($updated -isnot [string] -or -not (Test-Path -LiteralPath $updated)) { throw 'Update must return only the manifest directory.' }
    Assert-Contains (Get-Content -LiteralPath (Join-Path $updated 'qcksys.qlipq.locale.en-US.yaml') -Raw) 'Publisher: Accepted publisher'
    if ($ValidateWithWinGet) {
        winget validate --manifest $updated --disable-interactivity
        if ($LASTEXITCODE -ne 0) { throw "Updated manifest validation failed: $LASTEXITCODE" }
    }

    foreach ($failure in @('CLI failure', 'hash mismatch')) {
        $wingetTest.ExitCode = if ($failure -eq 'CLI failure') { 1 } else { 0 }
        $wingetTest.WrongHash = $failure -eq 'hash mismatch'
        $expectedError = if ($failure -eq 'CLI failure') { 'WingetCreate update failed' } else { 'published installer hash' }
        $rejected = $false
        try { & $generate -Tag 'v1.2.5' @arguments -Update -WingetCreatePath Invoke-TestWingetCreate | Out-Null }
        catch { $rejected = $_.ToString().Contains($expectedError) }
        if (-not $rejected) { throw "$failure must stop generation without falling back to templates." }
    }

    Write-Host 'WinGet manifest tests passed.'
} finally {
    $cleanupPath = (Resolve-Path -LiteralPath $testDirectory).Path
    if ((Split-Path -Parent $cleanupPath) -ne [System.IO.Path]::GetTempPath().TrimEnd('\', '/')) {
        throw 'Refusing to remove a test directory outside the temporary directory.'
    }
    Remove-Item -LiteralPath $testDirectory -Recurse -Force
}
