#![allow(dead_code)]

use std::path::PathBuf;

use uuid::Uuid;

pub(crate) const VHD_MOUNT_POINT_BASE: &str = "test-vhd-mount-point";
pub(crate) const VHD_NAME_BASE_NAME: &str = "usn-journal-test";
pub(crate) const VHD_EXT: &str = "vhdx";

pub(crate) fn get_workspace_root() -> anyhow::Result<PathBuf> {
    let workspace_root = std::env::var("CARGO_WORKSPACE_DIR").or_else(|_| {
        let crate_dir = std::env::var("CARGO_MANIFEST_DIR")?;
        std::path::Path::new(&crate_dir)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .ok_or(std::env::VarError::NotPresent)
    })?;
    println!("Current workspace root: {}", workspace_root);

    Ok(PathBuf::from(workspace_root))
}

pub(crate) fn setup() -> anyhow::Result<(PathBuf, Uuid)> {
    const SETUP_SCRIPT_NAME: &str = "test-setup.ps1";

    let workspace_root = get_workspace_root()?;
    let script_path = workspace_root.join(SETUP_SCRIPT_NAME);

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
        return Err(anyhow::anyhow!(
            "Failed to run {}: {}",
            SETUP_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    println!(
        "VHDX file created and mounted successfully at {}",
        mount_point.display()
    );

    Ok((mount_point, uuid))
}

pub(crate) fn teardown(uuid: Uuid) -> anyhow::Result<()> {
    const TEARDOWN_SCRIPT_NAME: &str = "test-teardown.ps1";

    let workspace_root = get_workspace_root()?;
    let script_path = workspace_root.join(TEARDOWN_SCRIPT_NAME);

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
        return Err(anyhow::anyhow!(
            "Failed to run {}: {}",
            TEARDOWN_SCRIPT_NAME,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    println!(
        "VHDX file unmounted and deleted successfully at {}",
        mount_point.display()
    );

    Ok(())
}
