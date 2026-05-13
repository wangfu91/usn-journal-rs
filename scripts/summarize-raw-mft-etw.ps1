[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$EtlPath,

    [string]$OutputDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RequiredCommand {
    param([Parameter(Mandatory)] [string]$Name)

    $command = Get-Command -Name $Name -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        throw "Required command '$Name' was not found in PATH."
    }
    return $command.Source
}

$EtlPath = [IO.Path]::GetFullPath($EtlPath)
if (-not (Test-Path $EtlPath)) {
    throw "ETL file '$EtlPath' does not exist."
}

$xperfPath = Get-RequiredCommand -Name 'xperf'
if (-not $PSBoundParameters.ContainsKey('OutputDir')) {
    $OutputDir = Join-Path ([IO.Path]::GetDirectoryName($EtlPath)) 'xperf'
}
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$reports = @(
    @{ Name = 'diskio'; Args = @('-i', $EtlPath, '-a', 'diskio') },
    @{ Name = 'diskio-detail'; Args = @('-i', $EtlPath, '-a', 'diskio', '-detail') },
    @{ Name = 'cpudisk'; Args = @('-i', $EtlPath, '-a', 'cpudisk') },
    @{ Name = 'process'; Args = @('-i', $EtlPath, '-a', 'process') },
    @{ Name = 'filename'; Args = @('-i', $EtlPath, '-a', 'filename') }
)

$summaryLines = New-Object System.Collections.Generic.List[string]
$summaryLines.Add('# ETW xperf summaries')
$summaryLines.Add('')
$summaryLines.Add(('- ETL: `{0}`' -f $EtlPath))
$summaryLines.Add(('- xperf: `{0}`' -f $xperfPath))
$summaryLines.Add('')
$summaryLines.Add('| Report | Status | Output |')
$summaryLines.Add('| --- | --- | --- |')

foreach ($report in $reports) {
    $stdoutPath = Join-Path $OutputDir ($report.Name + '.txt')
    $stderrPath = Join-Path $OutputDir ($report.Name + '.stderr.txt')

    Write-Host "Generating xperf report '$($report.Name)'..."
    $process = Start-Process -FilePath $xperfPath -ArgumentList $report.Args -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -PassThru -Wait -NoNewWindow

    if ($process.ExitCode -eq 0) {
        if ((Test-Path $stderrPath) -and [string]::IsNullOrWhiteSpace((Get-Content -Raw -Path $stderrPath))) {
            Remove-Item $stderrPath -ErrorAction SilentlyContinue
        }
        $summaryLines.Add("| $($report.Name) | ok | `$($stdoutPath)` |")
    } else {
        $summaryLines.Add("| $($report.Name) | failed ($($process.ExitCode)) | `$($stderrPath)` |")
    }
}

$summaryLines.Add('')
$summaryLines.Add('## WPA follow-up checklist')
$summaryLines.Add('')
$summaryLines.Add('- Open the `.etl` in WPA.')
$summaryLines.Add('- Start with Disk Usage, CPU Usage (Precise), and Generic Events / File I/O views.')
$summaryLines.Add('- Check whether the ingest process is disk-saturated, queue-depth-limited, or blocked behind background I/O.')
$summaryLines.Add('- Compare the process read burst timing against any concurrent `System` writes or other storage-heavy processes.')

$summaryPath = Join-Path $OutputDir 'README.md'
$summaryLines | Set-Content -Path $summaryPath -Encoding UTF8
Write-Host "xperf summaries written to $OutputDir"


