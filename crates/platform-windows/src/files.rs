//! Filesystem helpers: known folders, atomic preferences, disk space, safe moves.
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{System::Com::*, UI::Shell::*},
    core::{PCWSTR, Result as WinResult},
};

pub fn default_recording_directory() -> Result<PathBuf, String> {
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None)
            .map_err(|e| e.to_string())?;
        let result = path
            .to_string()
            .map(|p| PathBuf::from(p).join("FastRecorder"))
            .map_err(|e| e.to_string());
        CoTaskMemFree(Some(path.0.cast()));
        let directory = result?;
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        Ok(directory)
    }
}

pub fn preferences_path() -> Result<PathBuf, String> {
    unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)
            .map_err(|e| e.to_string())?;
        let result = raw
            .to_string()
            .map(PathBuf::from)
            .map_err(|e| e.to_string());
        CoTaskMemFree(Some(raw.0.cast()));
        Ok(result?.join("FastRecorder").join("preferences.json"))
    }
}

pub fn save_preferences(bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let path = preferences_path()?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            windows::Win32::Storage::FileSystem::MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                windows::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
            .map_err(|e| e.to_string())
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

pub(crate) fn available_disk_space(path: &Path) -> Result<u64, String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    unsafe {
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut available),
            None,
            None,
        )
        .map_err(|e| format!("Could not check free disk space: {e}"))?;
    }
    Ok(available)
}

pub(crate) fn move_without_overwrite(from: &Path, to: &Path) -> WinResult<()> {
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // No MOVEFILE_REPLACE_EXISTING: a file created since validation is protected.
    unsafe {
        windows::Win32::Storage::FileSystem::MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn finalization_never_replaces_a_destination_created_after_validation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fastrecorder-move-{stamp}"));
        std::fs::create_dir(&dir).unwrap();
        let temporary = dir.join("temporary.mp4");
        let destination = dir.join("recording.mp4");
        std::fs::write(&temporary, b"new recording").unwrap();
        std::fs::write(&destination, b"existing recording").unwrap();
        assert!(move_without_overwrite(&temporary, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing recording");
        assert!(temporary.exists());
        std::fs::remove_file(&destination).unwrap();
        move_without_overwrite(&temporary, &destination).unwrap();
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"new recording");
        std::fs::remove_file(&destination).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
