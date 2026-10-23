use std::path::PathBuf;

/// Папка данных Modly: `%APPDATA%\Modly`.
/// Переменная окружения `MODLY_HOME` подменяет её (для тестов и портативного режима).
pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("MODLY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Modly")))
        .unwrap_or_else(|| exe_dir().join("ModlyData"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn library_file() -> PathBuf {
    data_dir().join("library.json")
}

pub fn log_file() -> PathBuf {
    data_dir().join("modly.log")
}

/// Папка, куда пользователь кладёт пакеты модов.
pub fn user_mods_dir() -> PathBuf {
    let dir = data_dir().join("mods");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Папки, где искать пакеты модов по умолчанию:
/// `%APPDATA%\Modly\mods`, `mods` рядом с exe и — если Modly запущен из `target` при разработке —
/// папка `mods` в корне репозитория.
pub fn default_mod_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![user_mods_dir(), exe_dir().join("mods")];
    let mut cur = exe_dir();
    for _ in 0..4 {
        if cur.file_name().is_some_and(|n| n.eq_ignore_ascii_case("target")) {
            if let Some(root) = cur.parent() {
                dirs.push(root.join("mods"));
            }
            break;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs.dedup();
    dirs
}
