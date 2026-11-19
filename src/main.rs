use std::path::{Path, PathBuf};

/// The installed AppImage/deb/rpm does not carry the source-tree config next to the executable.
/// Embed the same file so a packaged app can materialise it in the per-user data directory.
const BUNDLED_CONFIG: &str = include_str!("../Auro.toml");

fn config_file() -> Option<PathBuf> {
    if std::env::var_os("AURO_CONFIG").is_some()
        || Path::new("Auro.toml").exists()
        || std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("Auro.toml")))
            .is_some_and(|path| path.exists())
    {
        return None;
    }

    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("auro-inspector");
    let path = dir.join("Auro.toml");
    if path.exists() {
        return Some(path);
    }
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "auro-inspector: cannot create config directory {}: {error}",
            dir.display()
        );
        return None;
    }
    if let Err(error) = std::fs::write(&path, BUNDLED_CONFIG) {
        eprintln!(
            "auro-inspector: cannot write packaged config {}: {error}",
            path.display()
        );
        return None;
    }
    Some(path)
}

fn main() {
    if let Some(path) = config_file() {
        // SAFETY: no worker threads or GPUI application exist yet; this runs before `run`.
        unsafe {
            std::env::set_var("AURO_CONFIG", path);
        }
    }
    auro_inspector_ui::run(auro_inspector_data::view_data);
}
