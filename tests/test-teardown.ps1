# This script unmounts and deletes a VHD file and its mount point in PowerShell.
# This script is designed to be run with administrator privileges.
# Note: This script needs the Hyper-V feature to be enabled on the system.

param(
    [Parameter(Mandatory = $true)]
    [string]$VhdPath,
    
    [Parameter(Mandatory = $true)]
    [string]$MountPath
)

# Set error action preference to stop on errors
$ErrorActionPreference = "Stop"

# Ensure the script is running with administrator privileges
if (-NOT ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")) {
    Write-Error "This script requires administrator privileges. Please run as Administrator."
    exit 1
}

# Dismount the VHD
$disk = Get-Disk | Where-Object { $_.Location -eq $VhdPath }
if ($disk) {
    Write-Host "Dismounting VHD..."
    Dismount-VHD -Path $VhdPath -Confirm:$false
}

# Try delete the VHD file and the mount path
if (Test-Path -Path $VhdPath) {
    Write-Host "Deleting VHD file at $VhdPath..."
    Remove-Item -Path $VhdPath -Confirm:$false -ErrorAction SilentlyContinue
}
else {
    Write-Host "No VHD file found at $VhdPath."
}

if (Test-Path -Path $MountPath) {
    Write-Host "Deleting mount path $MountPath..."
    Remove-Item -Path $MountPath -Recurse -Force -Confirm:$false -ErrorAction SilentlyContinue
}
else {
    Write-Host "No mount path found at $MountPath."
}
