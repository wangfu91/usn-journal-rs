#![allow(dead_code)]
use std::path::PathBuf;
use uuid::Uuid;

use crate::errors::UsnError;

pub const VHD_MOUNT_POINT_BASE: &str = "test-vhd-mount-point";
pub const VHD_NAME_BASE_NAME: &str = "usn-journal-test";
pub const VHD_EXT: &str = "vhdx";

pub fn get_workspace_root() -> Result<PathBuf, UsnError> {
    let workspace_root = std::env::var("CARGO_WORKSPACE_DIR")
        .or_else(|_| std::env::var("CARGO_MANIFEST_DIR"))
        .map_err(|e| UsnError::OtherError(format!("Failed to get workspace root: {}", e)))?;

    println!("Current workspace root: {}", workspace_root);

    Ok(PathBuf::from(workspace_root))
}

/// Set up a test VHD and mount point for integration tests.
///
/// Returns the mount point path and UUID. Runs a PowerShell script to create and mount a VHDX file.
pub fn setup() -> Result<(PathBuf, Uuid), UsnError> {
    const SETUP_SCRIPT_NAME: &str = "test-setup.ps1";

    let workspace_root = get_workspace_root()?;
    let script_path = workspace_root.join("tests").join(SETUP_SCRIPT_NAME);

    let uuid = Uuid::new_v4();
    let mount_point = workspace_root
        .join("target")
        .join(format!("{}-{}", VHD_MOUNT_POINT_BASE, uuid));
    let vhd_name = format!("{}-{}.{}", VHD_NAME_BASE_NAME, uuid, VHD_EXT);
    let vhd_path = workspace_root.join("target").join(vhd_name);
    println!("mount point: {}", mount_point.display());
    println!("vhd path: {}", vhd_path.display());

    let mount_point_clone = mount_point.clone();
    let vhd_path_clone = vhd_path.clone();

    // ./cargo-test-setup.ps1 -VhdPath $vhd_create_path -MountPath $mount_point
    let output = std::process::Command::new("powershell")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(script_path)
        .arg("-VhdPath")
        .arg(vhd_path_clone)
        .arg("-MountPath")
        .arg(mount_point_clone)
        .output()?;

    // Print stdout and stderr from the script
    if !output.stdout.is_empty() {
        println!(
            "{} stdout:\n{}",
            SETUP_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stdout)
        );
    }

    if !output.stderr.is_empty() {
        println!(
            "{} stderr:\n{}",
            SETUP_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    if !output.status.success() {
        return Err(UsnError::OtherError(format!(
            "Failed to run {}: {}",
            SETUP_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    println!(
        "VHDX file created and mounted successfully at {}",
        mount_point.display()
    );

    Ok((mount_point, uuid))
}

/// Tear down the test VHD and mount point created by `setup`.
///
/// Unmounts and deletes the VHDX file using a PowerShell script.
pub fn teardown(uuid: Uuid) -> Result<(), UsnError> {
    const TEARDOWN_SCRIPT_NAME: &str = "test-teardown.ps1";

    let workspace_root = get_workspace_root()?;
    let script_path = workspace_root.join("tests").join(TEARDOWN_SCRIPT_NAME);

    let mount_point = workspace_root
        .join("target")
        .join(format!("{}-{}", VHD_MOUNT_POINT_BASE, uuid));
    let vhd_name = format!("{}-{}.{}", VHD_NAME_BASE_NAME, uuid, VHD_EXT);
    let vhd_path = workspace_root.join("target").join(vhd_name);
    println!("mount point: {}", mount_point.display());
    println!("vhd path: {}", vhd_path.display());

    let mount_point_clone = mount_point.clone();
    let vhd_path_clone = vhd_path.clone();
    let output = std::process::Command::new("powershell")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(script_path)
        .arg("-VhdPath")
        .arg(vhd_path_clone)
        .arg("-MountPath")
        .arg(mount_point_clone)
        .output()?;
    // Print stdout and stderr from the script
    if !output.stdout.is_empty() {
        println!(
            "{} stdout:\n{}",
            TEARDOWN_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stdout)
        );
    }
    if !output.stderr.is_empty() {
        println!(
            "{} stderr:\n{}",
            TEARDOWN_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    if !output.status.success() {
        return Err(UsnError::OtherError(
            format!(
                "Failed to run {}: {}",
                TEARDOWN_SCRIPT_NAME,
                String::from_utf8_lossy(&output.stderr)
            )
            .to_string(),
        ));
    }
    println!(
        "VHDX file unmounted and deleted successfully at {}",
        mount_point.display()
    );

    Ok(())
}
