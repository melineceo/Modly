//! Карты модов: картинка игры, её края в координатах игры и метки.
//!
//! Карту делает автор мода — он знает, как устроен мир его игры: присылает её по протоколу
//! (`map`, `map_patch` — живые метки игрока и питомцев, действия вроде «Телепорт сюда») или кладёт
//! в пакет (`modly.json` → `maps`) — такая карта видна и без запущенной игры.
//!
//! Картинку мод указывает путём на этом компьютере или `data:`-адресом. Modly читает только
//! локальные диски (сетевые пути `\\сервер\…` Windows открывала бы через сеть, отдавая туда
//! учётные данные), не больше 32 МБ и только PNG, JPEG или WebP — по первым байтам файла.

use crate::manifest::{ModManifest, safe_relative};
use crate::site::{ImageFormat, image_format};
use anyhow::{Context, Result, bail};
use modly_protocol::GameMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Предел файла картинки.
pub const MAX_IMAGE: u64 = 32 * 1024 * 1024;
/// Пределы одной карты: лишнее отбрасывается.
pub const MAX_MAPS: usize = 16;
pub const MAX_MARKERS: usize = 5000;
pub const MAX_CATEGORIES: usize = 64;
pub const MAX_ACTIONS: usize = 8;

/// Карта в `modly.json`: сама карта или путь к JSON-файлу с ней внутри пакета.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageMap {
    File(String),
    Inline(Box<GameMap>),
}

impl PackageMap {
    /// Пути этой записи (файл карты, картинка) — для проверки, что они внутри пакета.
    pub fn paths(&self) -> Vec<&str> {
        match self {
            PackageMap::File(p) => vec![p.as_str()],
            PackageMap::Inline(m) => vec![m.image.as_str()],
        }
    }
}

/// Обрезать карту до разумных пределов (мод мог прислать что угодно).
pub fn limit(map: &mut GameMap) {
    map.categories.truncate(MAX_CATEGORIES);
    map.markers.truncate(MAX_MARKERS);
    map.click_actions.truncate(MAX_ACTIONS);
    for m in &mut map.markers {
        m.actions.truncate(MAX_ACTIONS);
    }
    if map.bounds.is_some_and(|b| !b.is_valid()) {
        map.bounds = None;
    }
}

/// Карты пакета. Битые записи пропускаются (с записью в журнал), остальные работают.
pub fn package_maps(m: &ModManifest) -> Vec<GameMap> {
    let mut out = Vec::new();
    for entry in m.maps.iter().take(MAX_MAPS) {
        let map = match entry {
            PackageMap::Inline(map) => Ok((**map).clone()),
            PackageMap::File(rel) => read_map_file(&m.dir, rel),
        };
        match map {
            Ok(mut map) if !map.id.is_empty() && safe_relative(&map.image) => {
                limit(&mut map);
                out.push(map);
            }
            Ok(map) => log::warn!("{}: карта «{}» — без id или с картинкой вне пакета", m.id, map.id),
            Err(e) => log::warn!("{}: карта {entry:?}: {e:#}", m.id),
        }
    }
    out
}

fn read_map_file(dir: &Path, rel: &str) -> Result<GameMap> {
    if !safe_relative(rel) {
        bail!("путь вне пакета");
    }
    let path = dir.join(rel);
    let len = std::fs::metadata(&path).with_context(|| path.display().to_string())?.len();
    if len > 8 * 1024 * 1024 {
        bail!("файл карты больше 8 МБ");
    }
    let data = std::fs::read(&path)?;
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&data);
    serde_json::from_slice(data).context("карта не читается")
}

/// Картинка карты из пакета (путь внутри пакета).
pub fn package_image(dir: &Path, rel: &str) -> Result<Vec<u8>> {
    if !safe_relative(rel) {
        bail!("картинка карты — вне пакета");
    }
    read_image(&dir.join(rel))
}

