use usn_journal_rs::{errors::UsnError, volume::Volume};

pub fn open_raw_volume(source: Option<String>) -> Result<(Volume, String), UsnError> {
    #[cfg(windows)]
    {
        let drive = source
            .as_deref()
            .and_then(|value| value.chars().next())
            .unwrap_or('C')
            .to_ascii_uppercase();
        Ok((Volume::from_drive_letter(drive)?, format!("{drive}:")))
    }
    #[cfg(target_os = "linux")]
    {
        let source = source.ok_or_else(|| {
            UsnError::InvalidMountPoint("pass an NTFS mount point or device path".into())
        })?;
        let volume = if source.starts_with("/dev/") {
            Volume::from_device_path(&source)?
        } else {
            Volume::from_mount_point(&source)?
        };
        Ok((volume, source))
    }
}
