//! Игры: библиотека Steam (как в WeMod — все установленные игры с обложками), запуск через Steam
//! и «Сделать мод» для любой игры.
//!
//! Обложки берутся из кэша самого Steam (`appcache/librarycache`) — без сети. Декодируются в
//! фоновом потоке и хранятся уменьшенными, в физических пикселях экрана.

use super::{App, post, with};
use crate::Store;
use crate::thumbs::{self, Kind};
use modly_core::library::{self, Game, SteamApp, SteamArt};
use modly_protocol::Tone;
use slint::{ComponentHandle, Image};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Обложки игр: (id игры, вид) → картинка (`None` — у игры её нет).
#[derive(Default)]
pub struct GameArt {
    images: HashMap<(String, SteamArt), Option<Image>>,
    pending: HashSet<(String, SteamArt)>,
}

/// Игра Steam, готовая к добавлению: приложение, папка с exe и сам exe.
pub type SteamFound = (SteamApp, PathBuf, String);

/// Найти игры Steam (в фоновом потоке поиска игр).
pub fn steam_scan() -> Vec<SteamFound> {
    library::steam_apps()
        .into_iter()
        .filter_map(|app| {
            let (dir, exe) = library::main_exe(&app.dir, &app.name)?;
            Some((app, dir, exe))
        })
        .collect()
}

impl App {
    /// Добавить найденные игры Steam. Скрытые человеком (убрал из библиотеки) не возвращаются.
    /// Возвращает, сколько добавлено.
    pub(super) fn add_steam_games(&mut self, found: Vec<SteamFound>) -> usize {
        let defs: Vec<_> = self.mods.iter().map(|m| m.game.clone()).collect();
        let mut added = 0;
        for (app, dir, exe) in found {
            if self.settings.hidden_steam.contains(&app.appid) {
                continue;
            }
            let id = library::game_id(&dir);
            if let Some(g) = self.library.games.iter_mut().find(|g| g.id == id) {
                g.steam_appid.get_or_insert(app.appid);
                continue;
            }
            let def = defs.iter().find(|d| library::matches_def(&dir, d));
            let name = def.map(|d| d.name.clone()).unwrap_or_else(|| app.name.clone());
            let mut game = Game::new(&dir, &exe, &name, def.map(|d| d.id.clone()), false);
            game.steam_appid = Some(app.appid);
            if self.library.add(game) {
                added += 1;
            }
        }
        added
    }

    /// Запустить игру Steam через Steam (оверлей, облако, лаунчеры игр — всё как обычно).
    pub(super) fn launch_steam(&mut self, game: &Game) -> bool {
        let Some(appid) = game.steam_appid else { return false };
        modly_core::win::shell_open(&format!("steam://rungameid/{appid}"));
        let msg = format!("{} {}", game.name, self.t("запускается через Steam…", "is starting via Steam…"));
        self.toast(Tone::Info, "", &msg);
        true
    }

    /// Убрали игру Steam из библиотеки — запомнить, чтобы поиск игр не вернул её.
    pub(super) fn hide_steam(&mut self, game: &Game) {
        if let Some(appid) = game.steam_appid {
            if !self.settings.hidden_steam.contains(&appid) {
                self.settings.hidden_steam.push(appid);
                self.settings.save();
            }
        }
    }

    /// Обложка игры из кэша Steam (загружается в фоне; пока её нет — `None`).
    pub(super) fn game_art(&mut self, game: &Game, art: SteamArt) -> Option<Image> {
        let appid = game.steam_appid?;
        let key = (game.id.clone(), art);
        if let Some(img) = self.game_art.images.get(&key) {
            return img.clone();
        }
        if self.game_art.pending.insert(key.clone()) {
            let scale = self.ui().map(|ui| ui.window().scale_factor()).unwrap_or(1.0);
            let kind = match art {
                SteamArt::Header => Kind::GameHeader,
                SteamArt::Hero => Kind::GameHero,
            };
            std::thread::spawn(move || {
                let pixels = library::steam_art(appid, art)
                    .and_then(|p| std::fs::read(p).ok())
                    .and_then(|data| thumbs::decode(&data, kind, kind.size(scale)).map_err(|e| log::info!("steam art {appid}: {e:#}")).ok());
                post(move |a| {
                    a.game_art.pending.remove(&key);
                    a.game_art.images.insert(key, pixels.map(|p| p.to_image()));
                    a.refresh_games();
                    a.refresh_game_page();
                });
            });
        }
        None
    }

    /// «Сделать мод»: проект мода для этой игры в «Документы\Modly Mods», сразу видимый в Modly.
    pub(super) fn create_mod(&mut self, id: &str) {
        let Some(game) = self.library.get(id).cloned() else { return };
        let root = modly_core::templates::default_root();
        match modly_core::templates::create(&game, &root) {
            Ok(created) => {
                // папка с проектами — в папки модов, режим разработчика — чтобы была кнопка «Собрать»
                if !self.settings.mod_dirs.contains(&root) {
                    self.settings.mod_dirs.push(root.clone());
                }
                self.set_developer_mode(true);
                self.reload_mods(false);
                let title = self.t("Проект мода создан", "Mod project created");
                let text = if created.csharp {
                    self.t(
                        "C#-шаблон для Unity открыт в Проводнике. Нажмите «Собрать», потом «Установить» — и мод появится в игре. Подробности — в README.md.",
                        "A C# Unity template is open in Explorer. Press Build, then Install — and the mod shows up in the game. See README.md.",
                    )
                } else {
                    self.t(
                        "C++-шаблон открыт в Проводнике. Как собрать и загрузить мод в эту игру — в README.md.",
                        "A C++ template is open in Explorer. How to build it and load it into this game — see README.md.",
                    )
                };
                self.toast(Tone::Success, &title, &text);
                #[cfg(windows)]
                modly_core::win::shell_open(&created.dir.display().to_string());
            }
            Err(e) => {
                let title = self.t("Не удалось создать проект", "Couldn't create the project");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
        }
    }
}

/// Кнопки игр (вызывается из `wire_callbacks`).
pub(super) fn wire(s: &Store) {
    s.on_create_mod(|id| with(move |a| a.create_mod(&id)));
}
