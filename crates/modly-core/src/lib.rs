//! Ядро Modly: всё, что не касается интерфейса.
//!
//! - [`manifest`] — пакеты модов (`modly.json`);
//! - [`library`] — библиотека игр и поиск их на диске;
//! - [`install`] — установка и удаление модов в папку игры;
//! - [`process`] — запущенные игры и запуск;
//! - [`hub`] — сервер именованного канала, к которому подключаются моды (протокол — в `modly-protocol`);
//! - [`hotkeys`] — глобальные горячие клавиши для элементов модов;
//! - [`site`] — каталог модов mods.hackerstone.xyz, установка из него и вход (через [`http`]);
//! - [`templates`] — «Сделать мод»: готовый проект мода для любой игры из библиотеки;
//! - [`remote`] — пульт с телефона: веб-сервер панели в домашней сети;
//! - [`update`] — обновления Modly: выпуски на GitHub, подписанный манифест, пакет обновления;
//! - [`settings`], [`paths`] — настройки и папки Modly.

pub mod hotkeys;
pub mod http;
pub mod hub;
pub mod install;
pub mod library;
pub mod maps;
pub mod manifest;
pub mod paths;
pub mod process;
pub mod remote;
pub mod settings;
pub mod site;
pub mod templates;
pub mod update;
#[cfg(windows)]
pub mod win;

pub use modly_protocol as protocol;

/// Записать файл атомарно: сначала во временный рядом, потом переименовать.
pub(crate) fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("modly-tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Короткий стабильный идентификатор из строки (FNV-1a, 64 бита).
pub fn stable_id(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
