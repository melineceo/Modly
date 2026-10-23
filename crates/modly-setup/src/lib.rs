//! Установщик, обновлятор и удаление Modly — одна программа в двух exe:
//!
//! - `modly-setup` (`ModlySetup-<версия>.exe`) — установщик, внутри архив выпуска;
//! - `modly-updater` — лежит рядом с modly.exe: ставит обновления (`--apply`), проверяет их
//!   (без аргументов) и удаляет Modly (`--uninstall`, на неё ведёт «Приложения и возможности»).
//!
//! Окно — на Slint в стиле самого Modly (`ui/setup.slint`), работа с файлами — в [`ops`],
//! с Windows — в [`system`], инструменты мододела — в [`tools`].

slint::include_modules!();

pub mod ops;
pub mod system;
pub mod tools;
mod ui;

pub use ui::{Apply, Mode, VERSION, run};

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Журнал: `%TEMP%\modly-setup.log` (кнопка «Журнал» на странице ошибки).
pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("modly-setup.log")
}

struct FileLog(Mutex<Option<std::fs::File>>);

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        if let Ok(mut f) = self.0.lock() {
            if let Some(f) = f.as_mut() {
                let _ = writeln!(f, "{} {:<5} {}", modly_core::win::local_time_hms(), r.level(), r.args());
            }
        }
    }
    fn flush(&self) {}
}

pub fn init_logging(what: &str) {
    let path = log_path();
    // журнал не растёт бесконечно: больше 1 МБ — начинаем заново
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&path);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    let logger = Box::leak(Box::new(FileLog(Mutex::new(file))));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    log::info!("---- {what} {VERSION}, args: {:?}", std::env::args().skip(1).collect::<Vec<_>>());
}

/// Значение ключа командной строки: `--dir <путь>`.
pub fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a.eq_ignore_ascii_case(name)).and_then(|i| args.get(i + 1)).cloned()
}

pub fn has_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|a| names.iter().any(|n| a.eq_ignore_ascii_case(n)))
}

/// Запущены внутри песочницы «упакованного» приложения — перезапуститься вне её, иначе записи в реестр
/// пропадут в ней. Проверка по факту ([`system::registry_sandboxed`]), а если WMI не ответила — по
/// родителям ([`system::in_packaged_tree`]). `true` — новая копия запущена, этой — выйти.
pub fn escape_package_sandbox(args: &[String]) -> bool {
    if has_flag(args, &["--outside"]) || system::sandbox().is_some() {
        return false;
    }
    let sandboxed = match system::registry_sandboxed() {
        Some(s) => s,
        None => system::in_packaged_tree(),
    };
    if !sandboxed {
        return false;
    }
    let mut again = args.to_vec();
    again.push("--outside".to_owned());
    match system::relaunch_outside(&again) {
        Ok(()) => {
            log::info!("started from a packaged app — relaunched outside it");
            true
        }
        Err(e) => {
            log::warn!("relaunch outside the app package: {e:#}");
            false
        }
    }
}
