//! Состояние приложения и всё, что делают кнопки интерфейса.
//!
//! `App` живёт на потоке интерфейса (thread_local). Фоновые потоки и хаб присылают результаты через
//! `slint::invoke_from_event_loop` → [`with`].

use crate::live::{self, Session};
use crate::{
    AppWindow, GameCard, Lang, LogLine, ModCard, SessionTab, Store, Toast, LiveControl, LivePage, LiveSection,
};
use modly_core::hotkeys::{Hotkey, HotkeyEvent, Hotkeys};
use modly_core::hub::{Hub, HubEvent, SessionId};
use modly_core::install::{self, ModState};
use modly_core::library::{self, Game, Library};
use modly_core::manifest::{self, ModManifest};
use modly_core::process::{self, Running};
use modly_core::settings::Settings;
use modly_protocol::{ClientMsg, Control, ControlKind, Tone};
use serde_json::Value;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod builds;
mod catalog;
mod games;
mod maps;
mod overlay;
mod remote;
mod update;
pub use catalog::{forward_to_running, url_from_args};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Версия для людей: `0.1.0-beta` → «0.1.0 Beta».
pub fn version_label() -> String {
    modly_core::update::label(VERSION)
}

const MAX_LOG: usize = 400;
/// Версия поиска игр: 2 — игры находятся и без пакетов модов, по признакам движка.
const SCAN_VERSION: u32 = 2;
/// Сколько ждать второго нажатия клавиши у кнопки с подтверждением (как у кнопки в панели).
const CONFIRM_WINDOW: Duration = Duration::from_secs(3);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Выполнить действие с состоянием приложения. Если оно сейчас занято (вызов изнутри другого
/// действия), действие откладывается на следующий проход цикла событий.
pub fn with(f: impl FnOnce(&mut App) + 'static) {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => {
            if let Some(app) = guard.as_mut() {
                f(app);
                app.sync_overlay();
            }
        }
        Err(_) => slint::Timer::single_shot(Duration::ZERO, move || with(f)),
    });
}

/// Из любого потока: выполнить действие на потоке интерфейса.
pub fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with(f));
}

pub struct App {
    ui: slint::Weak<AppWindow>,
    hub: Hub,
    settings: Settings,
    library: Library,
    watched: Arc<Mutex<Vec<Game>>>,
    mods: Vec<ModManifest>,
    states: HashMap<(String, String), ModState>,
    busy: HashSet<String>,
    running: Running,
    sessions: BTreeMap<SessionId, Session>,
    current_game: Option<String>,
    live_session: Option<SessionId>,
    live_page: usize,
    rows: HashMap<String, (usize, usize)>,
    section_models: Vec<Rc<VecModel<LiveControl>>>,
    game_icons: HashMap<String, Option<(Image, slint::Color)>>,
    drag_sent: HashMap<String, Instant>,
    search: String,
    next_toast: i32,
    /// `--game <имя>`: открыть игру, как только она появится в библиотеке.
    pending_game: Option<String>,
    /// Глобальные горячие клавиши (`None` — поток не запустился).
    hotkeys: Option<Hotkeys>,
    /// Зарегистрированные сочетания → какие элементы каких сессий они нажимают.
    hotkey_map: HashMap<Hotkey, Vec<(SessionId, String)>>,
    /// Сочетания, которые заняла другая программа.
    hotkey_failed: HashSet<Hotkey>,
    /// Элемент, которому сейчас назначают клавишу.
    capture: Option<(SessionId, String)>,
    /// Кнопки с подтверждением, «взведённые» первым нажатием клавиши.
    armed: HashMap<(SessionId, String), Instant>,
    /// Последнее уведомление о нажатии клавиши — следующее заменяет его, а не копится стопкой.
    last_hotkey_toast: Option<i32>,
    /// Каталог сайта модов, вход и ссылки modly:// (см. `app/catalog.rs`).
    cat: catalog::CatalogState,
    /// Обложки игр из кэша Steam (см. `app/games.rs`).
    game_art: games::GameArt,
    /// Оверлей в игре (см. `app/overlay.rs`).
    overlay: overlay::Overlay,
    /// Отпечатки сборок игр (см. `app/builds.rs`).
    builds: builds::Builds,
    /// Карты игр от модов (см. `app/maps.rs`).
    maps: maps::MapsState,
    /// Пульт с телефона (см. `app/remote.rs`).
    remote: remote::Remote,
    /// Обновления Modly (см. `app/update.rs`).
    upd: update::UpdateState,
    /// Потоки tokio: сервер пульта запускается и после старта.
    rt: tokio::runtime::Handle,

    m_games: Rc<VecModel<GameCard>>,
    m_library: Rc<VecModel<GameCard>>,
    m_game_mods: Rc<VecModel<ModCard>>,
    m_all_mods: Rc<VecModel<ModCard>>,
    m_mod_errors: Rc<VecModel<SharedString>>,
    m_live_tabs: Rc<VecModel<SessionTab>>,
    m_live_pages: Rc<VecModel<LivePage>>,
    m_live_sections: Rc<VecModel<LiveSection>>,
    m_sessions: Rc<VecModel<SessionTab>>,
    m_log: Rc<VecModel<LogLine>>,
    m_toasts: Rc<VecModel<Toast>>,
    m_mod_dirs: Rc<VecModel<SharedString>>,
}

// =============================================================================================
// Запуск

