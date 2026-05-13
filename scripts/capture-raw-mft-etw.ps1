[CmdletBinding()]
param(
    [ValidateSet('dynamic', 'contiguous', 'dynamic-physical-order', 'dynamic-cost-banded', 'dynamic-observed-adaptive')]
    [string[]]$Scheduling = @('dynamic'),

    [string]$OutputRoot = (Join-Path $PSScriptRoot '..\tmp\raw_mft_parallel_ingest_validation\etw'),

    [string]$Drive = $(if ($env:USN_RAW_MFT_BENCH_DRIVE) { $env:USN_RAW_MFT_BENCH_DRIVE } elseif ($env:USN_TEST_DRIVE) { $env:USN_TEST_DRIVE } else { 'C' }),

    [int]$Workers = 11,
    [UInt64]$ChunkRecords = 2048,
    [int]$MainBufferBytes = 262144,
    [int]$AttrBufferBytes = 16384,
    [UInt64]$StartRecord = 24,
    [string]$EndRecord,

    [switch]$SummaryLight,
    [switch]$SortAttrListByOffset,
    [switch]$PrintAttrListProfile,
    [switch]$DeferredAttrList,
    [int]$DeferredAttrListWindowRecords,
    [switch]$SkipBuild,
    [switch]$SummarizeWithXperf
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Test-IsElevated {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-RequiredCommand {
    param([Parameter(Mandatory)] [string]$Name)

    $command = Get-Command -Name $Name -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        throw "Required command '$Name' was not found in PATH."
    }
    return $command.Source
}

function Set-ScopedEnv {
    param(
        [Parameter(Mandatory)] [hashtable]$Values,
        [Parameter(Mandatory)] [hashtable]$Previous
    )

    foreach ($key in $Values.Keys) {
        $existing = [Environment]::GetEnvironmentVariable($key, 'Process')
        $Previous[$key] = $existing
        $value = $Values[$key]
        if ($null -eq $value -or $value -eq '') {
            Remove-Item -Path ("Env:{0}" -f $key) -ErrorAction SilentlyContinue
        } else {
            Set-Item -Path ("Env:{0}" -f $key) -Value ([string]$value)
        }
    }
}

function Restore-ScopedEnv {
    param([Parameter(Mandatory)] [hashtable]$Previous)

    foreach ($key in $Previous.Keys) {
        $value = $Previous[$key]
        if ($null -eq $value) {
            Remove-Item -Path ("Env:{0}" -f $key) -ErrorAction SilentlyContinue
        } else {
            Set-Item -Path ("Env:{0}" -f $key) -Value $value
        }
    }
}

if (-not (Test-IsElevated)) {
    throw 'ETW capture and raw volume access both require an elevated PowerShell session.'
}

$null = Get-RequiredCommand -Name 'cargo'
$wprPath = Get-RequiredCommand -Name 'wpr'

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$exampleExe = Join-Path $repoRoot 'target\release\examples\raw_mft_parallel_ingest_profile.exe'
$summaryScript = Join-Path $PSScriptRoot 'summarize-raw-mft-etw.ps1'

$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null

Push-Location $repoRoot
try {
    if (-not $SkipBuild) {
        Write-Host 'Building release profile target before ETW capture...'
        & cargo build --release --example raw_mft_parallel_ingest_profile
        if ($LASTEXITCODE -ne 0) {
            throw 'Release build failed.'
        }
    }

    if (-not (Test-Path $exampleExe)) {
        throw "Profile executable was not found at '$exampleExe'."
    }

    foreach ($mode in $Scheduling) {
        $timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
        $captureName = "raw_mft_parallel_ingest_{0}_{1}" -f $mode.Replace('-', '_'), $timestamp
        $captureDir = Join-Path $OutputRoot $captureName
        $etlPath = Join-Path $captureDir ($captureName + '.etl')
        $stdoutPath = Join-Path $captureDir 'profile.stdout.txt'
        $stderrPath = Join-Path $captureDir 'profile.stderr.txt'
        $metadataPath = Join-Path $captureDir 'capture.json'
        $envBefore = @{}
        $captureStarted = $false

        New-Item -ItemType Directory -Force -Path $captureDir | Out-Null

        $envValues = @{
            USN_RAW_MFT_BENCH_DRIVE = $Drive
            USN_RAW_MFT_BENCH_WORKERS = $Workers
            USN_RAW_MFT_BENCH_CHUNK_RECORDS = $ChunkRecords
            USN_RAW_MFT_BENCH_BUFFER_BYTES = $MainBufferBytes
            USN_RAW_MFT_BENCH_ATTR_BUFFER_BYTES = $AttrBufferBytes
            USN_RAW_MFT_BENCH_START_RECORD = $StartRecord
            USN_RAW_MFT_BENCH_END_RECORD = $EndRecord
            USN_RAW_MFT_BENCH_SCHEDULING = $mode
            USN_RAW_MFT_BENCH_SUMMARY_ATTR_LIST_LIGHT = $(if ($SummaryLight) { '1' } else { $null })
            USN_RAW_MFT_BENCH_ATTR_LIST_SORT_BY_OFFSET = $(if ($SortAttrListByOffset) { '1' } else { $null })
            USN_RAW_MFT_BENCH_PRINT_ATTR_LIST_PROFILE = $(if ($PrintAttrListProfile) { '1' } else { $null })
            USN_RAW_MFT_BENCH_DEFERRED_ATTR_LIST = $(if ($DeferredAttrList) { '1' } else { '0' })
            USN_RAW_MFT_BENCH_DEFERRED_ATTR_LIST_WINDOW_RECORDS = $(if ($PSBoundParameters.ContainsKey('DeferredAttrListWindowRecords')) { $DeferredAttrListWindowRecords } else { $null })
        }

        try {
            Set-ScopedEnv -Values $envValues -Previous $envBefore

            Write-Host "Starting WPR capture for scheduling mode '$mode'..."
            & $wprPath -start GeneralProfile -start FileIO -start DiskIO -filemode | Out-Null
            $captureStarted = $true

            $startTime = Get-Date
            Write-Host "Running $exampleExe"
            $process = Start-Process -FilePath $exampleExe -WorkingDirectory $repoRoot -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -PassThru -Wait -NoNewWindow
            $endTime = Get-Date

            if ($process.ExitCode -ne 0) {
                throw "Profile executable exited with code $($process.ExitCode)."
            }

            Write-Host "Stopping WPR capture to $etlPath"
            & $wprPath -stop $etlPath | Out-Null
            $captureStarted = $false

            $metadata = [ordered]@{
                capture_name = $captureName
                scheduling = $mode
                repo_root = $repoRoot
                example_exe = $exampleExe
                etl_path = $etlPath
                stdout_path = $stdoutPath
                stderr_path = $stderrPath
                started_at = $startTime.ToString('o')
                finished_at = $endTime.ToString('o')
                duration_seconds = [Math]::Round(($endTime - $startTime).TotalSeconds, 3)
                environment = $envValues
            }
            $metadata | ConvertTo-Json -Depth 4 | Set-Content -Path $metadataPath -Encoding UTF8

            if ($SummarizeWithXperf) {
                & $summaryScript -EtlPath $etlPath
            }

            Write-Host "Capture completed: $etlPath"
        } finally {
            if ($captureStarted) {
                Write-Warning 'WPR capture was still active during cleanup; attempting to stop it.'
                try {
                    & $wprPath -stop $etlPath | Out-Null
                } catch {
                    Write-Warning "Failed to stop WPR cleanly: $($_.Exception.Message)"
                }
            }
            Restore-ScopedEnv -Previous $envBefore
        }
    }
} finally {
    Pop-Location
}



