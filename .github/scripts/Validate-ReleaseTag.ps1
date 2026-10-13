param(
    [Parameter(Mandatory = $true)]
    [string]$TagName,
    [Parameter(Mandatory = $true)]
    [string]$EventPath
)

$ErrorActionPreference = 'Stop'
$event = Get-Content -LiteralPath $EventPath -Raw | ConvertFrom-Json
if ($event.created -ne $true -or $event.deleted -ne $false) {
    throw 'Publication requires a newly created, non-deleted tag.'
}
if ($event.ref -cne "refs/tags/$TagName" -or !$TagName.StartsWith('v')) {
    throw 'Publication requires a v-prefixed tag push.'
}

$manifestPath = Join-Path $PSScriptRoot '../../Cargo.toml'
$lockPath = Join-Path $PSScriptRoot '../../Cargo.lock'
if (!(Test-Path -LiteralPath $lockPath)) {
    throw 'Cargo.lock must be committed before creating a release tag.'
}
$metadataJson = cargo metadata --manifest-path $manifestPath --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata or lockfile validation failed.' }
$metadata = $metadataJson | ConvertFrom-Json
$packages = @($metadata.packages | Where-Object { $_.name -eq 'usn-journal-rs' -and $null -eq $_.source })
if ($packages.Count -ne 1) { throw 'Expected one local usn-journal-rs package.' }
$expectedTag = "v$($packages[0].version)"
if ($TagName -cne $expectedTag) {
    throw "Tag '$TagName' does not match package version '$expectedTag'."
}
Write-Output "Validated $expectedTag and Cargo.lock."
