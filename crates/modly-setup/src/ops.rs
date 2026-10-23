//! Что установщик, обновлятор и удаление делают с файлами, ярлыками и реестром.
//!
//! Файлы меняются через «шлюз»: новые сначала распаковываются в `.modly-new`, старые переезжают
//! в `.modly-old`, и только потом новые встают на место. Если что-то пошло не так, старые
//! возвращаются — Modly не останется наполовину обновлённым.

use crate::system;
use anyhow::{Context, Result, bail};
use modly_core::update;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const NEW_DIR: &str = ".modly-new";
const OLD_DIR: &str = ".modly-old";
pub const INFO_FILE: &str = "install.json";

/// Что сообщить окну.
pub enum Report {
    Step(usize, &'static str),
    Progress(f32),
    Status(String),
}

pub type Reporter<'a> = &'a (dyn Fn(Report) + Sync);

/// Что выбрал человек в установщике.
#[derive(Clone, Debug)]
pub struct Options {
    pub dir: PathBuf,
    pub desktop: bool,
    pub start_menu: bool,
    pub protocol: bool,
}

/// `<папка>\install.json`: какая версия стоит и какие ярлыки создал установщик.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallInfo {
    pub version: String,
    pub shortcuts: Vec<PathBuf>,
    pub protocol: bool,
}

impl InstallInfo {
    pub fn load(dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(dir.join(INFO_FILE)).ok()?;
        // файл могли поправить руками — Блокнот и PowerShell пишут BOM
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        serde_json::from_slice(bytes).ok()
    }

