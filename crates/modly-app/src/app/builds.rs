//! Сборки игр: отпечаток (SHA-256 файла с кодом игры) у каждой копии — чтобы показать на карточке
//! мода, проверял ли автор эту сборку (`game.builds` в `modly.json`), и дать автору дописать её туда.

use super::{App, post};
use modly_core::library::{self, Game};
use modly_core::manifest::{MANIFEST_FILE, ModManifest};
use modly_protocol::Tone;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Размер и время изменения файла: если они те же, отпечаток не пересчитываем.
type Stamp = (u64, Option<SystemTime>);

/// Отпечатки файлов сборок: считаются в фоне (exe Unreal — сотни мегабайт) и запоминаются.
#[derive(Default)]
pub(super) struct Builds {
    known: HashMap<PathBuf, (Stamp, String)>,
    pending: HashSet<PathBuf>,
}

impl Builds {
    pub(super) fn forget(&mut self) {
        self.known.clear();
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then(|| (meta.len(), meta.modified().ok()))
}

/// Что сказать о сборке копии игры на карточке мода.
pub(super) struct BuildNote {
    /// "" (мод сборки не указывает) | ok | unknown | none (сборки не указаны — видно только автору)
    pub state: &'static str,
    pub text: String,
    pub can_add: bool,
}

impl BuildNote {
    fn empty() -> Self {
        BuildNote { state: "", text: String::new(), can_add: false }
    }
}

/// Название сборки по папке игры: `…_DX12_b108` → `b108`, `Game 1.4.2` → `1.4.2`; иначе по отпечатку.
fn guess_build_name(game: &Game, hash: &str) -> String {
    let folder = game.folder_name();
    let token = folder
        .split(|c: char| !(c.is_alphanumeric() || c == '.'))
        .rfind(|t| {
            let digits = t.strip_prefix(['b', 'B']).filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
            let version = t.strip_prefix(['v', 'V']).unwrap_or(t);
            let dotted = version.contains('.')
                && version.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            digits.is_some() || dotted
        });
    match token {
        Some(t) if t.starts_with(['b', 'B']) => t.to_lowercase(),
        Some(t) => t.to_owned(),
        None => format!("build-{}", &hash[..8]),
    }
}

impl App {
    /// Отпечаток сборки, если уже посчитан; иначе считается в фоне, а карточка обновится сама.
    fn build_hash(&mut self, path: &Path) -> Option<String> {
        let st = stamp(path)?;
        if let Some((known, hash)) = self.builds.known.get(path) {
            if *known == st {
                return Some(hash.clone());
            }
        }
        if self.builds.pending.insert(path.to_path_buf()) {
            let path = path.to_path_buf();
            std::thread::spawn(move || {
                let r = library::file_sha256(&path).map_err(|e| e.to_string());
                post(move |a| a.on_build_hashed(path, st, r));
            });
        }
        None
    }

    fn on_build_hashed(&mut self, path: PathBuf, st: Stamp, r: Result<String, String>) {
        self.builds.pending.remove(&path);
        match r {
            Ok(hash) => {
                self.builds.known.insert(path, (st, hash));
                self.refresh_game_page();
            }
            Err(e) => log::warn!("build hash {}: {e}", path.display()),
        }
    }

    /// Пакет разработчика, а не скачанный из каталога: его `modly.json` можно дописывать.
    fn is_dev_package(m: &ModManifest) -> bool {
        !m.dir.starts_with(modly_core::paths::user_mods_dir())
    }