pub fn init(ui: &AppWindow, rt: tokio::runtime::Handle) {
    let settings = Settings::load();
    let lang = settings.effective_language();
    let hub = Hub::start(&rt, VERSION, lang, |e| post(move |a| a.on_hub(e)));
    let hotkeys = Hotkeys::start(|e| post(move |a| a.on_hotkey(e)))
        .map_err(|e| log::warn!("hotkeys: {e}"))
        .ok();

    let mut app = App {
        ui: ui.as_weak(),
        hub,
        settings,
        library: Library::load(),
        watched: Arc::new(Mutex::new(Vec::new())),
        mods: Vec::new(),
        states: HashMap::new(),
        busy: HashSet::new(),
        running: Running::default(),
        sessions: BTreeMap::new(),
        current_game: None,
        live_session: None,
        live_page: 0,
        rows: HashMap::new(),
        section_models: Vec::new(),
        game_icons: HashMap::new(),
        drag_sent: HashMap::new(),
        search: String::new(),
        next_toast: 1,
        pending_game: None,
        hotkeys,
        hotkey_map: HashMap::new(),
        hotkey_failed: HashSet::new(),
        capture: None,
        armed: HashMap::new(),
        last_hotkey_toast: None,
        cat: catalog::CatalogState::default(),
        game_art: games::GameArt::default(),
        overlay: overlay::Overlay::default(),
        remote: remote::Remote::default(),
        upd: update::UpdateState::default(),
        maps: maps::MapsState::default(),
        builds: builds::Builds::default(),
        rt: rt.clone(),
        m_games: Rc::new(VecModel::default()),
        m_library: Rc::new(VecModel::default()),
        m_game_mods: Rc::new(VecModel::default()),
        m_all_mods: Rc::new(VecModel::default()),
        m_mod_errors: Rc::new(VecModel::default()),
        m_live_tabs: Rc::new(VecModel::default()),
        m_live_pages: Rc::new(VecModel::default()),
        m_live_sections: Rc::new(VecModel::default()),
        m_sessions: Rc::new(VecModel::default()),
        m_log: Rc::new(VecModel::default()),
        m_toasts: Rc::new(VecModel::default()),
        m_mod_dirs: Rc::new(VecModel::default()),
    };

    let store = ui.global::<Store>();
    store.set_games(ModelRc::from(app.m_games.clone()));
    store.set_library(ModelRc::from(app.m_library.clone()));
    store.set_game_mods(ModelRc::from(app.m_game_mods.clone()));
    store.set_all_mods(ModelRc::from(app.m_all_mods.clone()));
    store.set_mod_errors(ModelRc::from(app.m_mod_errors.clone()));
    store.set_live_tabs(ModelRc::from(app.m_live_tabs.clone()));
    store.set_live_pages(ModelRc::from(app.m_live_pages.clone()));
    store.set_live_sections(ModelRc::from(app.m_live_sections.clone()));
    store.set_sessions(ModelRc::from(app.m_sessions.clone()));
    store.set_log(ModelRc::from(app.m_log.clone()));
    store.set_toasts(ModelRc::from(app.m_toasts.clone()));
    store.set_mod_dirs(ModelRc::from(app.m_mod_dirs.clone()));
    maps::attach(&store, &app.maps);
    store.set_version(version_label().into());
    store.set_pipe_name(modly_protocol::PIPE_NAME.into());
    store.set_data_dir(modly_core::paths::data_dir().display().to_string().into());
    store.set_hub_text(app.t("Запуск моста…", "Starting bridge…").into());

    app.apply_settings_to_ui();
    app.reload_mods(false);
    app.catalog_init();
    app.update_init();
    catalog::start_open_listener(&rt);
    app.hub.set_trace(app.settings.developer_mode);

    // первый запуск или поиск игр стал умнее — поискать ещё раз
    let first_scan = !app.settings.first_scan_done || app.settings.scan_version < SCAN_VERSION;
    let last = app.settings.last_game.clone();
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    wire_callbacks(&ui.global::<Store>());
    start_process_watch();
    // клавиша оверлея работает и без подключённых модов
    with(|a| a.sync_hotkeys());
    with(|a| a.sync_remote());

    if first_scan {
        with(|a| a.scan_games(true));
    }
    if !last.is_empty() {
        with(move |a| {
            if a.library.get(&last).is_some() {
                a.current_game = Some(last);
                a.refresh_game_page();
            }
        });
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    with(move |a| a.apply_args(&args));
}

/// Подвязать кнопки к окну: и к главному, и к оверлею (у каждого окна свой `Store`).
fn wire_callbacks(s: &Store) {
    catalog::wire(s);
    games::wire(s);
    remote::wire(s);
    maps::wire(s);
    update::wire(s);
    s.on_navigate(|p| with(move |a| a.navigate(&p)));
    s.on_search_changed(|t| with(move |a| a.set_search(t.to_string())));
    s.on_open_game(|id| with(move |a| a.open_game(&id)));
    s.on_launch_game(|id| with(move |a| a.launch_game(&id)));
    s.on_open_game_folder(|id| with(move |a| a.open_game_folder(&id)));
    s.on_remove_game(|id| with(move |a| a.remove_game(&id)));
    s.on_add_game(|| with(|a| a.add_game()));
    s.on_scan_games(|| with(|a| a.scan_games(false)));
    s.on_install_mod(|id| with(move |a| a.install_mod(&id)));
    s.on_uninstall_mod(|id| with(move |a| a.uninstall_mod(&id)));
    s.on_install_everywhere(|id| with(move |a| a.install_everywhere(&id)));
    s.on_build_mod(|id| with(move |a| a.build_mod(&id)));
    s.on_add_build(|id| with(move |a| a.add_build(&id)));
    s.on_open_mod_folder(|id| with(move |a| a.open_mod_folder(&id)));
    s.on_open_link(|l| with(move |a| a.open_link(&l)));
    s.on_live_select_tab(|i| with(move |a| a.live_select_tab(i)));
    s.on_live_select_page(|i| with(move |a| a.live_select_page(i)));
    s.on_live_set_bool(|id, v| with(move |a| a.live_set(&id, Value::Bool(v), true)));
    s.on_live_drag_number(|id, v| with(move |a| a.live_drag(&id, v)));
    s.on_live_set_number(|id, v| with(move |a| a.live_number(&id, v)));
    s.on_live_set_text(|id, t| with(move |a| a.live_set(&id, Value::String(t.to_string()), true)));
    s.on_live_pick(|id, v| with(move |a| a.live_pick(&id, &v)));
    s.on_live_invoke(|id| with(move |a| a.live_invoke(&id)));
    s.on_hotkey_capture_start(|id| with(move |a| a.hotkey_capture_start(&id)));
    s.on_hotkey_capture_cancel(|id| with(move |a| a.hotkey_capture_cancel(&id)));
    s.on_hotkey_clear(|id| with(move |a| a.hotkey_assign(&id, None)));
    s.on_hotkey_pressed(|id, text, ctrl, alt, shift, win| {
        // код клавиши узнаём сразу, пока Windows помнит её нажатой: иначе Num1 не отличить от 1
        let key = modly_core::hotkeys::captured(&text, ctrl, alt, shift, win);
        with(move |a| match key {
            Some(key) => a.hotkey_assign(&id, Some(key)),
            None => {
                let text = a.t("Эту клавишу назначить нельзя — выберите другую.", "This key can't be used — pick another one.");
                a.toast(Tone::Info, "", &text);
            }
        })
    });
    s.on_set_language(|l| with(move |a| a.set_language(&l)));
    s.on_set_developer_mode(|v| with(move |a| a.set_developer_mode(v)));
    s.on_set_gpu_rendering(|v| with(move |a| a.set_gpu_rendering(v)));
    s.on_set_wemod(|v| with(move |a| a.set_wemod(v)));
    s.on_set_overlay_hotkey(|k| with(move |a| a.set_overlay_hotkey(&k)));
    s.on_open_wemod(|| modly_core::win::shell_open("wemod://"));
    s.on_add_mod_dir(|| with(|a| a.add_mod_dir()));
    s.on_remove_mod_dir(|i| with(move |a| a.remove_mod_dir(i)));
    s.on_open_data_dir(|| modly_core::win::shell_open(&modly_core::paths::data_dir().display().to_string()));
    s.on_reload_mods(|| with(|a| a.reload_mods(true)));
    s.on_dismiss_toast(|id| with(move |a| a.dismiss_toast(id)));
    s.on_clear_log(|| with(|a| a.m_log.set_vec(Vec::new())));
    s.on_copy_text(|t| {
        let ok = modly_core::win::set_clipboard(&t);
        with(move |a| {
            if ok {
                let msg = a.t("Скопировано в буфер обмена", "Copied to clipboard");
                a.toast(Tone::Success, "", &msg);
            }
        })
    });
}

/// Следить за запущенными играми: раз в 1,5 секунды, в фоне.
fn start_process_watch() {
    let watched = APP.with(|c| c.borrow().as_ref().map(|a| a.watched.clone()));
    let Some(watched) = watched else { return };
    std::thread::Builder::new()
        .name("modly-process-watch".into())
        .spawn(move || loop {
            let games = watched.lock().unwrap().clone();
            let running = Running::snapshot(&games);
            post(move |a| a.on_running(running));
            std::thread::sleep(Duration::from_millis(1500));
        })
        .expect("spawn process watch");
}

/// Тёмный заголовок окна в цвет боковой панели.
pub fn style_window(ui: &AppWindow) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = ui.window().window_handle();
        if let Ok(h) = handle.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                modly_core::win::style_window(w.hwnd.get(), (0x0d, 0x10, 0x17), (0x1f, 0x25, 0x34), (0xee, 0xf0, 0xf7));
            }
        }
    }
}

// =============================================================================================

