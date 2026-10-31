param(
    [Parameter(Mandatory)]
    [string] $Tag,

    [Parameter(Mandatory)]
    [string] $InstallerPath,

    [Parameter(Mandatory)]
    [string] $OutputDirectory,

    [switch] $Update,

    [string] $WingetCreatePath = './dist/winget/wingetcreate.exe'
)

$ErrorActionPreference = 'Stop'

if ($Tag -cnotmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\z') {
    throw 'WinGet releases must use a stable vX.Y.Z tag.'
}

$version = $Tag.Substring(1)
$hash = (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash
$manifestDirectory = Join-Path $OutputDirectory "manifests/q/qcksys/qlipq/$version"

if ($Update) {
    & $WingetCreatePath update qcksys.qlipq --version $version `
        --urls "https://github.com/qcksys/qlipq/releases/download/$Tag/qlipq-setup-x64.exe|x64" `
        --release-notes-url "https://github.com/qcksys/qlipq/releases/tag/$Tag" `
        --out $OutputDirectory | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "WingetCreate update failed: $LASTEXITCODE" }
    $installer = Get-Content -LiteralPath (Join-Path $manifestDirectory 'qcksys.qlipq.installer.yaml') -Raw
    if ($installer -notmatch $hash) { throw 'WingetCreate manifest does not match the published installer hash.' }
    # WingetCreate 1.12.13 writes schema 1.12.0, older than the 1.28.0 winget-pkgs asks for.
    foreach ($manifest in Get-ChildItem -LiteralPath $manifestDirectory -Filter '*.yaml') {
        $content = (Get-Content -LiteralPath $manifest.FullName -Raw) `
            -replace '(?m)(?<=^ManifestVersion: )\d+\.\d+\.\d+', '1.28.0' `
            -replace '(?<=winget-manifest\.\w+\.)\d+\.\d+\.\d+(?=\.schema\.json)', '1.28.0'
        Set-Content -LiteralPath $manifest.FullName -Value $content -Encoding utf8NoBOM -NoNewline
    }
    return $manifestDirectory
}

New-Item -ItemType Directory -Path $manifestDirectory -Force | Out-Null

foreach ($template in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.yaml') {
    $manifest = (Get-Content -LiteralPath $template.FullName -Raw).
        Replace('{{VERSION}}', $version).
        Replace('{{TAG}}', $Tag).
        Replace('{{SHA256}}', $hash)
    Set-Content -LiteralPath (Join-Path $manifestDirectory $template.Name) -Value $manifest -Encoding utf8NoBOM -NoNewline
}

return $manifestDirectory