    /// Проверял ли автор мода сборку этой копии игры.
    pub(super) fn build_note(&mut self, game: &Game, m: &ModManifest) -> BuildNote {
        let dev = self.settings.developer_mode && Self::is_dev_package(m);
        if m.game.builds.is_empty() && !dev {
            return BuildNote::empty();
        }
        let Some(hash) = self.build_hash(&library::build_file(game, &m.game)) else {
            return BuildNote::empty();
        };
        if let Some(b) = m.game.build_named(&hash) {
            let text = format!("{}{}{}", self.t("Сборка игры ", "Game build "), b.name, self.t(" — проверена автором мода", " — tested by the mod's author"));
            return BuildNote { state: "ok", text, can_add: false };
        }
        if m.game.builds.is_empty() {
            let text = self.t(
                "В modly.json не указано, на каких сборках игры проверен мод.",
                "modly.json doesn't list the game builds the mod was tested on.",
            );
            return BuildNote { state: "none", text, can_add: dev };
        }
        let names: Vec<&str> = m.game.builds.iter().map(|b| b.name.as_str()).collect();
        let text = format!(
            "{}{}",
            self.t(
                "Эту сборку игры автор мода не проверял — мод может не работать. Проверены: ",
                "The mod's author hasn't tested this game build — the mod may not work. Tested: "
            ),
            names.join(", ")
        );
        BuildNote { state: "unknown", text, can_add: dev }
    }

    /// Автору (режим разработчика): дописать сборку открытой копии игры в `game.builds` мода.
    pub(super) fn add_build(&mut self, mod_id: &str) {
        let (Some(game), Some(m)) = (self.current(), self.find_mod(mod_id)) else { return };
        if !Self::is_dev_package(&m) {
            return;
        }
        let title = m.name.clone();
        match write_build(&game, &m) {
            Ok(name) => {
                let text = format!(
                    "{}{}{}",
                    self.t("Сборка «", "Build “"),
                    name,
                    self.t("» добавлена в modly.json — название можно поменять там же.", "” added to modly.json — you can rename it there.")
                );
                self.toast(Tone::Success, &title, &text);
                self.reload_mods(false);
            }
            Err(e) => {
                log::warn!("add build {mod_id}: {e:#}");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
        }
    }
}

/// Дописать сборку копии в `modly.json` пакета; возвращает её название.
fn write_build(game: &Game, m: &ModManifest) -> anyhow::Result<String> {
    use anyhow::{Context, bail};
    let hash = library::file_sha256(&library::build_file(game, &m.game)).context("файл сборки игры")?;
    if let Some(b) = m.game.build_named(&hash) {
        bail!("эта сборка уже есть в modly.json: «{}»", b.name);
    }
    let path = m.dir.join(MANIFEST_FILE);
    let data = std::fs::read(&path).with_context(|| path.display().to_string())?;
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&data);
    let mut root: Value = serde_json::from_slice(data).with_context(|| path.display().to_string())?;
    let game_obj = root.get_mut("game").and_then(Value::as_object_mut).context("в modly.json нет объекта game")?;
    let mut name = guess_build_name(game, &hash);
    if m.game.builds.iter().any(|b| b.name.eq_ignore_ascii_case(&name)) {
        name = format!("{name}-{}", &hash[..6]);
    }
    let builds = game_obj.entry("builds").or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = builds.as_array_mut() else { bail!("game.builds в modly.json — не список") };
    let mut entry = Map::new();
    entry.insert("name".into(), json!(name));
    entry.insert("sha256".into(), json!(hash));
    list.push(Value::Object(entry));
    let text = serde_json::to_string_pretty(&root)? + "\n";
    std::fs::write(&path, text).with_context(|| path.display().to_string())?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(folder: &str) -> Game {
        Game::new(&PathBuf::from(format!("C:\\Games\\{folder}")), "G.exe", "G", None, false)
    }

    #[test]
    fn build_names_from_folders() {
        let h = "0123456789abcdef".repeat(4);
        assert_eq!(guess_build_name(&game("Family_Time_Ep_3_WINDOWS_DX12_b108"), &h), "b108");
        assert_eq!(guess_build_name(&game("Family_Time_Ep_3_WINDOWS_DX12_b102 — копия"), &h), "b102");
        assert_eq!(guess_build_name(&game("Cool Game v1.4.2"), &h), "v1.4.2");
        assert_eq!(guess_build_name(&game("Cool Game 2.0"), &h), "2.0");
        assert_eq!(guess_build_name(&game("Cool Game 3"), &h), "build-01234567");
        assert_eq!(guess_build_name(&game("Battlefield"), &h), "build-01234567");
    }
}
