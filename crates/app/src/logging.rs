//! Bounded local fatal diagnostics; never uploads or records desktop/audio data.
use std::{fs::OpenOptions, io::Write};

pub fn write(message: &str) {
    let Some(directory) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory).join("FastRecorder");
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let path = directory.join("diagnostics.log");
    let truncate = std::fs::metadata(&path).is_ok_and(|info| info.len() > 256 * 1024);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(path)
    {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(
            file,
            "[{timestamp}] FastRecorder {}: {message}",
            env!("CARGO_PKG_VERSION")
        );
    }
}

pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&format!("Fatal panic: {info}"));
        previous(info);
    }));
}