    fn save(&self, dir: &Path) -> Result<()> {
        std::fs::write(dir.join(INFO_FILE), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

// --------------------------------------------------------------- установка ---

pub fn install(payload: &[u8], opts: &Options, version: &str, rep: Reporter) -> Result<()> {
    if payload.is_empty() {
        bail!("в этом установщике нет файлов Modly — скачайте установщик со страницы выпуска");
    }
    let dir = &opts.dir;

    rep(Report::Step(0, "active"));
    rep(Report::Status(t("Готовим папку…", "Preparing the folder…")));
    std::fs::create_dir_all(dir).with_context(|| format!("{}", dir.display()))?;
    if !system::dir_writable(dir) {
        bail!("{}", t("нет доступа к папке — выберите другую или запустите установщик от имени администратора", "no access to the folder — pick another one or run the setup as administrator"));
    }
    if !system::modly_pids(dir).is_empty() {
        bail!("{}", t("Modly всё ещё открыт — закройте его и повторите", "Modly is still running — close it and try again"));
    }
    rep(Report::Progress(0.08));
    rep(Report::Step(0, "done"));

    rep(Report::Step(1, "active"));
    rep(Report::Status(t("Копируем файлы Modly…", "Copying Modly files…")));
    let staging = dir.join(NEW_DIR);
    let _ = std::fs::remove_dir_all(&staging);
    let files = update::extract_package_from(std::io::Cursor::new(payload), &staging)?;
    rep(Report::Progress(0.45));
    swap_in(&staging, dir, &files, false)?;
    let _ = std::fs::remove_dir_all(&staging);
    rep(Report::Progress(0.7));
    rep(Report::Step(1, "done"));

    rep(Report::Step(2, "active"));
    rep(Report::Status(t("Создаём ярлыки…", "Creating shortcuts…")));
    let exe = dir.join(update::APP_EXE);
    let (start, desktop) = system::shortcut_paths();
    let mut shortcuts = Vec::new();
    for (want, lnk) in [(opts.start_menu, start), (opts.desktop, desktop)] {
        if want {
            system::create_shortcut(&lnk, &exe, "", "Modly — mod manager")?;
            shortcuts.push(lnk);
        } else if lnk.is_file() && previous_shortcut(dir, &lnk) {
            let _ = std::fs::remove_file(&lnk);
        }
    }
    rep(Report::Progress(0.82));
    rep(Report::Step(2, "done"));

    rep(Report::Step(3, "active"));
    rep(Report::Status(t("Регистрируем Modly в Windows…", "Registering Modly with Windows…")));
    if opts.protocol {
        system::register_protocol(&exe)?;
    } else {
        system::unregister_protocol_if_in(dir);
    }
    system::write_uninstall_entry(dir, version, dir_size_kb(dir))?;
    InstallInfo { version: version.to_owned(), shortcuts, protocol: opts.protocol }.save(dir)?;
    rep(Report::Progress(1.0));
    rep(Report::Step(3, "done"));
    log::info!("installed {version} into {}", dir.display());
    Ok(())
}

fn previous_shortcut(dir: &Path, lnk: &Path) -> bool {
    InstallInfo::load(dir).is_some_and(|i| i.shortcuts.iter().any(|s| s == lnk))
}

/// Поставить файлы `files` из `src` в `dir`: старые — в `.modly-old`, при ошибке вернуть их.
/// `copy` — копировать (исходная папка занята, например, запущенным обновлятором), иначе перенести.
pub fn swap_in(src: &Path, dir: &Path, files: &[PathBuf], copy: bool) -> Result<()> {
    let old = dir.join(OLD_DIR);
    let _ = std::fs::remove_dir_all(&old);
    std::fs::create_dir_all(&old)?;
    let mut moved: Vec<(PathBuf, bool)> = Vec::new(); // (файл, был ли старый)
    let result = (|| -> Result<()> {
        for f in files {
            let to = dir.join(f);
            let had_old = to.exists();
            if had_old {
                let backup = old.join(f);
                if let Some(p) = backup.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::rename(&to, &backup).with_context(|| format!("{} занят", to.display()))?;
            }
            moved.push((f.clone(), had_old));
            if let Some(p) = to.parent() {
                std::fs::create_dir_all(p)?;
            }
            let from = src.join(f);
            if copy {
                std::fs::copy(&from, &to).with_context(|| format!("{}", to.display()))?;
            } else {
                std::fs::rename(&from, &to).with_context(|| format!("{}", to.display()))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("swap failed, rolling back: {e:#}");
        for (f, had_old) in moved.iter().rev() {
            let to = dir.join(f);
            let _ = std::fs::remove_file(&to);
            if *had_old {
                let _ = std::fs::rename(old.join(f), &to);
            }
        }
        return Err(e);
    }
    // старый exe может быть ещё занят — тогда папка останется до следующего раза
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Вернуть файлы из `.modly-old` (новый Modly не запустился).
pub fn rollback(dir: &Path) -> Result<()> {
    let old = dir.join(OLD_DIR);
    if !old.is_dir() {
        bail!("старых файлов нет");
    }
    for entry in std::fs::read_dir(&old)?.flatten() {
        let to = dir.join(entry.file_name());
        let _ = std::fs::remove_file(&to);
        std::fs::rename(entry.path(), &to)?;
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

pub fn dir_size_kb(dir: &Path) -> u32 {
    let bytes: u64 = std::fs::read_dir(dir)
        .map(|it| it.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0);
    (bytes / 1024) as u32
}

// --------------------------------------------------------------- обновление ---

/// Поставить обновление: файлы — из папки распакованного пакета `src` (там же лежит этот
/// обновлятор), в папку Modly `dir`.
pub fn apply_update(src: &Path, dir: &Path, version: &str) -> Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(src)?.flatten() {
        let name = entry.file_name();
        let lower = name.to_string_lossy().to_lowercase();
        if entry.path().is_file() && !lower.ends_with(".log") && lower != INFO_FILE {
            files.push(PathBuf::from(name));
        }
    }
    if !files.iter().any(|f| f.as_os_str().eq_ignore_ascii_case(update::APP_EXE)) {
        bail!("в пакете обновления нет {}", update::APP_EXE);
    }
    swap_in(src, dir, &files, true)?;
    let from_setup = InstallInfo::load(dir);
    let mut info = from_setup.clone().unwrap_or_default();
    info.version = version.to_owned();
    let _ = info.save(dir);
    if from_setup.is_some() && system::installed().is_none() {
        // Modly ставил установщик, а записи в «Приложениях» нет (её унесла песочница другого
        // приложения или её удалили руками) — вернуть её, а с ней и ссылки modly://
        match system::write_uninstall_entry(dir, version, dir_size_kb(dir)) {
            Ok(()) => log::info!("restored the Apps & features entry"),
            Err(e) => log::warn!("restore the Apps & features entry: {e:#}"),
        }
        if info.protocol {
            let _ = system::register_protocol(&dir.join(update::APP_EXE));
        }
    } else {
        system::update_uninstall_version(dir, version, dir_size_kb(dir));
    }
    log::info!("updated {} to {version}", dir.display());
    Ok(())
}

// ----------------------------------------------------------------- удаление ---

/// Файлы, которые кладёт установщик (остальное в папке — не наше, не трогаем).
const OUR_FILES: &[&str] = &["modly.exe", "modly-updater.exe", "LICENSE.txt", "README.md", INFO_FILE];

pub fn uninstall(dir: &Path, remove_data: bool, rep: Reporter) -> Result<()> {
    rep(Report::Step(0, "active"));
    rep(Report::Status(t("Убираем ярлыки…", "Removing shortcuts…")));
    let info = InstallInfo::load(dir).unwrap_or_default();
    let (start, desktop) = system::shortcut_paths();
    for lnk in info.shortcuts.iter().chain([&start, &desktop]) {
        if lnk.is_file() {
            let _ = std::fs::remove_file(lnk);
        }
    }
    rep(Report::Progress(0.2));
    rep(Report::Step(0, "done"));

    rep(Report::Step(1, "active"));
    rep(Report::Status(t("Убираем Modly из Windows…", "Unregistering Modly…")));
    system::unregister_protocol_if_in(dir);
    if system::installed().is_none_or(|(d, _)| system::same_dir(&d, dir)) {
        system::remove_uninstall_entry();
    }
    rep(Report::Progress(0.4));
    rep(Report::Step(1, "done"));

    rep(Report::Step(2, "active"));
    rep(Report::Status(t("Удаляем файлы…", "Deleting files…")));
    for name in OUR_FILES {
        let p = dir.join(name);
        // файл может ещё держать только что закрытый Modly — несколько попыток
        let mut tries = 0;
        while p.is_file() {
            match std::fs::remove_file(&p) {
                Ok(()) => break,
                Err(_) if tries < 20 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(e) => return Err(e).with_context(|| format!("{}", p.display())),
            }
        }
    }
    for d in [NEW_DIR, OLD_DIR] {
        let _ = std::fs::remove_dir_all(dir.join(d));
    }
    let _ = std::fs::remove_dir(dir); // только если пустая
    rep(Report::Progress(0.7));
    rep(Report::Step(2, "done"));

    rep(Report::Step(3, "active"));
    // загрузки обновлений — всегда, данные — если попросили
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|_| system::sandbox().is_none()) {
        let _ = std::fs::remove_dir_all(PathBuf::from(local).join("Modly"));
    }
    if remove_data {
        rep(Report::Status(t("Удаляем данные Modly…", "Deleting Modly data…")));
        let data = modly_core::paths::data_dir();
        std::fs::remove_dir_all(&data).with_context(|| format!("{}", data.display()))?;
    }
    rep(Report::Progress(1.0));
    rep(Report::Step(3, "done"));
    log::info!("uninstalled from {}", dir.display());
    Ok(())
}

// --------------------------------------------------------------------- язык ---

static ENGLISH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn t(ru: &str, en: &str) -> String {
    if english() { en.to_owned() } else { ru.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_rolls_back_when_a_file_fails() {
        let root = std::env::temp_dir().join(format!("modly-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (src, dir) = (root.join("src"), root.join("dir"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(src.join("a.txt"), "new a").unwrap();
        std::fs::write(dir.join("a.txt"), "old a").unwrap();
        std::fs::write(dir.join("keep.txt"), "mine").unwrap();
        // b.txt в источнике нет — второй файл упадёт, a.txt должен вернуться
        let err = swap_in(&src, &dir, &[PathBuf::from("a.txt"), PathBuf::from("b.txt")], true);
        assert!(err.is_err());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "old a");
        // нормальная замена
        swap_in(&src, &dir, &[PathBuf::from("a.txt")], true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "new a");
        assert_eq!(std::fs::read_to_string(dir.join("keep.txt")).unwrap(), "mine");
        assert!(!dir.join(OLD_DIR).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Записи в «Приложениях» нет, а Modly ставил установщик — обновление возвращает её.
    #[test]
    fn update_restores_missing_uninstall_entry() {
        let root = std::env::temp_dir().join(format!("modly-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // реестр — в ветке песочницы установщика, не в настоящей записи
        unsafe { std::env::set_var("MODLY_SETUP_SANDBOX", root.join("sb")) };
        let (src, dir) = (root.join("src"), root.join("dir"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(update::APP_EXE), "old").unwrap();
        std::fs::write(src.join(update::APP_EXE), "new").unwrap();
        system::remove_uninstall_entry();

        // Modly не от установщика (нет install.json) — запись не появляется
        apply_update(&src, &dir, "0.9.0-beta").unwrap();
        assert!(system::installed().is_none());

        InstallInfo { version: "0.9.0-beta".into(), shortcuts: Vec::new(), protocol: false }.save(&dir).unwrap();
        apply_update(&src, &dir, "0.9.1-beta").unwrap();
        let (at, version) = system::installed().expect("entry restored");
        assert!(system::same_dir(&at, &dir));
        assert_eq!(version, "0.9.1-beta");

        system::remove_uninstall_entry();
        let _ = std::fs::remove_dir_all(&root);
    }
}
