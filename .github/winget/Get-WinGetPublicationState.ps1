param(
    [Parameter(Mandatory)]
    [ValidatePattern('^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\z')]
    [string] $Tag
)

$ErrorActionPreference = 'Stop'
$version = $Tag.Substring(1)
$headers = @{ Accept = 'application/vnd.github+json' }
if ($env:GH_TOKEN) { $headers.Authorization = "Bearer $env:GH_TOKEN" }

$response = Invoke-WebRequest -Uri 'https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests/q/qcksys/qlipq' -Headers $headers -SkipHttpErrorCheck
if ($response.StatusCode -notin @(200, 404)) {
    throw "Could not read the WinGet package: HTTP $($response.StatusCode)."
}

$update = $response.StatusCode -eq 200
$reason = ''
if ($update -and $version -in ($response.Content | ConvertFrom-Json).name) {
    $reason = "Version $version is already in microsoft/winget-pkgs."
} else {
    $query = [uri]::EscapeDataString('repo:microsoft/winget-pkgs is:pr is:open "qcksys.qlipq" in:title')
    $response = Invoke-WebRequest -Uri "https://api.github.com/search/issues?q=$query&per_page=100" -Headers $headers -SkipHttpErrorCheck
    if ($response.StatusCode -ne 200) { throw "Could not check WinGet PRs: HTTP $($response.StatusCode)." }
    $search = $response.Content | ConvertFrom-Json
    if ($search.incomplete_results -or $search.total_count -gt 100) { throw 'WinGet PR search was incomplete.' }
    $versionPattern = '(?<![\w.])v?' + [regex]::Escape($version) + '(?![\w.])'
    $pending = $search.items | Where-Object {
        $_.title -match '(?<![\w.])qcksys\.qlipq(?![\w.])' -and
        (-not $update -or $_.title -match $versionPattern)
    } | Select-Object -First 1
    if ($pending) { $reason = "A package submission is already open: $($pending.html_url)" }
}

[pscustomobject]@{ Update = $update; Submit = -not $reason; Reason = $reason }
