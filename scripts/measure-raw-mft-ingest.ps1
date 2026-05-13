[CmdletBinding()]
param(
	[string]$BaselineRef = 'HEAD',

	[string]$OutputRoot = (Join-Path $PSScriptRoot '..\tmp\raw_mft_parallel_ingest_validation\measurement'),

	[string]$Drive = $(if ($env:USN_RAW_MFT_BENCH_DRIVE) { $env:USN_RAW_MFT_BENCH_DRIVE } elseif ($env:USN_TEST_DRIVE) { $env:USN_TEST_DRIVE } else { 'C' }),

	[ValidateSet('dynamic', 'contiguous', 'dynamic-physical-order', 'dynamic-cost-banded', 'dynamic-observed-adaptive')]
	[string]$Scheduling = 'dynamic',

	[int]$Workers = 11,
	[UInt64]$ChunkRecords = 2048,
	[int]$MainBufferBytes = 262144,
	[int]$AttrBufferBytes = 16384,
	[UInt64]$StartRecord = 24,
	[string]$EndRecord,

	[switch]$SummaryLight,
	[switch]$SortAttrListByOffset,
	[switch]$PrintAttrListProfile,
	[switch]$PrintSchedulingProfile,
	[switch]$DeferredAttrList,
	[int]$DeferredAttrListWindowRecords,
	[switch]$CostHintAttrSample,

	[ValidateRange(2, 100)]
	[int]$SampleSize = 10,

	[ValidateRange(1, 600)]
	[int]$WarmUpSeconds = 5,

	[ValidateRange(1, 3600)]
	[int]$MeasurementSeconds = 30,

	[string]$CriterionBaselineName = 'candidate',

	[ValidateRange(1, 100)]
	[int]$ExactRuns = 5,

	[ValidateRange(0, 4096)]
	[double]$QuietDiskThresholdMiBPerSec = 32.0,

	[ValidateRange(1, 60)]
	[int]$QuietDiskConsecutiveSamples = 3,

	[ValidateRange(1, 30)]
	[int]$QuietDiskSampleSeconds = 2,

	[ValidateRange(1, 3600)]
	[int]$QuietDiskTimeoutSeconds = 120,

	[switch]$SkipCriterion,
	[switch]$SkipExactMatch,
	[switch]$SkipBuild,
	[switch]$KeepBaselineWorktree,
	[switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$hasDeferredAttrListWindowRecords = $PSBoundParameters.ContainsKey('DeferredAttrListWindowRecords')

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
		$Previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
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

function Format-Seconds {
	param([Parameter(Mandatory)] [double]$Seconds)

	return ('{0:N3}s' -f $Seconds)
}

function Get-SortedValues {
	param([Parameter(Mandatory)] [double[]]$Values)

	$sorted = [double[]]@($Values)
	[Array]::Sort($sorted)
	return ,$sorted
}

function Get-Median {
	param([Parameter(Mandatory)] [double[]]$Values)

	$sorted = @(Get-SortedValues -Values $Values)
	$count = $sorted.Length
	$middle = [int]($count / 2)
	if (($count % 2) -eq 1) {
		return $sorted[$middle]
	}

	return ($sorted[$middle - 1] + $sorted[$middle]) / 2.0
}

function Get-NumberSummary {
	param([Parameter(Mandatory)] [double[]]$Values)

	$measure = $Values | Measure-Object -Average -Minimum -Maximum
	[pscustomobject]@{
		count = $Values.Length
		mean_seconds = [double]$measure.Average
		median_seconds = Get-Median -Values $Values
		min_seconds = [double]$measure.Minimum
		max_seconds = [double]$measure.Maximum
	}
}

function Get-ExactElapsedSeconds {
	param([Parameter(Mandatory)] [string]$Text)

	$match = [regex]::Match($Text, '(?m)^\s*elapsed:\s+([0-9]+(?:\.[0-9]+)?)s\s*$')
	if (-not $match.Success) {
		throw 'Could not find an `elapsed:` line in exact-match output.'
	}

	return [double]$match.Groups[1].Value
}

function Get-CriterionSummary {
	param([Parameter(Mandatory)] [string]$Text)

	$timeMatch = [regex]::Matches($Text, 'time:\s+\[([^\]]+)\]') | Select-Object -Last 1
	$changeMatch = [regex]::Matches($Text, 'change:\s+\[([^\]]+)\]\s+\(p\s*=\s*([^\)]+)\)') | Select-Object -Last 1
	$verdictMatch = [regex]::Matches($Text, '(?m)^(Performance has improved\.|Performance has regressed\.|Change within noise threshold\.|No change in performance detected\.)$') | Select-Object -Last 1

	[pscustomobject]@{
		time = $(if ($timeMatch) { $timeMatch.Groups[1].Value } else { $null })
		change = $(if ($changeMatch) { $changeMatch.Groups[1].Value } else { $null })
		p_value = $(if ($changeMatch) { $changeMatch.Groups[2].Value } else { $null })
		verdict = $(if ($verdictMatch) { $verdictMatch.Value } else { $null })
	}
}

function Wait-ForQuieterDisk {
	param([Parameter(Mandatory)] [string]$Label)

	if ($DryRun -or $QuietDiskThresholdMiBPerSec -le 0) {
		return
	}

	$counterPath = '\PhysicalDisk(_Total)\Disk Bytes/sec'
	$deadline = (Get-Date).AddSeconds($QuietDiskTimeoutSeconds)
	$quietSamples = 0

	while ((Get-Date) -lt $deadline) {
		try {
			$sample = Get-Counter -Counter $counterPath -SampleInterval $QuietDiskSampleSeconds -MaxSamples 1
			$bytesPerSecond = [double]$sample.CounterSamples[0].CookedValue
		} catch {
			Write-Warning "Failed to read disk counter '$counterPath': $($_.Exception.Message). Continuing without the quiet-disk gate."
			return
		}

		$mibPerSecond = $bytesPerSecond / 1MB
		if ($mibPerSecond -le $QuietDiskThresholdMiBPerSec) {
			$quietSamples += 1
			Write-Host ("[{0}] quiet-disk sample {1}/{2}: {3:N1} MiB/s <= {4:N1} MiB/s" -f $Label, $quietSamples, $QuietDiskConsecutiveSamples, $mibPerSecond, $QuietDiskThresholdMiBPerSec)
			if ($quietSamples -ge $QuietDiskConsecutiveSamples) {
				return
			}
		} else {
			Write-Host ("[{0}] disk still busy: {1:N1} MiB/s > {2:N1} MiB/s; resetting quiet streak" -f $Label, $mibPerSecond, $QuietDiskThresholdMiBPerSec)
			$quietSamples = 0
		}
	}

	Write-Warning ("[{0}] quiet-disk gate timed out after {1}s; continuing anyway." -f $Label, $QuietDiskTimeoutSeconds)
}

function Invoke-CargoLogged {
	param(
		[Parameter(Mandatory)] [string]$WorkingDirectory,
		[Parameter(Mandatory)] [string[]]$Arguments,
		[Parameter(Mandatory)] [string]$LogPath,
		[Parameter(Mandatory)] [string]$Label
	)

	$commandLine = 'cargo ' + ($Arguments -join ' ')
	if ($DryRun) {
		Write-Host ("[dry-run] ({0}) {1}" -f $WorkingDirectory, $commandLine)
		Set-Content -Path $LogPath -Value ("[dry-run] {0}" -f $commandLine) -Encoding UTF8
		return '[dry-run]'
	}

	Push-Location $WorkingDirectory
	try {
		$output = & cargo @Arguments 2>&1 | Tee-Object -FilePath $LogPath
		$text = $output | Out-String
		if ($LASTEXITCODE -ne 0) {
			throw "$Label failed. See $LogPath"
		}

		return $text
	} finally {
		Pop-Location
	}
}

function Invoke-ExactRun {
	param(
		[Parameter(Mandatory)] [string]$WorkingDirectory,
		[Parameter(Mandatory)] [string]$ExecutablePath,
		[Parameter(Mandatory)] [string]$StdoutPath,
		[Parameter(Mandatory)] [string]$StderrPath,
		[Parameter(Mandatory)] [string]$Label
	)

	if ($DryRun) {
		$message = "[dry-run] $ExecutablePath"
		Set-Content -Path $StdoutPath -Value $message -Encoding UTF8
		Set-Content -Path $StderrPath -Value '' -Encoding UTF8
		return [pscustomobject]@{
			stdout = $message
			stderr = ''
			elapsed_seconds = $null
			wall_seconds = $null
		}
	}

	$start = Get-Date
	$process = Start-Process -FilePath $ExecutablePath -WorkingDirectory $WorkingDirectory -RedirectStandardOutput $StdoutPath -RedirectStandardError $StderrPath -PassThru -Wait -NoNewWindow
	$end = Get-Date

	if ($process.ExitCode -ne 0) {
		throw "$Label failed with exit code $($process.ExitCode). See $StdoutPath and $StderrPath"
	}

	$stdout = Get-Content -Path $StdoutPath -Raw
	$stderr = Get-Content -Path $StderrPath -Raw

	[pscustomobject]@{
		stdout = $stdout
		stderr = $stderr
		elapsed_seconds = Get-ExactElapsedSeconds -Text $stdout
		wall_seconds = [Math]::Round(($end - $start).TotalSeconds, 3)
	}
}

function New-BaselineWorktree {
	param(
		[Parameter(Mandatory)] [string]$RepoRoot,
		[Parameter(Mandatory)] [string]$WorktreePath,
		[Parameter(Mandatory)] [string]$Ref
	)

	if ($DryRun) {
		Write-Host ("[dry-run] git worktree add --detach `"{0}`" {1}" -f $WorktreePath, $Ref)
		return
	}

	if (Test-Path $WorktreePath) {
		Push-Location $RepoRoot
		try {
			& git worktree remove --force $WorktreePath 2>$null | Out-Null
			if ($LASTEXITCODE -ne 0 -and (Test-Path $WorktreePath)) {
				Remove-Item -Recurse -Force $WorktreePath
			}
		} finally {
			Pop-Location
		}
	}

	Push-Location $RepoRoot
	try {
		& git worktree add --detach $WorktreePath $Ref | Out-Null
		if ($LASTEXITCODE -ne 0) {
			throw "Failed to create detached worktree for '$Ref'."
		}
	} finally {
		Pop-Location
	}
}

function Remove-BaselineWorktree {
	param(
		[Parameter(Mandatory)] [string]$RepoRoot,
		[Parameter(Mandatory)] [string]$WorktreePath
	)

	if ($DryRun -or $KeepBaselineWorktree -or -not (Test-Path $WorktreePath)) {
		return
	}

	Push-Location $RepoRoot
	try {
		& git worktree remove --force $WorktreePath | Out-Null
		if ($LASTEXITCODE -ne 0 -and (Test-Path $WorktreePath)) {
			Remove-Item -Recurse -Force $WorktreePath
		}
		& git worktree prune | Out-Null
	} finally {
		Pop-Location
	}
}

function Get-BenchEnv {
	param(
		[Parameter(Mandatory)] [string]$CargoTargetDir,
		[switch]$EnableProfiles
	)

	return @{
		USN_RAW_MFT_BENCH_DRIVE = $Drive
		USN_RAW_MFT_BENCH_WORKERS = $Workers
		USN_RAW_MFT_BENCH_WORKERS_LIST = $null
		USN_RAW_MFT_BENCH_CHUNK_RECORDS = $ChunkRecords
		USN_RAW_MFT_BENCH_BUFFER_BYTES = $MainBufferBytes
		USN_RAW_MFT_BENCH_ATTR_BUFFER_BYTES = $AttrBufferBytes
		USN_RAW_MFT_BENCH_START_RECORD = $StartRecord
		USN_RAW_MFT_BENCH_END_RECORD = $EndRecord
		USN_RAW_MFT_BENCH_SCHEDULING = $Scheduling
		USN_RAW_MFT_BENCH_SCHEDULING_LIST = $null
		USN_RAW_MFT_BENCH_INCLUDE_SERIAL = $null
		USN_RAW_MFT_BENCH_SUMMARY_ATTR_LIST_LIGHT = $(if ($SummaryLight) { '1' } else { $null })
		USN_RAW_MFT_BENCH_ATTR_LIST_SORT_BY_OFFSET = $(if ($SortAttrListByOffset) { '1' } else { $null })
		USN_RAW_MFT_BENCH_PRINT_SUMMARY = $null
		USN_RAW_MFT_BENCH_SUMMARY_RUNS = $null
		USN_RAW_MFT_BENCH_PRINT_ATTR_LIST_PROFILE = $(if ($EnableProfiles -and $PrintAttrListProfile) { '1' } else { $null })
		USN_RAW_MFT_BENCH_PRINT_SCHEDULING_PROFILE = $(if ($EnableProfiles -and $PrintSchedulingProfile) { '1' } else { $null })
		USN_RAW_MFT_BENCH_DEFERRED_ATTR_LIST = $(if ($DeferredAttrList) { '1' } else { '0' })
		USN_RAW_MFT_BENCH_DEFERRED_ATTR_LIST_WINDOW_RECORDS = $(if ($hasDeferredAttrListWindowRecords) { $DeferredAttrListWindowRecords } else { $null })
		USN_RAW_MFT_BENCH_COST_HINT_ATTR_SAMPLE = $(if ($CostHintAttrSample) { '1' } else { $null })
		CARGO_TARGET_DIR = $CargoTargetDir
		CARGO_TERM_COLOR = 'never'
	}
}

function Invoke-CriterionComparison {
	param(
		[Parameter(Mandatory)] [string]$RepoRoot,
		[Parameter(Mandatory)] [string]$BaselineWorktree,
		[Parameter(Mandatory)] [string]$SharedTargetDir,
		[Parameter(Mandatory)] [string]$CriterionDir
	)

	$baselineEnv = @{}
	$currentEnv = @{}
	$baselineLog = Join-Path $CriterionDir 'baseline.log.txt'
	$currentLog = Join-Path $CriterionDir 'current.log.txt'

	$baselineArgs = @(
		'bench', '--bench', 'raw_mft_ingest', '--',
		'--sample-size', $SampleSize,
		'--warm-up-time', $WarmUpSeconds,
		'--measurement-time', $MeasurementSeconds,
		'--save-baseline', $CriterionBaselineName
	)
	$currentArgs = @(
		'bench', '--bench', 'raw_mft_ingest', '--',
		'--sample-size', $SampleSize,
		'--warm-up-time', $WarmUpSeconds,
		'--measurement-time', $MeasurementSeconds,
		'--baseline', $CriterionBaselineName
	)

	try {
		Set-ScopedEnv -Values (Get-BenchEnv -CargoTargetDir $SharedTargetDir) -Previous $baselineEnv
		Wait-ForQuieterDisk -Label 'criterion-baseline'
		$baselineText = Invoke-CargoLogged -WorkingDirectory $BaselineWorktree -Arguments $baselineArgs -LogPath $baselineLog -Label 'Criterion baseline run'
	} finally {
		Restore-ScopedEnv -Previous $baselineEnv
	}

	try {
		Set-ScopedEnv -Values (Get-BenchEnv -CargoTargetDir $SharedTargetDir) -Previous $currentEnv
		Wait-ForQuieterDisk -Label 'criterion-current'
		$currentText = Invoke-CargoLogged -WorkingDirectory $RepoRoot -Arguments $currentArgs -LogPath $currentLog -Label 'Criterion current run'
	} finally {
		Restore-ScopedEnv -Previous $currentEnv
	}

	return [pscustomobject]@{
		baseline_log = $baselineLog
		current_log = $currentLog
		baseline = Get-CriterionSummary -Text $baselineText
		current = Get-CriterionSummary -Text $currentText
	}
}

function Invoke-ExactMatchComparison {
	param(
		[Parameter(Mandatory)] [string]$RepoRoot,
		[Parameter(Mandatory)] [string]$BaselineWorktree,
		[Parameter(Mandatory)] [string]$ComparisonDir
	)

	$baselineTargetDir = Join-Path $ComparisonDir 'baseline-target'
	$currentTargetDir = Join-Path $ComparisonDir 'current-target'
	$baselineRunsDir = Join-Path $ComparisonDir 'baseline-runs'
	$currentRunsDir = Join-Path $ComparisonDir 'current-runs'
	$baselineExe = Join-Path $baselineTargetDir 'release\examples\raw_mft_parallel_ingest_profile.exe'
	$currentExe = Join-Path $currentTargetDir 'release\examples\raw_mft_parallel_ingest_profile.exe'
	$baselineEnv = @{}
	$currentEnv = @{}
	$baselineElapsed = New-Object System.Collections.Generic.List[double]
	$currentElapsed = New-Object System.Collections.Generic.List[double]
	$baselineWall = New-Object System.Collections.Generic.List[double]
	$currentWall = New-Object System.Collections.Generic.List[double]

	New-Item -ItemType Directory -Force -Path $baselineRunsDir, $currentRunsDir | Out-Null

	if (-not $SkipBuild) {
		try {
			Set-ScopedEnv -Values (Get-BenchEnv -CargoTargetDir $baselineTargetDir) -Previous $baselineEnv
			Wait-ForQuieterDisk -Label 'exact-build-baseline'
			Invoke-CargoLogged -WorkingDirectory $BaselineWorktree -Arguments @('build', '--release', '--example', 'raw_mft_parallel_ingest_profile') -LogPath (Join-Path $ComparisonDir 'baseline-build.log.txt') -Label 'Baseline exact-match build' | Out-Null
		} finally {
			Restore-ScopedEnv -Previous $baselineEnv
		}

		try {
			Set-ScopedEnv -Values (Get-BenchEnv -CargoTargetDir $currentTargetDir) -Previous $currentEnv
			Wait-ForQuieterDisk -Label 'exact-build-current'
			Invoke-CargoLogged -WorkingDirectory $RepoRoot -Arguments @('build', '--release', '--example', 'raw_mft_parallel_ingest_profile') -LogPath (Join-Path $ComparisonDir 'current-build.log.txt') -Label 'Current exact-match build' | Out-Null
		} finally {
			Restore-ScopedEnv -Previous $currentEnv
		}
	}

	for ($index = 1; $index -le $ExactRuns; $index += 1) {
		$baselineRunDir = Join-Path $baselineRunsDir ('run-{0:D2}' -f $index)
		$currentRunDir = Join-Path $currentRunsDir ('run-{0:D2}' -f $index)
		New-Item -ItemType Directory -Force -Path $baselineRunDir, $currentRunDir | Out-Null

		try {
			Set-ScopedEnv -Values (Get-BenchEnv -CargoTargetDir $baselineTargetDir -EnableProfiles) -Previous $baselineEnv
			Wait-ForQuieterDisk -Label ("exact-baseline-{0:D2}" -f $index)
			$result = Invoke-ExactRun -WorkingDirectory $BaselineWorktree -ExecutablePath $baselineExe -StdoutPath (Join-Path $baselineRunDir 'stdout.txt') -StderrPath (Join-Path $baselineRunDir 'stderr.txt') -Label ("Baseline exact-match run #{0}" -f $index)
			if ($null -ne $result.elapsed_seconds) {
				$baselineElapsed.Add($result.elapsed_seconds)
				$baselineWall.Add($result.wall_seconds)
			}
		} finally {
			Restore-ScopedEnv -Previous $baselineEnv
		}

		try {
			Set-ScopedEnv -Values (Get-BenchEnv -CargoTargetDir $currentTargetDir -EnableProfiles) -Previous $currentEnv
			Wait-ForQuieterDisk -Label ("exact-current-{0:D2}" -f $index)
			$result = Invoke-ExactRun -WorkingDirectory $RepoRoot -ExecutablePath $currentExe -StdoutPath (Join-Path $currentRunDir 'stdout.txt') -StderrPath (Join-Path $currentRunDir 'stderr.txt') -Label ("Current exact-match run #{0}" -f $index)
			if ($null -ne $result.elapsed_seconds) {
				$currentElapsed.Add($result.elapsed_seconds)
				$currentWall.Add($result.wall_seconds)
			}
		} finally {
			Restore-ScopedEnv -Previous $currentEnv
		}
	}

	$baselineElapsedArray = $baselineElapsed.ToArray()
	$currentElapsedArray = $currentElapsed.ToArray()
	$baselineWallArray = $baselineWall.ToArray()
	$currentWallArray = $currentWall.ToArray()
	$deltaPercent = $null
	if ($baselineElapsedArray.Length -gt 0 -and $currentElapsedArray.Length -gt 0) {
		$baselineMedian = (Get-NumberSummary -Values $baselineElapsedArray).median_seconds
		$currentMedian = (Get-NumberSummary -Values $currentElapsedArray).median_seconds
		if ($baselineMedian -ne 0) {
			$deltaPercent = (($currentMedian - $baselineMedian) / $baselineMedian) * 100.0
		}
	}

	return [pscustomobject]@{
		baseline_elapsed = $baselineElapsedArray
		current_elapsed = $currentElapsedArray
		baseline_wall = $baselineWallArray
		current_wall = $currentWallArray
		baseline_elapsed_summary = $(if ($baselineElapsedArray.Length -gt 0) { Get-NumberSummary -Values $baselineElapsedArray } else { $null })
		current_elapsed_summary = $(if ($currentElapsedArray.Length -gt 0) { Get-NumberSummary -Values $currentElapsedArray } else { $null })
		baseline_wall_summary = $(if ($baselineWallArray.Length -gt 0) { Get-NumberSummary -Values $baselineWallArray } else { $null })
		current_wall_summary = $(if ($currentWallArray.Length -gt 0) { Get-NumberSummary -Values $currentWallArray } else { $null })
		median_delta_percent = $deltaPercent
		baseline_runs_dir = $baselineRunsDir
		current_runs_dir = $currentRunsDir
	}
}

if ($SkipCriterion -and $SkipExactMatch) {
	throw 'Nothing to do: both -SkipCriterion and -SkipExactMatch were provided.'
}

if (-not $DryRun -and -not (Test-IsElevated)) {
	throw 'Raw volume access requires an elevated PowerShell session.'
}

$null = Get-RequiredCommand -Name 'cargo'
$null = Get-RequiredCommand -Name 'git'

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$criterionDir = Join-Path $OutputRoot 'criterion'
$exactDir = Join-Path $OutputRoot 'exact-match'
$criterionTargetDir = Join-Path $criterionDir 'cargo-target'
$baselineWorktree = Join-Path $OutputRoot 'baseline-worktree'
$summaryPath = Join-Path $OutputRoot 'summary.json'
$summaryReadme = Join-Path $OutputRoot 'README.md'

New-Item -ItemType Directory -Force -Path $OutputRoot, $criterionDir, $exactDir | Out-Null

$currentHead = $null
Push-Location $repoRoot
try {
	if ($DryRun) {
		$currentHead = '[dry-run]'
	} else {
		$currentHead = (& git rev-parse --short HEAD).Trim()
	}
} finally {
	Pop-Location
}

$summary = [ordered]@{
	generated_at = (Get-Date).ToString('o')
	repo_root = $repoRoot
	current_head = $currentHead
	baseline_ref = $BaselineRef
	output_root = $OutputRoot
	quiet_disk = [ordered]@{
		threshold_mib_per_sec = $QuietDiskThresholdMiBPerSec
		consecutive_samples = $QuietDiskConsecutiveSamples
		sample_seconds = $QuietDiskSampleSeconds
		timeout_seconds = $QuietDiskTimeoutSeconds
	}
	workload = [ordered]@{
		drive = $Drive
		scheduling = $Scheduling
		workers = $Workers
		chunk_records = $ChunkRecords
		main_buffer_bytes = $MainBufferBytes
		attr_buffer_bytes = $AttrBufferBytes
		start_record = $StartRecord
		end_record = $EndRecord
		summary_light = [bool]$SummaryLight
		sort_attr_list_by_offset = [bool]$SortAttrListByOffset
		print_attr_list_profile = [bool]$PrintAttrListProfile
		print_scheduling_profile = [bool]$PrintSchedulingProfile
		deferred_attr_list = [bool]$DeferredAttrList
		deferred_attr_list_window_records = $(if ($hasDeferredAttrListWindowRecords) { $DeferredAttrListWindowRecords } else { $null })
		cost_hint_attr_sample = [bool]$CostHintAttrSample
	}
}

New-BaselineWorktree -RepoRoot $repoRoot -WorktreePath $baselineWorktree -Ref $BaselineRef
try {
	if (-not $SkipCriterion) {
		$criterionResult = Invoke-CriterionComparison -RepoRoot $repoRoot -BaselineWorktree $baselineWorktree -SharedTargetDir $criterionTargetDir -CriterionDir $criterionDir
		$summary['criterion'] = $criterionResult
	}

	if (-not $SkipExactMatch) {
		$exactResult = Invoke-ExactMatchComparison -RepoRoot $repoRoot -BaselineWorktree $baselineWorktree -ComparisonDir $exactDir
		$summary['exact_match'] = $exactResult
	}
} finally {
	Remove-BaselineWorktree -RepoRoot $repoRoot -WorktreePath $baselineWorktree
}

$summary | ConvertTo-Json -Depth 8 | Set-Content -Path $summaryPath -Encoding UTF8

$readmeLines = New-Object System.Collections.Generic.List[string]
$readmeLines.Add('# Raw MFT ingest measurement run')
$readmeLines.Add('')
$readmeLines.Add(('* Generated at: `{0}`' -f $summary.generated_at))
$readmeLines.Add(('* Current worktree HEAD: `{0}`' -f $summary.current_head))
$readmeLines.Add(('* Baseline ref: `{0}`' -f $summary.baseline_ref))
$readmeLines.Add(('* Output root: `{0}`' -f $summary.output_root))
$readmeLines.Add('')
$readmeLines.Add('## Workload shape')
$readmeLines.Add('')
$readmeLines.Add(('* Drive: `{0}`' -f $summary.workload.drive))
$readmeLines.Add(('* Scheduling: `{0}`' -f $summary.workload.scheduling))
$readmeLines.Add(('* Workers: `{0}`' -f $summary.workload.workers))
$readmeLines.Add(('* Chunk records: `{0}`' -f $summary.workload.chunk_records))
$readmeLines.Add(('* Main buffer bytes: `{0}`' -f $summary.workload.main_buffer_bytes))
$readmeLines.Add(('* Attr buffer bytes: `{0}`' -f $summary.workload.attr_buffer_bytes))
$readmeLines.Add(('* Start record: `{0}`' -f $summary.workload.start_record))
$readmeLines.Add(('* End record: `{0}`' -f $(if ($summary.workload.end_record) { $summary.workload.end_record } else { 'full' })))
$readmeLines.Add(('* Summary-light: `{0}`' -f $summary.workload.summary_light))
$readmeLines.Add(('* Sort attr-list by offset: `{0}`' -f $summary.workload.sort_attr_list_by_offset))
$readmeLines.Add('')

if ($summary.Contains('criterion')) {
	$readmeLines.Add('## Criterion')
	$readmeLines.Add('')
	$readmeLines.Add(('* Baseline log: `{0}`' -f $summary.criterion.baseline_log))
	$readmeLines.Add(('* Current log: `{0}`' -f $summary.criterion.current_log))
	$readmeLines.Add(('* Baseline result: `{0}`' -f $(if ($summary.criterion.baseline.time) { $summary.criterion.baseline.time } else { 'n/a' })))
	$readmeLines.Add(('* Current result: `{0}`' -f $(if ($summary.criterion.current.time) { $summary.criterion.current.time } else { 'n/a' })))
	if ($summary.criterion.current.change) {
		$readmeLines.Add(('* Change vs baseline: `{0}` (p = {1})' -f $summary.criterion.current.change, $summary.criterion.current.p_value))
	}
	if ($summary.criterion.current.verdict) {
		$readmeLines.Add(('* Verdict: `{0}`' -f $summary.criterion.current.verdict))
	}
	$readmeLines.Add('')
}

if ($summary.Contains('exact_match')) {
	$readmeLines.Add('## Exact-match repeated runs')
	$readmeLines.Add('')
	$readmeLines.Add(('* Baseline runs: `{0}`' -f $summary.exact_match.baseline_runs_dir))
	$readmeLines.Add(('* Current runs: `{0}`' -f $summary.exact_match.current_runs_dir))
	if ($summary.exact_match.baseline_elapsed_summary) {
		$readmeLines.Add(('* Baseline elapsed median: `{0}` (mean `{1}`, min `{2}`, max `{3}`)' -f (Format-Seconds -Seconds $summary.exact_match.baseline_elapsed_summary.median_seconds), (Format-Seconds -Seconds $summary.exact_match.baseline_elapsed_summary.mean_seconds), (Format-Seconds -Seconds $summary.exact_match.baseline_elapsed_summary.min_seconds), (Format-Seconds -Seconds $summary.exact_match.baseline_elapsed_summary.max_seconds)))
	}
	if ($summary.exact_match.current_elapsed_summary) {
		$readmeLines.Add(('* Current elapsed median: `{0}` (mean `{1}`, min `{2}`, max `{3}`)' -f (Format-Seconds -Seconds $summary.exact_match.current_elapsed_summary.median_seconds), (Format-Seconds -Seconds $summary.exact_match.current_elapsed_summary.mean_seconds), (Format-Seconds -Seconds $summary.exact_match.current_elapsed_summary.min_seconds), (Format-Seconds -Seconds $summary.exact_match.current_elapsed_summary.max_seconds)))
	}
	if ($null -ne $summary.exact_match.median_delta_percent) {
		$readmeLines.Add(('* Median delta vs baseline: `{0:N2}%`' -f $summary.exact_match.median_delta_percent))
	}
	$readmeLines.Add('')
}

$readmeLines.Add('See also:')
$readmeLines.Add('')
$readmeLines.Add(('* `summary.json` for machine-readable output'))
$readmeLines.Add(('* the per-run `stdout.txt` / `stderr.txt` files under `exact-match`'))
$readmeLines.Add(('* the Criterion logs under `criterion`'))

$readmeLines | Set-Content -Path $summaryReadme -Encoding UTF8

Write-Host ''
Write-Host 'Raw MFT ingest measurement summary'
Write-Host ('  output root: {0}' -f $OutputRoot)
if ($summary.Contains('criterion')) {
	Write-Host ('  Criterion baseline: {0}' -f $(if ($summary.criterion.baseline.time) { $summary.criterion.baseline.time } else { 'n/a' }))
	Write-Host ('  Criterion current:  {0}' -f $(if ($summary.criterion.current.time) { $summary.criterion.current.time } else { 'n/a' }))
	if ($summary.criterion.current.change) {
		Write-Host ('  Criterion change:   {0}' -f $summary.criterion.current.change)
	}
	if ($summary.criterion.current.verdict) {
		Write-Host ('  Criterion verdict:  {0}' -f $summary.criterion.current.verdict)
	}
}
if ($summary.Contains('exact_match') -and $summary.exact_match.baseline_elapsed_summary -and $summary.exact_match.current_elapsed_summary) {
	Write-Host ('  Exact baseline median: {0}' -f (Format-Seconds -Seconds $summary.exact_match.baseline_elapsed_summary.median_seconds))
	Write-Host ('  Exact current median:  {0}' -f (Format-Seconds -Seconds $summary.exact_match.current_elapsed_summary.median_seconds))
	if ($null -ne $summary.exact_match.median_delta_percent) {
		Write-Host ('  Exact median delta:    {0:N2}%' -f $summary.exact_match.median_delta_percent)
	}
}
Write-Host ('  summary json: {0}' -f $summaryPath)
Write-Host ('  summary readme: {0}' -f $summaryReadme)




