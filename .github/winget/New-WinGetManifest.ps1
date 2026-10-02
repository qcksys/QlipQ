param(
    [Parameter(Mandatory)]
    [string] $Tag,

    [Parameter(Mandatory)]
    [string] $InstallerPath,

    [Parameter(Mandatory)]
    [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'

if ($Tag -cnotmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\z') {
    throw 'WinGet releases must use a stable vX.Y.Z tag.'
}

$version = $Tag.Substring(1)
$hash = (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash
$manifestDirectory = Join-Path $OutputDirectory "manifests/q/qcksys/qlipq/$version"
New-Item -ItemType Directory -Path $manifestDirectory -Force | Out-Null

foreach ($template in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.yaml') {
    $manifest = (Get-Content -LiteralPath $template.FullName -Raw).
        Replace('{{VERSION}}', $version).
        Replace('{{TAG}}', $Tag).
        Replace('{{SHA256}}', $hash)
    Set-Content -LiteralPath (Join-Path $manifestDirectory $template.Name) -Value $manifest -Encoding utf8NoBOM -NoNewline
}

return $manifestDirectory
