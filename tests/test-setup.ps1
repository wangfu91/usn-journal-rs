# This script creates a VHD file, mounts it, initializes the disk, creates a partition, formats it with NTFS, and assigns a mount point.
# It requires administrator privileges to run.
# Note: This script needs the Hypwer-V feature to be enabled on the system.

param(
    [Parameter(Mandatory=$true)]
    [string]$VhdPath,
    
    [Parameter(Mandatory=$true)]
    [string]$MountPath
)

# Set error action preference to stop on errors
$ErrorActionPreference = "Stop"

# Ensure the script is running with administrator privileges
if (-NOT ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")) {
    Write-Error "This script requires administrator privileges. Please run as Administrator."
    exit 1
}

$disk = Get-Disk | Where-Object { $_.Location -eq $VhdPath }
if ($disk) {
    Write-Host "VHD already mounted at $MountPath, skipping..."
    exit 0
}

# Create directory for VHD if it doesn't exist
$vhdDirectory = Split-Path -Path $VhdPath -Parent
if (-not (Test-Path -Path $vhdDirectory)) {
    New-Item -Path $vhdDirectory -ItemType Directory -Force | Out-Null
}

# Create directory for mount point if it doesn't exist
if (-not (Test-Path -Path $MountPath)) {
    New-Item -Path $MountPath -ItemType Directory -Force | Out-Null
}

# Check if $MountPath is an empty directory, fail if not
if (Test-Path -Path $MountPath) {
    $files = Get-ChildItem -Path $MountPath
    if ($files.Count -gt 0) {
        Write-Error "Mount path $MountPath is not empty. Please provide an empty directory."
        exit 1
    }
}

try {
    # Create the VHD file (256MB in size)
    Write-Host "Creating VHD file at $VhdPath..."
    New-VHD -Path $VhdPath -SizeBytes 256MB -Dynamic -Confirm:$false
    
    # Mount the VHD
    Write-Host "Mounting VHD..."
    $disk = Mount-VHD -Path $VhdPath -PassThru
    
    # Initialize the disk
    Write-Host "Initializing disk..."
    Initialize-Disk -Number $disk.Number -PartitionStyle GPT -Confirm:$false
    
    # Create a new partition, format it with NTFS, and assign a mount point
    Write-Host "Creating partition, formatting with NTFS, and mounting to $MountPath..."
    $partition = New-Partition -DiskNumber $disk.Number -UseMaximumSize -AssignDriveLetter
    Format-Volume -DriveLetter $partition.DriveLetter -FileSystem NTFS -NewFileSystemLabel "TestVHD" -Confirm:$false | Out-Null
    
    # Remove the drive letter and add a mount point
    $driveLetter = $partition.DriveLetter
    Remove-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $partition.PartitionNumber -AccessPath "$($driveLetter):"
    Add-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $partition.PartitionNumber -AccessPath $MountPath
    
    Write-Host "VHD successfully created and mounted to $MountPath"

    # Create some random files/folder in the mount path for testing
    Write-Host "Creating test files in $MountPath..."
    New-Item -Path "$MountPath\TestFile1.txt" -ItemType File -Force | Out-Null
    New-Item -Path "$MountPath\TestFile2.txt" -ItemType File -Force | Out-Null
    New-Item -Path "$MountPath\TestFolder" -ItemType Directory -Force | Out-Null
    New-Item -Path "$MountPath\TestFolder\TestFile3.txt" -ItemType File -Force | Out-Null
    New-Item -Path "$MountPath\TestFolder\TestFile4.txt" -ItemType File -Force | Out-Null
    Write-Host "Test files created."
}
catch {
    Write-Error "An error occurred: $_"
    
    # Cleanup if something fails
    if ($disk) {
        Dismount-VHD -Path $VhdPath -ErrorAction SilentlyContinue
    }
    
    exit 1
}