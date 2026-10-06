$ErrorActionPreference = 'Stop'
$check = Join-Path $PSScriptRoot 'Get-WinGetPublicationState.ps1'
$testState = @{ Requests = 0 }

function Invoke-WebRequest($Uri, $Headers, [switch] $SkipHttpErrorCheck) {
    $testState.Requests++
    if ($Uri -eq 'https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests/q/qcksys/qlipq') {
        return @{ StatusCode = $case.Status; Content = '[{"name":"1.2.2"},{"name":"1.2.3"}]' }
    }
    if ($Uri -notlike 'https://api.github.com/search/issues?q=*&per_page=100') { throw "Unexpected URL: $Uri" }
    $items = @($case.Titles | ForEach-Object { @{ title = $_; html_url = 'https://github.com/microsoft/winget-pkgs/pull/123' } })
    return @{
        StatusCode = $(if ($case.SearchStatus) { $case.SearchStatus } else { 200 })
        Content = @{
            items = $items
            incomplete_results = [bool] $case.Incomplete
            total_count = $(if ($case.Count) { $case.Count } else { $items.Count })
        } | ConvertTo-Json -Depth 4
    }
}

$cases = @(
    @{ Name = 'first submission'; Status = 404; Update = $false; Submit = $true },
    @{ Name = 'pending first submission'; Status = 404; Titles = @('qcksys.qlipq version 1.2.4'); Update = $false; Submit = $false },
    @{ Name = 'older first submission'; Status = 404; Titles = @('qcksys.qlipq version 1.0.0'); Update = $false; Submit = $false },
    @{ Name = 'accepted package update'; Status = 200; Update = $true; Submit = $true },
    @{ Name = 'already accepted version'; Status = 200; Tag = 'v1.2.3'; Update = $true; Submit = $false },
    @{ Name = 'pending update'; Status = 200; Titles = @('New version: qcksys.qlipq version 1.2.4'); Update = $true; Submit = $false },
    @{ Name = 'pending update with tag'; Status = 200; Titles = @('qcksys.qlipq v1.2.4'); Update = $true; Submit = $false },
    @{ Name = 'different version'; Status = 200; Titles = @('qcksys.qlipq version 1.2.40'); Update = $true; Submit = $true },
    @{ Name = 'different package'; Status = 200; Titles = @('qcksys.qlipq.Other version 1.2.4'); Update = $true; Submit = $true },
    @{ Name = 'authentication failure'; Status = 401; Error = 'HTTP 401' },
    @{ Name = 'rate limit'; Status = 403; Error = 'HTTP 403' },
    @{ Name = 'API unavailable'; Status = 500; Error = 'HTTP 500' },
    @{ Name = 'search failure'; Status = 404; SearchStatus = 503; Error = 'HTTP 503' },
    @{ Name = 'incomplete search'; Status = 200; Incomplete = $true; Error = 'incomplete' },
    @{ Name = 'truncated search'; Status = 200; Count = 101; Error = 'incomplete' },
    @{ Name = 'prerelease'; Tag = 'v1.2.4-beta.1'; Error = 'Cannot validate argument' },
    @{ Name = 'tag with newline'; Tag = "v1.2.4`n"; Error = 'Cannot validate argument' }
)

foreach ($case in $cases) {
    $testState.Requests = 0
    $tag = if ($case.Tag) { $case.Tag } else { 'v1.2.4' }
    $failure = $null
    try { $state = & $check -Tag $tag } catch { $failure = $_ }
    if ($case.Error) {
        if (-not $failure -or "$failure" -notmatch $case.Error) { throw "$($case.Name): expected $($case.Error), got $failure" }
        continue
    }
    if ($failure) { throw $failure }
    if ($state.Update -ne $case.Update -or $state.Submit -ne $case.Submit) { throw "$($case.Name): incorrect publication decision." }
    if (-not $state.Submit -and -not $state.Reason) { throw "$($case.Name): missing skip reason." }
    if ($case.Tag -eq 'v1.2.3' -and $testState.Requests -ne 1) { throw 'Accepted versions must not depend on PR search.' }
}

Write-Host 'WinGet publication state tests passed.'