/// Картинка карты, которую прислал мод: полный путь на локальном диске или `data:image/…;base64,…`.
pub fn mod_image(src: &str) -> Result<Vec<u8>> {
    if let Some(rest) = src.strip_prefix("data:") {
        let (head, body) = rest.split_once(',').context("неверный data:-адрес")?;
        if !head.ends_with(";base64") {
            bail!("data:-адрес — только base64");
        }
        let data = base64(body).context("неверный base64")?;
        return raster(data);
    }
    if !local_path_ok(src) {
        bail!("картинка карты — только полный путь на диске этого компьютера (C:\\…)");
    }
    read_image(Path::new(src))
}

/// Полный путь на локальном диске: `C:\…` или `C:/…`. Сетевые пути и устройства (`\\…`, `//…`) — нет.
pub fn local_path_ok(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/') && !p.contains('\0')
}

fn read_image(path: &Path) -> Result<Vec<u8>> {
    let len = std::fs::metadata(path).with_context(|| path.display().to_string())?.len();
    if len > MAX_IMAGE {
        bail!("картинка карты больше {} МБ", MAX_IMAGE >> 20);
    }
    raster(std::fs::read(path).with_context(|| path.display().to_string())?)
}

fn raster(data: Vec<u8>) -> Result<Vec<u8>> {
    match image_format(&data) {
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Webp) => Ok(data),
        _ => bail!("картинка карты — не PNG, JPEG или WebP"),
    }
}

/// Base64 (обычный алфавит, `=` в конце можно не ставить; пробелы и переводы строк пропускаются).
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    #[test]
    fn mod_images_only_from_local_disks() {
        for p in [r"\\server\share\map.png", "//server/share/map.png", r"\\?\C:\map.png", r"\\.\pipe\x", "map.png", "C:map.png", "", "C:\\a\0b"] {
            assert!(!local_path_ok(p), "{p:?}");
        }
        assert!(local_path_ok(r"C:\Games\My\map.png") && local_path_ok("d:/maps/x.webp"));

        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("map.png");
        std::fs::write(&png, PNG).unwrap();
        assert!(mod_image(png.to_str().unwrap()).is_ok());
        let txt = dir.path().join("secret.txt");
        std::fs::write(&txt, b"password=123").unwrap();
        assert!(mod_image(txt.to_str().unwrap()).is_err(), "не картинка — не читается");
    }

    #[test]
    fn data_urls_are_decoded() {
        let b64 = "iVBORw0KGgpyZXN0"; // PNG-подпись + "rest"
        assert_eq!(mod_image(&format!("data:image/png;base64,{b64}")).unwrap(), PNG);
        assert!(mod_image("data:image/png,raw").is_err());
        assert!(mod_image("data:image/png;base64,PHN2Zz4=").is_err(), "<svg> — не растр");
        assert_eq!(base64("aGk=").unwrap(), b"hi");
        assert!(base64("a$b").is_none());
    }

    #[test]
    fn package_maps_stay_inside_the_package() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("maps")).unwrap();
        std::fs::write(dir.path().join("maps/world.json"), r#"{"id":"world","image":"maps/world.png","markers":[{"id":"a","x":1,"y":2}]}"#).unwrap();
        std::fs::write(dir.path().join("maps/world.png"), PNG).unwrap();
        let json = r#"{"id":"m","name":"M","game":{"id":"g","name":"G","exe":"g.exe"},"install":{"method":"files","files":[]},
            "maps":["maps/world.json",{"id":"cave","image":"maps/world.png"}]}"#;
        std::fs::write(dir.path().join("modly.json"), json).unwrap();
        let m = ModManifest::load(dir.path()).unwrap();
        let maps = package_maps(&m);
        assert_eq!(maps.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["world", "cave"]);
        assert!(package_image(dir.path(), &maps[0].image).is_ok());
        assert!(package_image(dir.path(), "../outside.png").is_err());

        // путь карты наружу — пакет не загружается вовсе
        std::fs::write(dir.path().join("modly.json"), json.replace("maps/world.json", "../world.json")).unwrap();
        assert!(ModManifest::load(dir.path()).is_err());
    }
}