impl App {
    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn lang(&self) -> &'static str {
        self.settings.effective_language()
    }

    fn t(&self, ru: &str, en: &str) -> String {
        if self.lang() == "ru" { ru.to_owned() } else { en.to_owned() }
    }

    fn apply_settings_to_ui(&mut self) {
        let Some(ui) = self.ui() else { return };
        ui.global::<Lang>().set_english(self.lang() != "ru");
        let store = ui.global::<Store>();
        store.set_lang_setting(self.settings.language.as_str().into());
        store.set_developer_mode(self.settings.developer_mode);
        store.set_gpu_rendering(self.settings.gpu_rendering);
        store.set_wemod(self.settings.wemod);
        store.set_remote(self.settings.remote);
        store.set_overlay_hotkey(match self.settings.overlay_hotkey.as_str() {
            "" => overlay::DEFAULT_KEY.into(),
            k => k.into(),
        });
        #[cfg(windows)]
        store.set_wemod_available(modly_core::win::wemod_installed());
        let mut dirs: Vec<SharedString> = Vec::new();
        for d in self.mod_dirs() {
            dirs.push(d.display().to_string().into());
        }
        self.m_mod_dirs.set_vec(dirs);
    }

    /// Папки модов: по умолчанию + из настроек.
    fn mod_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = modly_core::paths::default_mod_dirs();
        for d in &self.settings.mod_dirs {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        dirs
    }

    // -----------------------------------------------------------------------------------------
    // Моды и библиотека

    fn reload_mods(&mut self, notify: bool) {
        let (mods, errors) = manifest::discover(&self.mod_dirs());
        self.mods = mods;
        self.maps.forget_packages();
        self.builds.forget();
        self.relink_games();
        self.library.save();
        self.states.clear();
        self.m_mod_errors.set_vec(errors.iter().map(|e| SharedString::from(e.as_str())).collect::<Vec<_>>());
        for e in &errors {
            log::warn!("mod package: {e}");
        }
        self.refresh_games();
        self.refresh_game_page();
        self.refresh_all_mods();
        if notify {
            let msg = format!("{}{}", self.t("Модов найдено: ", "Mods found: "), self.mods.len());
            self.toast(Tone::Info, "", &msg);
        }
    }

    fn mod_state(&mut self, game: &Game, m: &ModManifest) -> ModState {
        let key = (game.id.clone(), m.id.clone());
        if let Some(s) = self.states.get(&key) {
            return s.clone();
        }
        let s = install::status(game, m);
        self.states.insert(key, s.clone());
        s
    }

    /// Моды для копии игры — по её папке (см. [`Game::has_def`]), как бы автор ни назвал игру.
    fn mods_for(&self, game: &Game) -> Vec<ModManifest> {
        self.mods.iter().filter(|m| game.has_def(&m.game.id)).cloned().collect()
    }

    /// Привязать копии игр к описаниям из пакетов модов (по папке и номеру в Steam).
    fn relink_games(&mut self) {
        let defs: Vec<_> = self.mods.iter().map(|m| m.game.clone()).collect();
        self.library.link_defs(&defs);
    }

    fn game_icon(&mut self, game: &Game) -> Option<(Image, slint::Color)> {
        if let Some(cached) = self.game_icons.get(&game.id) {
            return cached.clone();
        }
        let icon = crate::icons::exe_icon(&game.exe_path()).map(|(img, accent)| {
            let accent = self
                .mods
                .iter()
                .find(|m| game.has_def(&m.game.id))
                .and_then(|m| m.game.accent.as_deref().and_then(crate::icons::parse_hex))
                .or(accent)
                .unwrap_or_else(|| crate::icons::fallback_accent(&game.name));
            (img, accent)
        });
        self.game_icons.insert(game.id.clone(), icon.clone());
        icon
    }

    fn game_card(&mut self, game: &Game) -> GameCard {
        let icon = self.game_icon(game);
        let cover = self.game_art(game, library::SteamArt::Header);
        let mods = self.mods_for(game);
        let installed = mods.iter().filter(|m| matches!(self.mod_state(game, m), ModState::Installed { .. })).count();
        let live = self.sessions.values().any(|s| s.game_id.as_deref() == Some(game.id.as_str()));
        let mut folder = game.folder_name();
        if folder.is_empty() {
            folder = game.dir.display().to_string();
        }
        GameCard {
            id: game.id.as_str().into(),
            name: game.name.as_str().into(),
            folder: folder.into(),
            engine: game.engine().label().into(),
            has_icon: icon.is_some(),
            accent: icon.as_ref().map(|(_, c)| *c).unwrap_or_else(|| crate::icons::fallback_accent(&game.name)),
            icon: icon.map(|(i, _)| i).unwrap_or_default(),
            running: self.running.is_running(game),
            live,
            mods_installed: installed as i32,
            mods_total: mods.len() as i32,
            missing: !game.exists(),
            has_cover: cover.is_some(),
            cover: cover.unwrap_or_default(),
            has_hero: false,
            hero: Image::default(),
            steam: game.steam_appid.is_some(),
        }
    }

    /// Подписи копий одной игры: только отличающаяся часть имени папки
    /// (`Family_Time_Ep_3_WINDOWS_DX11_b102` → `DX11_b102`).
    fn copy_labels(games: &[Game]) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        for g in games {
            let folder = g.folder_name();
            let siblings: Vec<String> = games.iter().filter(|o| o.name == g.name).map(|o| o.folder_name()).collect();
            if siblings.len() < 2 {
                labels.insert(g.id.clone(), folder);
                continue;
            }
            let first: Vec<char> = siblings[0].chars().collect();
            let mut common = first.len();
            for s in &siblings[1..] {
                common = common.min(s.chars().zip(&first).take_while(|(a, b)| a == *b).count());
            }
            // не резать посреди слова: откатиться к разделителю
            let chars: Vec<char> = folder.chars().collect();
            while common > 0 && !matches!(chars.get(common - 1), Some('_' | ' ' | '-' | '.')) {
                common -= 1;
            }
            let rest: String = chars[common.min(chars.len())..].iter().collect();
            let rest = rest.trim_matches(|c| c == '_' || c == ' ' || c == '-').to_owned();
            labels.insert(g.id.clone(), if rest.is_empty() { folder } else { rest });
        }
        labels
    }

    fn refresh_games(&mut self) {
        *self.watched.lock().unwrap() = self.library.games.clone();
        let games = self.library.games.clone();
        let labels = Self::copy_labels(&games);
        let cards: Vec<GameCard> = games
            .iter()
            .map(|g| {
                let mut card = self.game_card(g);
                if let Some(l) = labels.get(&g.id) {
                    card.folder = l.as_str().into();
                }
                card
            })
            .collect();
        let query = self.search.to_lowercase();
        let filtered: Vec<GameCard> = cards
            .iter()
            .filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query) || c.folder.to_lowercase().contains(&query))
            .cloned()
            .collect();
        self.m_games.set_vec(cards);
        self.m_library.set_vec(filtered);
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_live_count(self.sessions.len() as i32);
        }
        if let Some(want) = self.pending_game.clone() {
            let want = want.to_lowercase();
            let found = self.library.games.iter().find(|g| {
                g.id == want || g.name.to_lowercase().contains(&want) || g.folder_name().to_lowercase().contains(&want)
            });
            if let Some(id) = found.map(|g| g.id.clone()) {
                self.pending_game = None;
                slint::Timer::single_shot(Duration::ZERO, move || with(move |a| a.open_game(&id)));
            }
        }
    }

    /// Флаги запуска: `--page home|mods|dev|settings`, `--game <имя или часть папки>`.
    pub fn apply_args(&mut self, args: &[String]) {
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--page" => {
                    if let Some(p) = it.next() {
                        self.navigate(p);
                    }
                }
                "--game" => {
                    self.pending_game = it.next().cloned();
                    self.refresh_games();
                }
                link if link.to_ascii_lowercase().starts_with("modly://") => {
                    let link = link.to_owned();
                    self.open_url(&link);
                }
                _ => {}
            }
        }
    }

    fn set_search(&mut self, text: String) {
        self.search = text;
        self.refresh_games();
    }

    fn state_text(&self, state: &str) -> String {
        match state {
            "installed" => self.t("Установлен", "Installed"),
            "outdated" => self.t("Есть обновление", "Update available"),
            "broken" => self.t("Повреждён", "Broken"),
            "unbuilt" => self.t("Не собран", "Not built"),
            _ => self.t("Не установлен", "Not installed"),
        }
    }

    fn mod_card(&mut self, m: &ModManifest, game: Option<&Game>) -> ModCard {
        let lang = self.lang();
        let copies: Vec<Game> = self.library.games.iter().filter(|g| g.has_def(&m.game.id)).cloned().collect();
        let unbuilt = !m.missing_files().is_empty();
        let (state, live) = match game {
            Some(g) => {
                let st = match self.mod_state(g, m) {
                    ModState::NotInstalled if unbuilt => "unbuilt",
                    ModState::NotInstalled => "none",
                    ModState::Installed { current: true } => "installed",
                    ModState::Installed { current: false } if unbuilt => "installed",
                    ModState::Installed { current: false } => "outdated",
                    ModState::Broken(_) => "broken",
                };
                let live = self.sessions.values().any(|s| s.hello.mod_info.id == m.id && s.game_id.as_deref() == Some(g.id.as_str()));
                (st, live)
            }
            None => {
                let live = self.sessions.values().any(|s| s.hello.mod_info.id == m.id);
                (if unbuilt { "unbuilt" } else { "none" }, live)
            }
        };
        let installed_count = copies.iter().filter(|g| matches!(self.mod_state(g, m), ModState::Installed { .. })).count();
        let state_text = if game.is_none() && !unbuilt {
            if copies.is_empty() {
                self.t("Игра не найдена", "Game not found")
            } else {
                format!("{}{} / {}", self.t("Установлен: ", "Installed: "), installed_count, copies.len())
            }
        } else {
            self.state_text(state)
        };
        let build = match game {
            Some(g) => self.build_note(g, m),
            None => builds::BuildNote { state: "", text: String::new(), can_add: false },
        };
        ModCard {
            id: m.id.as_str().into(),
            name: m.name.as_str().into(),
            version: m.version.as_str().into(),
            author: m.author.as_str().into(),
            description: m.description.as_ref().map(|d| d.resolve(lang)).unwrap_or("").into(),
            game_name: m.game.name.as_str().into(),
            state: if game.is_none() && !unbuilt && installed_count > 0 { "installed".into() } else { state.into() },
            state_text: state_text.into(),
            live,
            can_build: m.dev.as_ref().is_some_and(|d| d.build.is_some()),
            busy: self.busy.contains(&m.id),
            installed_count: installed_count as i32,
            copies: copies.len() as i32,
            build_state: build.state.into(),
            build_text: build.text.into(),
            can_add_build: build.can_add,
        }
    }

    fn refresh_all_mods(&mut self) {
        let mods = self.mods.clone();
        let cards: Vec<ModCard> = mods.iter().map(|m| self.mod_card(m, None)).collect();
        self.m_all_mods.set_vec(cards);
    }

    fn current(&self) -> Option<Game> {
        self.current_game.as_ref().and_then(|id| self.library.get(id)).cloned()
    }

    fn refresh_game_page(&mut self) {
        let Some(game) = self.current() else { return };
        let mut card = self.game_card(&game);
        card.folder = game.dir.display().to_string().into();
        if let Some(hero) = self.game_art(&game, library::SteamArt::Hero) {
            card.hero = hero;
            card.has_hero = true;
        }
        let mods = self.mods_for(&game);
        let cards: Vec<ModCard> = mods.iter().map(|m| self.mod_card(m, Some(&game))).collect();
        self.m_game_mods.set_vec(cards);
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_game(card);
        }
        self.refresh_game_maps(&game);
        self.catalog_for_game();
        self.rebuild_live();
    }

    // -----------------------------------------------------------------------------------------
    // Навигация и игры

    fn navigate(&mut self, page: &str) {
        if page != "game" {
            self.end_capture();
        }
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_page(page.into());
        }
        match page {
            "mods" => self.refresh_all_mods(),
            "home" => self.refresh_games(),
            "catalog" => self.catalog_show(),
            "settings" => self.refresh_remote_url(false),
            _ => {}
        }
    }

    fn open_game(&mut self, id: &str) {
        if self.library.get(id).is_none() {
            return;
        }
        if self.current_game.as_deref() != Some(id) {
            self.current_game = Some(id.to_owned());
            self.live_session = None;
            self.live_page = 0;
        }
        self.states.retain(|(g, _), _| g != id); // свежее состояние модов при открытии
        self.refresh_game_page();
        self.navigate("game");
        self.settings.last_game = id.to_owned();
        self.settings.save();
    }

    fn launch_game(&mut self, id: &str) {
        let Some(game) = self.library.get(id).cloned() else { return };
        if self.launch_steam(&game) {
            return;
        }
        match process::launch(&game, &[]) {
            Ok(pid) => {
                log::info!("launched {} (pid {pid})", game.exe_path().display());
                let msg = format!("{} {}", game.name, self.t("запускается…", "is starting…"));
                self.toast(Tone::Info, "", &msg);
            }
            Err(e) => {
                let title = self.t("Не удалось запустить игру", "Couldn't start the game");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
        }
    }

    fn open_game_folder(&mut self, id: &str) {
        if let Some(g) = self.library.get(id) {
            modly_core::win::shell_open(&g.dir.display().to_string());
        }
    }

    fn remove_game(&mut self, id: &str) {
        if let Some(g) = self.library.get(id).cloned() {
            self.hide_steam(&g);
            // игру не из Steam поиск по движку тоже не вернёт
            if g.steam_appid.is_none() && !self.settings.hidden_games.iter().any(|h| h == id) {
                self.settings.hidden_games.push(id.to_owned());
                self.settings.save();
            }
        }
        self.library.remove(id);
        self.library.save();
        if self.current_game.as_deref() == Some(id) {
            self.current_game = None;
            self.navigate("home");
        }
        self.refresh_games();
    }

    fn add_game(&mut self) {
        let title = self.t("Выберите папку игры", "Choose the game folder");
        let Some(dir) = rfd::FileDialog::new().set_title(&title).pick_folder() else { return };
        match self.detect_game(&dir) {
            Some(game) => {
                let id = game.id.clone();
                let name = game.name.clone();
                // добавили сами — значит, больше не скрыта
                if self.settings.hidden_games.iter().any(|h| *h == id) {
                    self.settings.hidden_games.retain(|h| *h != id);
                    self.settings.save();
                }
                if self.library.add(game) {
                    self.relink_games();
                    self.library.save();
                    let msg = format!("{} {}", name, self.t("в библиотеке", "added to the library"));
                    self.toast(Tone::Success, "", &msg);
                }
                self.refresh_games();
                self.open_game(&id);
            }
            None => {
                let title = self.t("Игра не найдена", "No game found");
                let text = self.t("В этой папке нет exe игры.", "There is no game exe in this folder.");
                self.toast(Tone::Warning, &title, &text);
            }
        }
    }

    /// Определить игру в папке: сначала по пакетам модов, потом по Unity-папке данных, потом самый большой exe.
    fn detect_game(&self, dir: &Path) -> Option<Game> {
        for m in &self.mods {
            if library::matches_def(dir, &m.game) {
                return Some(Game::new(dir, &m.game.exe, &m.game.name, Some(m.game.id.clone()), true));
            }
        }
        const HELPERS: &[&str] = &["unitycrashhandler64.exe", "unitycrashhandler32.exe", "crashreportclient.exe", "uninstall.exe", "unins000.exe"];
        let mut exes: Vec<(String, u64)> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let lower = name.to_lowercase();
                (lower.ends_with(".exe") && !HELPERS.contains(&lower.as_str())).then(|| (name, e.metadata().map(|m| m.len()).unwrap_or(0)))
            })
            .collect();
        exes.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
        let exe = exes
            .iter()
            .find(|(n, _)| dir.join(manifest::expand("{data}", n)).is_dir())
            .or(exes.first())
            .map(|(n, _)| n.clone())?;
        let name = library::guess_name(dir, &exe);
        Some(Game::new(dir, &exe, &name, None, true))
    }

    fn scan_games(&mut self, first: bool) {
        let mut defs: Vec<_> = Vec::new();
        for m in &self.mods {
            if !defs.iter().any(|d: &manifest::GameDef| d.id == m.game.id) {
                defs.push(m.game.clone());
            }
        }
        let mut roots = library::default_roots();
        for r in &self.settings.scan_roots {
            roots.push((r.clone(), 3));
        }
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_scanning(true);
        }
        std::thread::spawn(move || {
            let started = Instant::now();
            let (by_def, detected) = library::scan_all(&defs, &roots);
            let found: Vec<(manifest::GameDef, PathBuf)> = by_def.into_iter().map(|(i, dir)| (defs[i].clone(), dir)).collect();
            let steam = games::steam_scan();
            log::info!(
                "scan: {} by mod packages, {} by engine, {} Steam in {:?}",
                found.len(),
                detected.len(),
                steam.len(),
                started.elapsed()
            );
            post(move |a| a.on_scan_done(found, detected, steam, first));
        });
    }

    fn on_scan_done(
        &mut self,
        found: Vec<(manifest::GameDef, PathBuf)>,
        detected: Vec<library::Detected>,
        steam: Vec<games::SteamFound>,
        first: bool,
    ) {
        // папки игр Steam — за поиском Steam (он помнит, кого человек убрал из библиотеки)
        let steam_dirs: Vec<String> = steam.iter().map(|(app, _, _)| app.dir.to_string_lossy().to_lowercase()).collect();
        let mut added = self.add_steam_games(steam);
        for (def, dir) in found {
            if self.settings.hidden_games.contains(&library::game_id(&dir)) {
                continue;
            }
            if self.library.add(Game::new(&dir, &def.exe, &def.name, Some(def.id.clone()), false)) {
                added += 1;
            }
        }
        for d in detected {
            let lower = d.dir.to_string_lossy().to_lowercase();
            if steam_dirs.iter().any(|s| lower == *s || lower.starts_with(&format!("{s}\\"))) {
                continue;
            }
            if self.settings.hidden_games.contains(&library::game_id(&d.dir)) {
                continue;
            }
            if self.library.add(Game::new(&d.dir, &d.exe, &d.name, None, false)) {
                added += 1;
            }
        }
        self.relink_games();
        self.library.save();
        self.settings.first_scan_done = true;
        self.settings.scan_version = SCAN_VERSION;
        self.settings.save();
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_scanning(false);
        }
        self.refresh_games();
        if added > 0 || !first {
            let msg = if added > 0 {
                format!("{}{}", self.t("Новых игр в библиотеке: ", "New games in the library: "), added)
            } else {
                self.t("Новых игр не найдено", "No new games found")
            };
            self.toast(if added > 0 { Tone::Success } else { Tone::Info }, "", &msg);
        }
    }

    fn on_running(&mut self, running: Running) {
        if running.same_games(&self.running, &self.library.games) {
            return;
        }
        self.running = running;
        self.refresh_games();
        if let Some(game) = self.current() {
            let mut card = self.game_card(&game);
            card.folder = game.dir.display().to_string().into();
            if let Some(ui) = self.ui() {
                ui.global::<Store>().set_game(card);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Установка

    fn find_mod(&self, id: &str) -> Option<ModManifest> {
        self.mods.iter().find(|m| m.id == id).cloned()
    }

    fn install_mod(&mut self, mod_id: &str) {
        let (Some(game), Some(m)) = (self.current(), self.find_mod(mod_id)) else { return };
        self.run_install(m, vec![game], true);
    }

    fn uninstall_mod(&mut self, mod_id: &str) {
        let (Some(game), Some(m)) = (self.current(), self.find_mod(mod_id)) else { return };
        self.run_install(m, vec![game], false);
    }

    fn install_everywhere(&mut self, mod_id: &str) {
        let Some(m) = self.find_mod(mod_id) else { return };
        let games: Vec<Game> = self.library.games.iter().filter(|g| g.has_def(&m.game.id)).cloned().collect();
        if !games.is_empty() {
            self.run_install(m, games, true);
        }
    }

    fn run_install(&mut self, m: ModManifest, games: Vec<Game>, install: bool) {
        self.busy.insert(m.id.clone());
        self.refresh_game_page();
        self.refresh_all_mods();
        std::thread::spawn(move || {
            let results: Vec<(Game, Result<(), String>)> = games
                .into_iter()
                .map(|g| {
                    let r = if install { install::install(&g, &m) } else { install::uninstall(&g, &m) };
                    (g, r.map_err(|e| format!("{e:#}")))
                })
                .collect();
            post(move |a| a.on_install_done(m, results, install));
        });
    }

    fn on_install_done(&mut self, m: ModManifest, results: Vec<(Game, Result<(), String>)>, install: bool) {
        self.busy.remove(&m.id);
        self.states.clear();
        let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
        for (g, r) in &results {
            if let Err(e) = r {
                let title = format!("{} — {}", m.name, g.folder_name());
                self.toast(Tone::Danger, &title, e);
            }
        }
        if ok > 0 {
            let running = results.iter().any(|(g, r)| r.is_ok() && self.running.is_running(g));
            let title = if install {
                format!("{} {}", m.name, self.t("установлен", "installed"))
            } else {
                format!("{} {}", m.name, self.t("удалён", "removed"))
            };
            let text = if results.len() > 1 {
                format!("{}{} / {}", self.t("Копий игры: ", "Game copies: "), ok, results.len())
            } else if running {
                self.t("Игра запущена — изменения вступят в силу после перезапуска.", "The game is running — restart it to apply.")
            } else if install {
                self.t("Запустите игру — мод загрузится сам.", "Start the game — the mod loads by itself.")
            } else {
                String::new()
            };
            self.toast(Tone::Success, &title, &text);
        }
        // поставили в сборку, которую автор не проверял, — предупредить (не запрещать)
        if install {
            for (g, _) in results.iter().filter(|(_, r)| r.is_ok()) {
                let note = self.build_note(g, &m);
                if note.state == "unknown" {
                    let title = format!("{} — {}", m.name, g.folder_name());
                    self.toast(Tone::Warning, &title, &note.text);
                }
            }
        }
        self.refresh_games();
        self.refresh_game_page();
        self.refresh_all_mods();
    }

    fn build_mod(&mut self, mod_id: &str) {
        let Some(m) = self.find_mod(mod_id) else { return };
        let Some(cmd) = m.dev.as_ref().and_then(|d| d.build.clone()) else { return };
        self.busy.insert(m.id.clone());
        self.refresh_game_page();
        self.refresh_all_mods();
        self.log_line(&m.id, &format!("$ {cmd}"), "warning");
        std::thread::spawn(move || {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", &cmd]).current_dir(&m.dir);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let result = command.output().map_err(|e| e.to_string());
            post(move |a| a.on_build_done(m, result));
        });
    }

    fn on_build_done(&mut self, m: ModManifest, result: Result<std::process::Output, String>) {
        self.busy.remove(&m.id);
        match result {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
                for line in text.lines().filter(|l| !l.trim().is_empty()).take(200) {
                    self.log_line(&m.id, line.trim_end(), if out.status.success() { "" } else { "danger" });
                }
                if out.status.success() {
                    let title = format!("{} {}", m.name, self.t("собран", "built"));
                    self.toast(Tone::Success, &title, &self.t("Можно ставить в игру.", "Ready to install."));
                } else {
                    let title = format!("{} {}", m.name, self.t("не собрался", "failed to build"));
                    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned();
                    self.toast(Tone::Danger, &title, &last);
                }
            }
            Err(e) => {
                let title = self.t("Сборка не запустилась", "Build didn't start");
                self.toast(Tone::Danger, &title, &e);
            }
        }
        self.reload_mods(false);
    }

    fn open_mod_folder(&mut self, mod_id: &str) {
        if let Some(m) = self.find_mod(mod_id) {
            modly_core::win::shell_open(&m.dir.display().to_string());
        }
    }

    /// Ссылка, папка или путь внутри репозитория Modly (`docs`, `sdk/unity`).
    fn open_link(&mut self, link: &str) {
        // документация — на сайте модов: поиск, оглавление, два языка
        if link == "docs" {
            modly_core::win::shell_open(&format!("{}/docs.php?p=sdk", modly_core::site::site_url()));
            return;
        }
        if link.starts_with("http://") || link.starts_with("https://") || Path::new(link).is_absolute() {
            modly_core::win::shell_open(link);
            return;
        }
        let mut cur = modly_core::paths::exe_dir();
        for _ in 0..6 {
            let candidate = cur.join(link);
            if candidate.exists() && cur.join("docs").join("PROTOCOL.md").exists() {
                modly_core::win::shell_open(&candidate.display().to_string());
                return;
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
        let text = self.t("Папка лежит в репозитории Modly: ", "This folder is in the Modly repository: ") + link;
        self.toast(Tone::Info, "", &text);
    }

    // -----------------------------------------------------------------------------------------
    // Настройки

    fn set_language(&mut self, lang: &str) {
        self.settings.language = lang.to_owned();
        self.settings.save();
        self.hub.set_lang(self.lang());
        self.apply_settings_to_ui();
        self.refresh_games();
        self.refresh_game_page();
        self.refresh_all_mods();
        self.refresh_sessions();
        self.catalog_cards();
        self.map_render();
    }

    fn set_developer_mode(&mut self, on: bool) {
        self.settings.developer_mode = on;
        self.settings.save();
        self.hub.set_trace(on);
        self.apply_settings_to_ui();
    }

    /// Клавиша оверлея: одна из предложенных или "off".
    fn set_overlay_hotkey(&mut self, key: &str) {
        if key != "off" && Hotkey::parse(key).is_none() {
            return;
        }
        self.settings.overlay_hotkey = if key == overlay::DEFAULT_KEY { String::new() } else { key.to_owned() };
        self.settings.save();
        if key == "off" {
            self.hide_overlay();
        }
        self.sync_hotkeys();
        self.apply_settings_to_ui();
    }

    /// Связка с WeMod: только кнопка «Открыть в WeMod». Трейнеры WeMod закрытые и работают в самом WeMod —
    /// Modly их не скачивает и не запускает.
    fn set_wemod(&mut self, on: bool) {
        self.settings.wemod = on;
        self.settings.save();
        self.apply_settings_to_ui();
    }

    fn set_gpu_rendering(&mut self, on: bool) {
        self.settings.gpu_rendering = on;
        self.settings.save();
        self.apply_settings_to_ui();
        let text = self.t("Перезапустите Modly, чтобы сменить способ отрисовки.", "Restart Modly to switch the renderer.");
        self.toast(Tone::Info, "", &text);
    }

    fn add_mod_dir(&mut self) {
        let title = self.t("Папка с модами", "Mods folder");
        if let Some(dir) = rfd::FileDialog::new().set_title(&title).pick_folder() {
            if !self.settings.mod_dirs.contains(&dir) {
                self.settings.mod_dirs.push(dir);
                self.settings.save();
            }
            self.apply_settings_to_ui();
            self.reload_mods(true);
        }
    }

    fn remove_mod_dir(&mut self, index: i32) {
        let dirs = self.mod_dirs();
        if let Some(dir) = dirs.get(index as usize) {
            let before = self.settings.mod_dirs.len();
            self.settings.mod_dirs.retain(|d| d != dir);
            if self.settings.mod_dirs.len() == before {
                let text = self.t("Это папка по умолчанию — её нельзя убрать.", "This is a default folder and can't be removed.");
                self.toast(Tone::Info, "", &text);
                return;
            }
            self.settings.save();
            self.apply_settings_to_ui();
            self.reload_mods(false);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Уведомления и консоль

    pub fn toast(&mut self, tone: Tone, title: &str, text: &str) {
        let secs = if tone == Tone::Danger { 8 } else { 4 };
        self.toast_for(tone, title, text, Duration::from_secs(secs));
    }

    /// Уведомление, которое само скроется через `shown`. Возвращает его номер.
    fn toast_for(&mut self, tone: Tone, title: &str, text: &str, shown: Duration) -> i32 {
        let id = self.next_toast;
        self.next_toast += 1;
        while self.m_toasts.row_count() >= 4 {
            self.m_toasts.remove(0);
        }
        self.m_toasts.push(Toast { id, tone: tone.as_str().into(), title: title.into(), text: text.into() });
        slint::Timer::single_shot(shown, move || with(move |a| a.dismiss_toast(id)));
        id
    }

    fn dismiss_toast(&mut self, id: i32) {
        if let Some(i) = (0..self.m_toasts.row_count()).find(|&i| self.m_toasts.row_data(i).is_some_and(|t| t.id == id)) {
            self.m_toasts.remove(i);
        }
    }

    fn log_line(&mut self, source: &str, text: &str, tone: &str) {
        let mut text = text.to_owned();
        if text.len() > 600 {
            let mut cut = 600;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push('…');
        }
        if self.m_log.row_count() >= MAX_LOG {
            self.m_log.remove(0);
        }
        self.m_log.push(LogLine {
            time: modly_core::win::local_time_hms().into(),
            source: source.into(),
            text: text.into(),
            tone: tone.into(),
        });
    }

    // -----------------------------------------------------------------------------------------
    // Хаб

    fn on_hub(&mut self, e: HubEvent) {
        match e {
            HubEvent::Listening => self.set_hub_state("ok", &self.t("Мост открыт — моды могут подключаться", "Bridge open — mods can connect")),
            HubEvent::ListenFailed(err) => {
                let text = self.t("Мост занят: ", "Bridge busy: ") + &err;
                self.set_hub_state("error", &text);
            }
            HubEvent::Connected { id, hello } => self.on_connected(id, hello),
            HubEvent::Disconnected { id } => self.on_disconnected(id),
            HubEvent::Message { id, msg } => self.on_message(id, msg),
            HubEvent::BadMessage { id, error, line } => {
                let src = self.session_source(id);
                self.log_line(&src, &format!("{error}: {line}"), "danger");
            }
            HubEvent::Traffic { id, outgoing, line } => {
                let src = format!("{} {}", self.session_source(id), if outgoing { "←" } else { "→" });
                self.log_line(&src, &line, if outgoing { "out" } else { "in" });
            }
        }
    }

    fn set_hub_state(&mut self, state: &str, text: &str) {
        if let Some(ui) = self.ui() {
            let s = ui.global::<Store>();
            s.set_hub_state(state.into());
            s.set_hub_text(text.into());
        }
    }

    fn session_source(&self, id: SessionId) -> String {
        self.sessions.get(&id).map(|s| s.hello.mod_info.id.clone()).unwrap_or_else(|| format!("#{id}"))
    }

    /// Чья это игра: по процессу мода, иначе по описанию игры из hello. Незнакомую игру добавляем в библиотеку.
    fn resolve_game(&mut self, hello: &modly_core::hub::Hello) -> Option<String> {
        let exe = hello.pid.and_then(process::exe_of);
        if let Some(exe) = &exe {
            if let Some(g) = self.library.find_by_exe(exe) {
                return Some(g.id.clone());
            }
        }
        if let Some(info) = &hello.game {
            let candidates: Vec<&Game> = self.library.games.iter().filter(|g| g.has_def(&info.id)).collect();
            if let Some(g) = candidates.iter().find(|g| self.running.is_running(g)).or(candidates.first()) {
                if exe.is_none() {
                    return Some(g.id.clone());
                }
            }
        }
        // игра запущена из папки, которой нет в библиотеке, — добавим её
        let exe = exe?;
        let dir = exe.parent()?;
        let exe_name = exe.file_name()?.to_string_lossy().into_owned();
        let def = self.mods.iter().find(|m| library::matches_def(dir, &m.game)).map(|m| m.game.clone());
        let name = def
            .as_ref()
            .map(|d| d.name.clone())
            .or_else(|| hello.game.as_ref().map(|g| g.name.clone()).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| library::guess_name(dir, &exe_name));
        let game = Game::new(dir, &exe_name, &name, def.map(|d| d.id), false);
        let id = game.id.clone();
        self.library.add(game);
        self.relink_games();
        self.library.save();
        Some(id)
    }

    fn on_connected(&mut self, id: SessionId, hello: modly_core::hub::Hello) {
        let game_id = self.resolve_game(&hello);
        let session = Session::new(id, hello, game_id.clone());
        let game_name = game_id.as_ref().and_then(|g| self.library.get(g)).map(|g| g.name.clone()).unwrap_or_default();
        let title = format!("{} {}", session.name(), self.t("на связи", "connected"));
        self.sessions.insert(id, session);
        self.toast(Tone::Success, &title, &game_name);
        if self.current_game.is_some() && self.current_game == game_id && self.live_session.is_none() {
            self.live_session = Some(id);
            self.live_page = 0;
        }
        self.refresh_sessions();
        self.refresh_games();
        self.refresh_game_page();
    }

    fn on_disconnected(&mut self, id: SessionId) {
        if let Some(s) = self.sessions.remove(&id) {
            let title = format!("{} {}", s.name(), self.t("отключился", "disconnected"));
            self.toast(Tone::Info, &title, "");
        }
        if self.live_session == Some(id) {
            self.live_session = None;
            self.live_page = 0;
        }
        if self.capture.as_ref().is_some_and(|(s, _)| *s == id) {
            self.end_capture();
        }
        self.armed.retain(|(s, _), _| *s != id);
        self.sync_hotkeys();
        self.refresh_sessions();
        self.refresh_games();
        self.refresh_game_page();
    }

    fn on_message(&mut self, id: SessionId, msg: ClientMsg) {
        let lang = self.lang();
        let is_current = self.live_session == Some(id);
        match msg {
            ClientMsg::Layout { pages } => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.set_layout(pages);
                }
                self.sync_hotkeys();
                if is_current {
                    self.rebuild_live();
                }
            }
            ClientMsg::State { values } => {
                let Some(s) = self.sessions.get_mut(&id) else { return };
                let ids: Vec<String> = values.keys().cloned().collect();
                for (k, v) in values {
                    s.values.insert(k, v);
                }
                if is_current {
                    self.update_rows(&ids);
                }
            }
            ClientMsg::Patch { controls } => {
                let Some(s) = self.sessions.get_mut(&id) else { return };
                let mut rebuild = false;
                for (cid, p) in &controls {
                    rebuild |= s.patch(cid, p);
                }
                if rebuild {
                    self.sync_hotkeys(); // у скрытых элементов клавиши не держим
                }
                if is_current {
                    if rebuild {
                        self.rebuild_live();
                    } else {
                        let ids: Vec<String> = controls.keys().cloned().collect();
                        self.update_rows(&ids);
                    }
                }
            }
            ClientMsg::Notify { tone, text } => {
                let title = self.sessions.get(&id).map(|s| s.name().to_owned()).unwrap_or_default();
                self.toast(tone, &title, text.resolve(lang));
            }
            ClientMsg::Status { tone, text } => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.status = (!text.is_empty()).then_some((tone, text));
                }
                self.refresh_sessions();
                if is_current {
                    self.update_status();
                }
            }
            ClientMsg::Result { ok, text, .. } => {
                let title = self.sessions.get(&id).map(|s| s.name().to_owned()).unwrap_or_default();
                match (ok, text) {
                    (false, t) => {
                        let t = t.map(|t| t.resolve(lang).to_owned()).unwrap_or_else(|| self.t("Не получилось", "Failed"));
                        self.toast(Tone::Danger, &title, &t);
                    }
                    (true, Some(t)) if !t.is_empty() => self.toast(Tone::Success, &title, t.resolve(lang)),
                    _ => {}
                }
            }
            ClientMsg::Log { tone, text } => {
                let src = self.session_source(id);
                self.log_line(&src, &text, tone.as_str());
            }
            ClientMsg::Paused { paused } => {
                log::info!("session #{id}: game {}", if paused { "paused (main thread stalled)" } else { "resumed" });
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.paused = paused;
                }
                if is_current {
                    self.update_status();
                }
            }
            ClientMsg::Map(map) => self.on_mod_map(id, map),
            ClientMsg::MapPatch { map, set, remove } => self.on_mod_map_patch(id, &map, set, remove),
            ClientMsg::MapRemove { map } => self.on_mod_map_remove(id, &map),
            ClientMsg::Hello { .. } | ClientMsg::Pong | ClientMsg::Bye => {}
        }
    }

    fn refresh_sessions(&mut self) {
        let lang = self.lang();
        let tabs: Vec<SessionTab> = self
            .sessions
            .values()
            .map(|s| {
                let game = s
                    .game_id
                    .as_ref()
                    .and_then(|g| self.library.get(g))
                    .map(|g| g.name.clone())
                    .or_else(|| s.hello.game.as_ref().map(|g| g.name.clone()))
                    .unwrap_or_default();
                let (status, tone) = s.status.as_ref().map(|(t, x)| (x.resolve(lang).to_owned(), t.as_str())).unwrap_or_default();
                SessionTab {
                    id: s.id as i32,
                    title: s.name().into(),
                    subtitle: s.hello.mod_info.author.clone().unwrap_or_default().into(),
                    version: s.hello.mod_info.version.as_str().into(),
                    sdk: s.hello.sdk.clone().unwrap_or_default().into(),
                    game: game.into(),
                    status: status.into(),
                    status_tone: tone.into(),
                }
            })
            .collect();
        self.m_sessions.set_vec(tabs);
    }

    // -----------------------------------------------------------------------------------------
    // Панель управления

    fn game_sessions(&self) -> Vec<SessionId> {
        match &self.current_game {
            Some(g) => self.sessions.values().filter(|s| s.game_id.as_ref() == Some(g)).map(|s| s.id).collect(),
            None => Vec::new(),
        }
    }

    fn rebuild_live(&mut self) {
        let Some(ui) = self.ui() else { return };
        let store = ui.global::<Store>();
        let lang = self.lang();
        let ids = self.game_sessions();
        if self.live_session.is_none_or(|s| !ids.contains(&s)) {
            self.live_session = ids.first().copied();
            self.live_page = 0;
        }
        let tabs: Vec<SessionTab> = ids
            .iter()
            .filter_map(|id| self.sessions.get(id))
            .map(|s| SessionTab {
                id: s.id as i32,
                title: s.name().into(),
                version: s.hello.mod_info.version.as_str().into(),
                ..Default::default()
            })
            .collect();
        self.m_live_tabs.set_vec(tabs);
        store.set_live_tab(self.live_session.and_then(|s| ids.iter().position(|x| *x == s)).unwrap_or(0) as i32);

        let Some(session) = self.live_session.and_then(|id| self.sessions.get(&id)) else {
            store.set_live_connected(false);
            self.m_live_pages.set_vec(Vec::new());
            self.m_live_sections.set_vec(Vec::new());
            self.rows.clear();
            self.section_models.clear();
            self.end_capture();
            return;
        };
        if self.live_page >= session.pages.len() {
            self.live_page = 0;
        }
        let mod_id = &session.hello.mod_info.id;
        let built = live::build_page(session, self.live_page, lang, &|c| self.hotkey_view(mod_id, c));
        let mut subtitle = String::new();
        if !session.hello.mod_info.version.is_empty() {
            subtitle.push('v');
            subtitle.push_str(&session.hello.mod_info.version);
        }
        if let Some(author) = session.hello.mod_info.author.as_deref().filter(|a| !a.is_empty()) {
            subtitle.push_str("  ·  ");
            subtitle.push_str(author);
        }
        if session.pages.is_empty() {
            subtitle.push_str("  ·  ");
            subtitle.push_str(if lang == "ru" { "мод ещё не прислал интерфейс" } else { "the mod hasn't sent its UI yet" });
        }
        store.set_live_connected(true);
        store.set_live_title(session.name().into());
        store.set_live_subtitle(subtitle.into());
        store.set_live_page(self.live_page as i32);
        self.m_live_pages.set_vec(built.pages);
        self.m_live_sections.set_vec(built.sections);
        self.section_models = built.section_models;
        self.rows = built.rows;
        // элемент, которому назначали клавишу, ушёл с экрана (другая страница, другой мод)
        if self.capture.as_ref().is_some_and(|(sid, cid)| Some(*sid) != self.live_session || !self.rows.contains_key(cid)) {
            self.end_capture();
        }
        self.update_status();
    }

    fn update_status(&mut self) {
        let Some(ui) = self.ui() else { return };
        let store = ui.global::<Store>();
        let lang = self.lang();
        let session = self.live_session.and_then(|id| self.sessions.get(&id));
        store.set_live_paused(session.is_some_and(|s| s.paused));
        let status = session.and_then(|s| s.status.clone());
        match status {
            Some((tone, text)) => {
                store.set_live_status(text.resolve(lang).into());
                store.set_live_status_tone(tone.as_str().into());
            }
            None => store.set_live_status(SharedString::new()),
        }
    }

    fn update_rows(&mut self, ids: &[String]) {
        let lang = self.lang();
        let Some(session) = self.live_session.and_then(|id| self.sessions.get(&id)) else { return };
        let mod_id = &session.hello.mod_info.id;
        for id in ids {
            if let (Some((si, ci)), Some(c)) = (self.rows.get(id), session.control(id)) {
                if let Some(model) = self.section_models.get(*si) {
                    let lc = live::to_live_with_key(c, session.values.get(id), lang, &|c| self.hotkey_view(mod_id, c));
                    model.set_row_data(*ci, lc);
                }
            }
        }
    }

    fn live_select_tab(&mut self, index: i32) {
        let ids = self.game_sessions();
        if let Some(id) = ids.get(index as usize) {
            self.live_session = Some(*id);
            self.live_page = 0;
            self.rebuild_live();
        }
    }

    fn live_select_page(&mut self, index: i32) {
        self.live_page = index.max(0) as usize;
        self.rebuild_live();
    }

    /// Отправить значение моду. `optimistic` — сразу показать его, не дожидаясь ответа.
    fn live_set(&mut self, id: &str, value: Value, optimistic: bool) {
        if let Some(sid) = self.live_session {
            self.set_value(sid, id, value, optimistic);
        }
    }

    /// Отправить значение моду в любой сессии (горячие клавиши работают и для мода, которого не видно).
    fn set_value(&mut self, sid: SessionId, id: &str, value: Value, optimistic: bool) {
        self.hub.set(sid, id, value.clone());
        if optimistic {
            if let Some(s) = self.sessions.get_mut(&sid) {
                s.values.insert(id.to_owned(), value);
            }
            if self.live_session == Some(sid) {
                self.update_rows(&[id.to_owned()]);
            }
        }
    }

    fn step_of(&self, id: &str) -> f64 {
        let control = self.live_session.and_then(|s| self.sessions.get(&s)).and_then(|s| s.control(id));
        match control.map(|c| &c.kind) {
            Some(ControlKind::Slider { step, .. }) => step.unwrap_or(0.0),
            Some(ControlKind::Number { step, .. }) => step.unwrap_or(1.0),
            _ => 0.0,
        }
    }

    /// Перетаскивание ползунка: не чаще ~14 раз в секунду, окончательное значение придёт по отпусканию.
    fn live_drag(&mut self, id: &str, v: f32) {
        let now = Instant::now();
        if self.drag_sent.get(id).is_some_and(|t| now.duration_since(*t) < Duration::from_millis(70)) {
            return;
        }
        self.drag_sent.insert(id.to_owned(), now);
        let value = live::number_value(v as f64, self.step_of(id));
        self.live_set(id, value, false);
    }

    fn live_number(&mut self, id: &str, v: f32) {
        self.drag_sent.remove(id);
        let value = live::number_value(v as f64, self.step_of(id));
        self.live_set(id, value, true);
    }

    fn live_pick(&mut self, id: &str, option: &str) {
        let Some(sid) = self.live_session else { return };
        let Some(session) = self.sessions.get(&sid) else { return };
        let Some(control) = session.control(id) else { return };
        let value = match &control.kind {
            ControlKind::Select { multiple: true, .. } => {
                let mut list: Vec<Value> = match session.values.get(id) {
                    Some(Value::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                if let Some(pos) = list.iter().position(|v| v.as_str() == Some(option)) {
                    list.remove(pos);
                } else {
                    list.push(Value::String(option.to_owned()));
                }
                Value::Array(list)
            }
            _ => Value::String(option.to_owned()),
        };
        self.live_set(id, value, true);
    }

    fn live_invoke(&mut self, id: &str) {
        if let Some(sid) = self.live_session {
            self.hub.invoke(sid, id);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Горячие клавиши

    /// Клавиша элемента для панели: сочетание и занято ли оно другой программой.
    fn hotkey_view(&self, mod_id: &str, c: &Control) -> (SharedString, bool) {
        match control_hotkey(&self.settings, mod_id, c) {
            Some(k) => (k.to_string().into(), self.hotkey_failed.contains(&k)),
            None => (SharedString::new(), false),
        }
    }

    /// Перерисовать клавиши в строках открытой страницы.
    fn refresh_hotkey_rows(&mut self) {
        let ids: Vec<String> = self.rows.keys().cloned().collect();
        self.update_rows(&ids);
    }

    /// Зарегистрировать клавиши видимых переключателей и кнопок всех подключённых модов.
    /// Пока мод не подключён, его клавиши не заняты — в других программах они работают как обычно.
    fn sync_hotkeys(&mut self) {
        let mut map: HashMap<Hotkey, Vec<(SessionId, String)>> = HashMap::new();
        for s in self.sessions.values() {
            for c in s.pages.iter().flat_map(|p| &p.sections).flat_map(|sec| &sec.controls).filter(|c| c.visible) {
                if let Some(k) = control_hotkey(&self.settings, &s.hello.mod_info.id, c) {
                    map.entry(k).or_default().push((s.id, c.id.clone()));
                }
            }
        }
        let mut keys: Vec<Hotkey> = map.keys().copied().collect();
        if let Some(k) = self.overlay_key() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        keys.sort();
        self.hotkey_map = map;
        if let Some(h) = &self.hotkeys {
            h.set(keys);
        }
    }

    fn on_hotkey(&mut self, e: HotkeyEvent) {
        match e {
            HotkeyEvent::Pressed { key, foreground_pid } => self.fire_hotkey(key, foreground_pid),
            HotkeyEvent::Failed(keys) => {
                let fresh: Vec<String> = keys.iter().filter(|k| !self.hotkey_failed.contains(k)).map(|k| k.to_string()).collect();
                self.hotkey_failed = keys.into_iter().collect();
                if !fresh.is_empty() {
                    let title = self.t("Горячая клавиша занята", "Hotkey is taken");
                    let text = fresh.join(", ")
                        + &self.t(" — её уже использует другая программа. Назначьте другую.", " — another program already uses it. Pick a different one.");
                    self.toast(Tone::Warning, &title, &text);
                }
                self.refresh_hotkey_rows();
            }
        }
    }

    fn fire_hotkey(&mut self, key: Hotkey, foreground: Option<u32>) {
        if self.overlay_key() == Some(key) {
            self.toggle_overlay(foreground);
            return;
        }
        let Some(bindings) = self.hotkey_map.get(&key).cloned() else { return };
        // мод запущен в нескольких копиях игры — нажимаем в той, что сейчас на экране
        let in_front: Vec<(SessionId, String)> = bindings
            .iter()
            .filter(|(sid, _)| foreground.is_some() && self.sessions.get(sid).and_then(|s| s.hello.pid) == foreground)
            .cloned()
            .collect();
        let targets = if in_front.is_empty() { bindings } else { in_front };
        for (sid, cid) in targets {
            self.trigger(sid, &cid, key);
        }
    }

    /// Клавиша нажала элемент: переключатель меняет значение на обратное, кнопка срабатывает.
    fn trigger(&mut self, sid: SessionId, cid: &str, key: Hotkey) {
        let lang = self.lang();
        let Some(s) = self.sessions.get(&sid) else { return };
        let Some(c) = s.control(cid) else { return };
        let label = c.label.resolve(lang);
        let title = format!("{key} · {}", if label.is_empty() { cid } else { label });
        let current = s.values.get(cid).or(c.value.as_ref()).and_then(Value::as_bool).unwrap_or(false);
        let (enabled, kind) = (c.enabled, c.kind.clone());
        if !enabled {
            let text = self.t("Сейчас недоступно", "Not available right now");
            self.hotkey_toast(Tone::Info, &title, &text);
            return;
        }
        match kind {
            ControlKind::Toggle => {
                self.set_value(sid, cid, Value::Bool(!current), true);
                let (tone, text) = if current { (Tone::Info, self.t("Выключено", "Off")) } else { (Tone::Success, self.t("Включено", "On")) };
                self.hotkey_toast(tone, &title, &text);
            }
            ControlKind::Button { tone, confirm } => {
                // как в панели: опасная кнопка или кнопка с подтверждением срабатывает со второго нажатия
                let slot = (sid, cid.to_owned());
                let armed = self.armed.get(&slot).is_some_and(|t| t.elapsed() < CONFIRM_WINDOW);
                if (confirm.is_some() || tone == Tone::Danger) && !armed {
                    self.armed.insert(slot, Instant::now());
                    let text = self.t("Нажмите ещё раз, чтобы подтвердить", "Press again to confirm");
                    self.hotkey_toast(Tone::Warning, &title, &text);
                    return;
                }
                self.armed.remove(&slot);
                self.hub.invoke(sid, cid);
                let text = self.t("Нажато", "Pressed");
                self.hotkey_toast(Tone::Success, &title, &text);
            }
            _ => {}
        }
    }

    /// Короткое уведомление о срабатывании клавиши; следующее заменяет предыдущее.
    fn hotkey_toast(&mut self, tone: Tone, title: &str, text: &str) {
        if let Some(id) = self.last_hotkey_toast.take() {
            self.dismiss_toast(id);
        }
        self.last_hotkey_toast = Some(self.toast_for(tone, title, text, Duration::from_millis(2500)));
    }

    /// Пользователь нажал на клавишу элемента: ждём сочетание. Все клавиши пока сняты, иначе
    /// уже назначенное сочетание перехватит Windows и окно его не увидит.
    fn hotkey_capture_start(&mut self, cid: &str) {
        let Some(sid) = self.live_session else { return };
        self.capture = Some((sid, cid.to_owned()));
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_hotkey_capture(cid.into());
        }
        if let Some(h) = &self.hotkeys {
            h.suspend(true);
        }
    }

    fn hotkey_capture_cancel(&mut self, cid: &str) {
        if self.capture.as_ref().is_some_and(|(_, c)| c == cid) {
            self.end_capture();
        }
    }

    fn end_capture(&mut self) {
        if self.capture.take().is_none() {
            return;
        }
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_hotkey_capture(SharedString::new());
        }
        if let Some(h) = &self.hotkeys {
            h.suspend(false);
        }
    }

    /// Назначить элементу клавишу (`None` — снять). Одна клавиша — один элемент: у прежнего владельца она снимается.
    fn hotkey_assign(&mut self, cid: &str, key: Option<Hotkey>) {
        let Some((sid, capturing)) = self.capture.clone() else { return };
        if capturing != cid {
            return;
        }
        self.end_capture();
        let Some(mod_id) = self.sessions.get(&sid).map(|s| s.hello.mod_info.id.clone()) else { return };
        let mut taken_from: Vec<(String, String, String)> = Vec::new();
        if let Some(k) = key {
            let lang = self.lang();
            for s in self.sessions.values() {
                let m = &s.hello.mod_info.id;
                for c in s.pages.iter().flat_map(|p| &p.sections).flat_map(|sec| &sec.controls) {
                    let mine = *m == mod_id && c.id == cid;
                    if !mine && control_hotkey(&self.settings, m, c) == Some(k) && !taken_from.iter().any(|(tm, tc, _)| tm == m && *tc == c.id) {
                        let label = c.label.resolve(lang);
                        taken_from.push((m.clone(), c.id.clone(), if label.is_empty() { c.id.clone() } else { label.to_owned() }));
                    }
                }
            }
            // и у модов, которые сейчас не подключены
            for (m, controls) in &self.settings.hotkeys {
                for (c, v) in controls {
                    let mine = *m == mod_id && c == cid;
                    if !mine && Hotkey::parse(v) == Some(k) && !taken_from.iter().any(|(tm, tc, _)| tm == m && tc == c) {
                        taken_from.push((m.clone(), c.clone(), c.clone()));
                    }
                }
            }
        }
        for (m, c, _) in &taken_from {
            self.settings.set_hotkey(m, c, None);
        }
        self.settings.set_hotkey(&mod_id, cid, key);
        self.settings.save();
        self.sync_hotkeys();
        self.refresh_hotkey_rows();
        if let (Some(k), Some((_, _, label))) = (key, taken_from.first()) {
            let text = format!("{k} {}«{label}»", self.t("больше не нажимает ", "no longer triggers "));
            self.toast(Tone::Info, "", &text);
        }
    }
}

/// Горячая клавиша элемента: назначенная пользователем, иначе предложенная модом. Только у переключателей и кнопок.
fn control_hotkey(settings: &Settings, mod_id: &str, c: &Control) -> Option<Hotkey> {
    if !live::takes_hotkey(c) {
        return None;
    }
    settings.hotkey(mod_id, &c.id, c.hotkey.as_deref())
}
