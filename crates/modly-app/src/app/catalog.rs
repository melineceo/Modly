//! Каталог сайта модов в Modly: список модов, установка в один клик, лайки, необязательный
//! вход и ссылки `modly://install/<адрес>` с кнопки «Установить через Modly» на сайте.
//!
//! Сеть и диск — в отдельных потоках, результаты возвращаются через [`post`]. Сама проверка
//! пакетов (SHA-256, распаковка, пути) — в `modly_core::site`, картинки — в [`crate::thumbs`].

use super::{App, post, with};
use crate::thumbs::{self, Kind};
use crate::{SiteCard, Store};
use modly_core::library::Game;
use modly_core::site::{self, Account, SiteError, SiteMod, SitePackage};
use modly_protocol::Tone;
use slint::{ComponentHandle, Image, ModelRc, VecModel};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Сколько картинок каталога грузится одновременно.
const ART_THREADS: usize = 3;
/// Сколько памяти могут занять готовые логотипы и обложки — сверх этого Modly забывает те, что
/// дольше всех не показывались (вернутся с диска, когда карточка снова появится на экране).
const ART_BUDGET: usize = 10 * 1024 * 1024;

/// Логотип или обложка: (вид, адрес на сайте).
type ArtKey = (Kind, String);

enum Art {
    Queued,
    Loading,
    Failed,
    /// `used` — когда её последний раз просили (для [`ART_BUDGET`]).
    Ready { image: Image, bytes: usize, used: u64 },
}

#[derive(Default)]
pub struct CatalogState {
    mods: Vec<SiteMod>,
    loaded: bool,
    loading: bool,
    search: String,
    /// Пакеты, поставленные из каталога: адрес на сайте → пакет.
    packages: HashMap<String, SitePackage>,
    /// Что сейчас ставится: адрес → прогресс 0…1.
    busy: HashMap<String, f32>,
    /// Скачанные файлы модов без пакета Modly: адрес → файл в «ЗагрузкиModly».
    downloads: HashMap<String, site::SiteDownload>,
    liking: HashSet<String>,
    account: Option<Account>,
    login_cancel: Option<Arc<AtomicBool>>,
    /// Мод из ссылки modly://, которую ждёт подтверждение.
    confirm: Option<SiteMod>,
    /// Мод 18+ из ссылки modly://: сначала «Вам есть 18?», потом — обычное подтверждение установки.
    age_then: Option<SiteMod>,
    /// Сколько модов 18+ каталог не показал (показ выключен в настройках).
    adult_hidden: usize,
    /// С какой настройкой 18+ запрошен каталог, который сейчас грузится.
    adult_req: bool,
    model: Rc<VecModel<SiteCard>>,
    /// Моды каталога для открытой игры (на её странице).
    game_model: Rc<VecModel<SiteCard>>,
    /// Каталог уже пробовали загрузить сами, для страницы игры (без сети — не повторять).
    auto_loaded: bool,
    /// Логотипы и обложки — уменьшенные, в физических пикселях при масштабе `art_scale`.
    art: HashMap<ArtKey, Art>,
    /// Очередь загрузки картинок — в порядке появления карточек на экране.
    art_queue: VecDeque<ArtKey>,
    art_active: usize,
    art_tick: u64,
    art_scale: f32,
}

/// Палитры модов — те же, что на сайте (основной цвет градиента).
fn accent_color(name: &str) -> slint::Color {
    let hex = match name {
        "ocean" => 0x22d3ee,
        "emerald" => 0x10b981,
        "mint" => 0x14b8a6,
        "sunset" => 0xfb923c,
        "tangerine" => 0xff8a30,
        "crimson" => 0xef4444,
        "sakura" => 0xf472b6,
        "gold" => 0xf59e0b,
        "ice" => 0x60a5fa,
        "mono" => 0x6b7280,
        _ => 0x8b5cf6,
    };
    slint::Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8 & 0xff) as u8, (hex & 0xff) as u8)
}

/// Имя канала, по которому второй запуск Modly передаёт ссылку первому. Своё у каждого
/// пользователя Windows; писать в канал может только он сам (права по умолчанию).
fn open_pipe_name() -> String {
    let who = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    format!(r"\\.\pipe\modly-open-{}", modly_core::stable_id(&who))
}

/// Ссылка `modly://…` из аргументов запуска.
pub fn url_from_args(args: &[String]) -> Option<String> {
    args.iter().find(|a| a.len() < 300 && a.to_ascii_lowercase().starts_with("modly://")).cloned()
}

/// Modly уже открыт: передать ему ссылку. `false` — передать не удалось.
pub fn forward_to_running(url: &str) -> bool {
    use std::io::Write;
    for _ in 0..10 {
        match std::fs::OpenOptions::new().write(true).open(open_pipe_name()) {
            Ok(mut f) => return f.write_all(url.as_bytes()).is_ok(),
            // канал занят другим клиентом — подождать
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    false
}

/// Принимать ссылки от вторых запусков Modly.
#[cfg(windows)]
pub fn start_open_listener(rt: &tokio::runtime::Handle) {
    rt.spawn(async move {
        use tokio::io::AsyncReadExt;
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = open_pipe_name();
        let mut first = true;
        loop {
            let server = match ServerOptions::new().first_pipe_instance(first).create(&name) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("open pipe {name}: {e}");
                    return;
                }
            };
            first = false;
            let mut server = server;
            if server.connect().await.is_err() {
                continue;
            }
            let mut buf = Vec::new();
            let mut limited = (&mut server).take(1024);
            let read = limited.read_to_end(&mut buf);
            let _ = tokio::time::timeout(Duration::from_secs(2), read).await;
            let text = String::from_utf8_lossy(&buf).trim().to_owned();
            if !text.is_empty() {
                post(move |a| a.open_url(&text));
            }
        }
    });
}

#[cfg(not(windows))]
pub fn start_open_listener(_rt: &tokio::runtime::Handle) {}

impl App {
    pub(super) fn catalog_init(&mut self) {
        if let Some(ui) = self.ui() {
            let store = ui.global::<Store>();
            store.set_catalog(ModelRc::from(self.cat.model.clone()));
            store.set_game_site_mods(ModelRc::from(self.cat.game_model.clone()));
            store.set_site_host(site::site_host().into());
        }
        self.cat.downloads = site::site_downloads();
        self.cat.packages = site::site_packages(&modly_core::paths::user_mods_dir());
        self.cat.account = site::load_account();
        self.account_to_ui();
        // Вход мог закончиться на сайте («выйти везде», отзыв устройства) — проверяем тихо.
        if let Some(acc) = self.cat.account.clone() {
            std::thread::spawn(move || {
                let r = site::me(&acc.token);
                post(move |a| match r {
                    Ok(user) => {
                        if let Some(own) = a.cat.account.as_mut().filter(|o| o.token == acc.token) {
                            own.user = user;
                            site::update_account_user(own);
                            a.account_to_ui();
                        }
                    }
                    Err(e) if matches!(e.downcast_ref::<SiteError>(), Some(SiteError::NeedLogin)) => {
                        site::forget_account();
                        a.cat.account = None;
                        a.account_to_ui();
                    }
                    Err(e) => log::info!("account check: {e:#}"),
                });
            });
        }
        // Ссылки modly:// ведут на этот exe. Только у настоящей сборки и не в тестовом окружении.
        #[cfg(all(windows, not(debug_assertions)))]
        if std::env::var_os("MODLY_HOME").is_none() {
            std::thread::spawn(|| {
                if let Ok(exe) = std::env::current_exe() {
                    if let Err(e) = modly_core::win::register_url_protocol(&exe) {
                        log::warn!("modly:// register: {e:#}");
                    }
                }
            });
        }
    }

    fn account_to_ui(&mut self) {
        let Some(ui) = self.ui() else { return };
        let store = ui.global::<Store>();
        store.set_signed_in(self.cat.account.is_some());
        store.set_account_nick(self.cat.account.as_ref().map(|a| a.user.nick.as_str()).unwrap_or("").into());
        store.set_account_busy(self.cat.login_cancel.is_some());
    }

    /// Открыли страницу каталога: в первый раз — загрузить.
    pub(super) fn catalog_show(&mut self) {
        self.art_scale();
        if !self.cat.loaded && !self.cat.loading {
            self.catalog_refresh();
        } else {
            self.catalog_cards();
        }
    }

    pub(super) fn catalog_refresh(&mut self) {
        if self.cat.loading {
            return;
        }
        self.cat.loading = true;
        self.cat.packages = site::site_packages(&modly_core::paths::user_mods_dir());
        self.cat.downloads = site::site_downloads();
        // картинки, которые не загрузились, — попробовать снова
        self.cat.art.retain(|_, a| !matches!(a, Art::Failed));
        self.catalog_state("loading", "");
        let token = self.cat.account.as_ref().map(|a| a.token.clone());
        let adult = self.settings.show_adult;
        self.cat.adult_req = adult;
        std::thread::spawn(move || {
            let r = site::catalog(token.as_deref(), adult);
            post(move |a| a.on_catalog(r));
        });
    }

    fn on_catalog(&mut self, r: anyhow::Result<site::Catalog>) {
        self.cat.loading = false;
        // пока каталог грузился, показ 18+ переключили — загрузить заново, с новой настройкой
        if r.is_ok() && self.cat.adult_req != self.settings.show_adult {
            self.catalog_refresh();
            return;
        }
        match r {
            Ok(site::Catalog { mods, adult_hidden }) => {
                self.cat.adult_hidden = adult_hidden;
                self.adult_to_ui();
                // картинки модов, которых в каталоге больше нет (или у которых новый логотип), — забыть
                let used: HashSet<&str> =
                    mods.iter().chain(&self.cat.confirm).flat_map(|m| [m.logo.as_str(), m.cover.as_str()]).collect();
                self.cat.art.retain(|k, a| used.contains(k.1.as_str()) || matches!(a, Art::Loading));
                self.cat.art_queue.retain(|k| used.contains(k.1.as_str()));
                self.cat.mods = mods;
                self.cat.loaded = true;
                self.catalog_state("ok", "");
            }
            Err(e) => {
                if matches!(e.downcast_ref::<SiteError>(), Some(SiteError::NeedLogin)) {
                    // токен отозван — забываем и грузим без него
                    site::forget_account();
                    self.cat.account = None;
                    self.account_to_ui();
                    self.catalog_refresh();
                    return;
                }
                log::warn!("catalog: {e:#}");
                self.catalog_state("error", &format!("{e:#}"));
            }
        }
        self.catalog_cards();
    }

    fn catalog_state(&self, state: &str, error: &str) {
        if let Some(ui) = self.ui() {
            let store = ui.global::<Store>();
            store.set_catalog_state(state.into());
            store.set_catalog_error(error.into());
        }
    }

    pub(super) fn catalog_search(&mut self, text: String) {
        self.cat.search = text.trim().to_lowercase();
        self.catalog_cards();
    }

    /// Копия игры мода в библиотеке.
    fn owned_game(&self, m: &SiteMod) -> Option<&Game> {
        self.library.games.iter().find(|g| game_fits(g, m) && g.exists())
    }

    /// Открыли игру: моды каталога для неё, которых в ней ещё нет, — под её модами. Каталог
    /// подгружается сам, если его ещё не открывали.
    pub(super) fn catalog_for_game(&mut self) {
        if !self.cat.loaded && !self.cat.loading && !self.cat.auto_loaded {
            self.cat.auto_loaded = true;
            self.catalog_refresh();
        }
        let cards: Vec<SiteCard> = match self.current() {
            Some(g) => {
                // мод уже есть у игры (поставлен или лежит у разработчика) — он в списке выше
                let local: HashSet<String> = self.mods_for(&g).into_iter().map(|m| m.id).collect();
                self.cat
                    .mods
                    .iter()
                    .filter(|m| game_fits(&g, m))
                    .map(|m| (m, self.site_card(m)))
                    .filter(|(m, c)| c.state == "outdated" || (c.state != "installed" && !local.contains(&m.slug)))
                    .map(|(_, c)| c)
                    .collect()
            }
            None => Vec::new(),
        };
        self.cat.game_model.set_vec(cards);
    }

    fn site_card(&self, m: &SiteMod) -> SiteCard {
        let latest = m.latest.as_ref();
        let modly = latest.is_some_and(|l| l.modly);
        let state = if self.cat.busy.contains_key(&m.slug) {
            "busy"
        } else if modly || self.cat.packages.contains_key(&m.slug) {
            match (self.cat.packages.get(&m.slug), latest) {
                (Some(p), Some(l)) if p.version != l.version && l.modly => "outdated",
                (Some(_), _) => "installed",
                _ => "none",
            }
        } else {
            // мод без пакета Modly: файл можно скачать в «Загрузки»
            match (self.cat.downloads.get(&m.slug), latest) {
                (Some(d), Some(l)) if d.version != l.version => "outdated",
                (Some(_), _) => "downloaded",
                _ => "none",
            }
        };
        let (logo, has_logo) = self.art_image(Kind::Logo, &m.logo);
        let (cover, has_cover) = self.art_image(Kind::Cover, &m.cover);
        let owned = self.owned_game(m);
        SiteCard {
            slug: m.slug.as_str().into(),
            title: m.title.as_str().into(),
            game: m.game.as_str().into(),
            tagline: m.tagline.get(self.lang()).into(),
            letter: m.title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
            accent: accent_color(&m.accent),
            logo,
            has_logo,
            cover,
            has_cover,
            version: latest.map(|l| l.version.as_str()).unwrap_or("").into(),
            downloads: m.downloads.clamp(0, i32::MAX as i64) as i32,
            likes: m.likes.clamp(0, i32::MAX as i64) as i32,
            liked: m.liked,
            modly,
            downloadable: latest.is_some_and(|l| l.download || l.modly),
            featured: m.featured,
            adult: m.adult,
            state: state.into(),
            progress: self.cat.busy.get(&m.slug).copied().unwrap_or(0.0),
            owned: owned.is_some(),
            owned_steam: owned.is_some_and(|g| g.steam_appid.is_some()),
        }
    }

    pub(super) fn catalog_cards(&mut self) {
        let q = self.cat.search.clone();
        let mut cards: Vec<SiteCard> = self
            .cat
            .mods
            .iter()
            .filter(|m| {
                q.is_empty()
                    || m.title.to_lowercase().contains(&q)
                    || m.game.to_lowercase().contains(&q)
                    || m.slug.contains(&q)
                    || m.tagline.get(self.lang()).to_lowercase().contains(&q)
            })
            .map(|m| self.site_card(m))
            .collect();
        // сначала моды для игр, которые у игрока есть; порядок сайта внутри групп сохраняется
        cards.sort_by_key(|c| !c.owned);
        self.cat.model.set_vec(cards);
        self.game_cards_update();
    }

    /// Карточки модов каталога на странице игры — без новой загрузки каталога.
    fn game_cards_update(&mut self) {
        if self.cat.loaded {
            self.catalog_for_game();
        }
    }

    /// Обновить одну карточку (прогресс установки, лайк) без пересборки всего списка.
    fn catalog_card_update(&mut self, slug: &str) {
        let Some(m) = self.cat.mods.iter().find(|m| m.slug == slug).cloned() else { return };
        let card = self.site_card(&m);
        use slint::Model;
        for model in [&self.cat.model, &self.cat.game_model] {
            for i in 0..model.row_count() {
                if model.row_data(i).is_some_and(|c| c.slug == slug) {
                    model.set_row_data(i, card.clone());
                    break;
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Логотипы и обложки

    /// Масштаб экрана, под который готовятся картинки. Окно переехало на экран с другим
    /// масштабом — прежние картинки забываются и перерисуются, когда карточки снова попросят.
    fn art_scale(&mut self) -> f32 {
        let scale = self.ui().map(|ui| ui.window().scale_factor()).unwrap_or(1.0);
        if scale != self.cat.art_scale {
            self.cat.art_scale = scale;
            self.cat.art.retain(|_, a| !matches!(a, Art::Ready { .. }));
        }
        scale
    }

    fn art_image(&self, kind: Kind, url: &str) -> (Image, bool) {
        match self.cat.art.get(&(kind, url.to_owned())) {
            Some(Art::Ready { image, .. }) => (image.clone(), true),
            _ => (Image::default(), false),
        }
    }

    /// Карточка мода `slug` показалась на экране (`near`) или ушла с него. Ушедшие карточки
    /// снимают свои картинки с очереди — при быстрой прокрутке не качается то, что уже не видно.
    pub(super) fn catalog_art(&mut self, slug: &str, near: bool) {
        let Some(m) = self.cat.mods.iter().chain(&self.cat.confirm).chain(&self.cat.age_then).find(|m| m.slug == slug) else { return };
        let keys = [(Kind::Logo, m.logo.clone()), (Kind::Cover, m.cover.clone())];
        for key in keys.into_iter().filter(|k| !k.1.is_empty()) {
            if near {
                self.art_want(key);
            } else if matches!(self.cat.art.get(&key), Some(Art::Queued)) {
                self.cat.art.remove(&key);
                self.cat.art_queue.retain(|k| *k != key);
            }
        }
        self.art_pump();
    }

    fn art_want(&mut self, key: ArtKey) {
        self.cat.art_tick += 1;
        match self.cat.art.get_mut(&key) {
            Some(Art::Ready { used, .. }) => *used = self.cat.art_tick,
            Some(_) => {}
            None => {
                self.cat.art.insert(key.clone(), Art::Queued);
                self.cat.art_queue.push_back(key);
            }
        }
    }

    fn art_pump(&mut self) {
        while self.cat.art_active < ART_THREADS {
            let Some(key) = self.cat.art_queue.pop_front() else { break };
            self.cat.art.insert(key.clone(), Art::Loading);
            self.cat.art_active += 1;
            let size = key.0.size(self.art_scale());
            std::thread::spawn(move || {
                let r = site::thumb(&key.1).and_then(|data| thumbs::decode(&data, key.0, size));
                post(move |a| a.on_art(key, r));
            });
        }
    }

    fn on_art(&mut self, key: ArtKey, r: anyhow::Result<thumbs::Pixels>) {
        self.cat.art_active -= 1;
        match r {
            Ok(px) => {
                self.cat.art_tick += 1;
                let art = Art::Ready { image: px.to_image(), bytes: px.data.len(), used: self.cat.art_tick };
                self.cat.art.insert(key.clone(), art);
                self.art_show(&key.1);
                self.art_trim();
            }
            Err(e) => {
                log::info!("catalog image {}: {e:#}", key.1);
                self.cat.art.insert(key, Art::Failed);
            }
        }
        self.art_pump();
    }

    /// Обновить карточки (и окно подтверждения), где эта картинка.
    fn art_show(&mut self, url: &str) {
        let slugs: Vec<String> = self.cat.mods.iter().filter(|m| m.logo == url || m.cover == url).map(|m| m.slug.clone()).collect();
        for slug in slugs {
            self.catalog_card_update(&slug);
        }
        if let Some(m) = self.cat.confirm.clone().filter(|m| m.logo == url || m.cover == url) {
            let card = self.site_card(&m);
            if let Some(ui) = self.ui() {
                ui.global::<Store>().set_confirm_mod(card);
            }
        }
        if let Some(m) = self.cat.age_then.clone().filter(|m| m.logo == url || m.cover == url) {
            let card = self.site_card(&m);
            if let Some(ui) = self.ui() {
                ui.global::<Store>().set_age_mod(card);
            }
        }
    }

    /// Готовых картинок больше [`ART_BUDGET`] — забыть те, что дольше всех не показывались.
    fn art_trim(&mut self) {
        let mut ready: Vec<(u64, usize, ArtKey)> = self
            .cat
            .art
            .iter()
            .filter_map(|(k, a)| match a {
                Art::Ready { bytes, used, .. } => Some((*used, *bytes, k.clone())),
                _ => None,
            })
            .collect();
        let mut total: usize = ready.iter().map(|r| r.1).sum();
        ready.sort_by_key(|r| r.0);
        for (_, bytes, key) in ready {
            if total <= ART_BUDGET {
                break;
            }
            self.cat.art.remove(&key);
            total -= bytes;
            self.art_show(&key.1);
        }
    }

    // ---------------------------------------------------------------------------------------
    // Установка

    pub(super) fn catalog_install(&mut self, slug: &str) {
        if !site::slug_ok(slug) || self.cat.busy.contains_key(slug) {
            return;
        }
        self.cat.busy.insert(slug.to_owned(), 0.0);
        self.catalog_card_update(slug);
        let slug = slug.to_owned();
        std::thread::spawn(move || {
            let mut progress = progress_poster(slug.clone());
            let r = site::install_package(&slug, &modly_core::paths::user_mods_dir(), &mut progress);
            post(move |a| a.on_package(slug, r));
        });
    }

    /// «Скачать» у мода без пакета Modly: файл — в «Загрузки\Modly», с проверкой SHA-256.
    pub(super) fn catalog_download(&mut self, slug: &str) {
        if !site::slug_ok(slug) || self.cat.busy.contains_key(slug) {
            return;
        }
        self.cat.busy.insert(slug.to_owned(), 0.0);
        self.catalog_card_update(slug);
        let slug = slug.to_owned();
        std::thread::spawn(move || {
            let mut progress = progress_poster(slug.clone());
            let r = site::download_file(&slug, &mut progress);
            post(move |a| a.on_download(slug, r));
        });
    }

    fn on_download(&mut self, slug: String, r: anyhow::Result<site::SiteDownload>) {
        self.cat.busy.remove(&slug);
        let title = self.cat.mods.iter().find(|m| m.slug == slug).map(|m| m.title.clone()).unwrap_or_else(|| slug.clone());
        match r {
            Ok(d) => {
                let name = d.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let text = format!(
                    "{name}{}",
                    self.t(" проверен (SHA-256) и лежит в «Загрузки › Modly». Кнопка «Показать файл» откроет папку.",
                           " is verified (SHA-256) and saved to Downloads › Modly. “Show file” opens the folder.")
                );
                self.cat.downloads.insert(slug.clone(), d);
                self.toast(Tone::Success, &title, &text);
            }
            Err(e) => {
                log::warn!("site download {slug}: {e:#}");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
        }
        self.catalog_card_update(&slug);
    }

    /// «Показать файл»: скачанный файл в Проводнике.
    pub(super) fn catalog_reveal(&mut self, slug: &str) {
        match self.cat.downloads.get(slug).filter(|d| d.path.is_file()) {
            #[cfg(windows)]
            Some(d) => modly_core::win::reveal(&d.path),
            #[cfg(not(windows))]
            Some(_) => {}
            None => {
                // файл удалили — карточка снова предложит скачать
                self.cat.downloads.remove(slug);
                self.catalog_card_update(slug);
            }
        }
    }

    fn on_package(&mut self, slug: String, r: anyhow::Result<SitePackage>) {
        self.cat.busy.remove(&slug);
        let title = self.cat.mods.iter().find(|m| m.slug == slug).map(|m| m.title.clone()).unwrap_or_else(|| slug.clone());
        match r {
            Err(e) => {
                log::warn!("site install {slug}: {e:#}");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
            Ok(pkg) => {
                self.cat.packages.insert(slug.clone(), pkg.clone());
                self.reload_mods(false);
                match self.find_mod(&pkg.id) {
                    Some(m) if !m.dir.starts_with(modly_core::paths::user_mods_dir()) => {
                        // Тот же мод уже лежит в другой папке модов (например, у разработчика) — он главнее.
                        let text = format!(
                            "{}{}",
                            self.t("Пакет скачан, но Modly использует мод из папки ", "Package downloaded, but Modly uses the mod from "),
                            m.dir.display()
                        );
                        self.toast(Tone::Warning, &title, &text);
                    }
                    Some(m) => {
                        let games: Vec<Game> =
                            self.library.games.iter().filter(|g| g.has_def(&m.game.id) && g.exists()).cloned().collect();
                        if games.is_empty() {
                            let text = format!(
                                "{}{}{}",
                                self.t("Пакет скачан и проверен. Добавьте игру «", "Package downloaded and verified. Add “"),
                                m.game.name,
                                self.t("» в библиотеку — мод появится на её странице.", "” to your library — the mod shows up on its page.")
                            );
                            self.toast(Tone::Info, &title, &text);
                        } else {
                            // Пакет на месте — ставим во все копии игры, как кнопка «Установить во все копии».
                            self.run_install(m, games, true);
                        }
                    }
                    None => {
                        let text = self.t("Пакет скачан, но Modly его не видит — загляните в «Моды».", "Package downloaded, but Modly can't see it — check Mods.");
                        self.toast(Tone::Warning, &title, &text);
                    }
                }
            }
        }
        self.catalog_cards();
    }

    pub(super) fn catalog_open(&mut self, slug: &str) {
        let base = site::site_url();
        let url = if slug.is_empty() {
            format!("{base}/studio.php")
        } else if !site::slug_ok(slug) {
            return;
        } else {
            // Свою страницу (у Creatly — /creatly/) берём из каталога, только если она на этом же сайте.
            match self.cat.mods.iter().find(|m| m.slug == slug) {
                Some(m) if m.url.starts_with(&format!("{base}/")) => m.url.clone(),
                _ => format!("{base}/m/{slug}"),
            }
        };
        modly_core::win::shell_open(&url);
    }

    /// «К игре»: страница игры, для которой мод, или «Моды», если игры в библиотеке нет.
    pub(super) fn catalog_goto(&mut self, slug: &str) {
        let game = self
            .cat
            .packages
            .get(slug)
            .and_then(|p| self.find_mod(&p.id))
            .and_then(|m| self.library.games.iter().find(|g| g.has_def(&m.game.id)).map(|g| g.id.clone()));
        match game {
            Some(id) => self.open_game(&id),
            None => self.navigate("mods"),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Лайки и вход

    pub(super) fn catalog_like(&mut self, slug: &str) {
        let Some(token) = self.cat.account.as_ref().map(|a| a.token.clone()) else {
            let title = self.t("Нужен вход", "Sign in first");
            let text = self.t("Лайки ставятся от имени аккаунта Hackerstone — нажмите «Войти» вверху.", "Likes come from your Hackerstone account — press Sign in at the top.");
            self.toast(Tone::Info, &title, &text);
            return;
        };
        if !site::slug_ok(slug) || !self.cat.liking.insert(slug.to_owned()) {
            return;
        }
        let slug = slug.to_owned();
        std::thread::spawn(move || {
            let r = site::like(&slug, &token);
            post(move |a| {
                a.cat.liking.remove(&slug);
                match r {
                    Ok((liked, likes)) => {
                        if let Some(m) = a.cat.mods.iter_mut().find(|m| m.slug == slug) {
                            m.liked = liked;
                            m.likes = likes;
                        }
                        a.catalog_card_update(&slug);
                    }
                    Err(e) if matches!(e.downcast_ref::<SiteError>(), Some(SiteError::NeedLogin)) => {
                        site::forget_account();
                        a.cat.account = None;
                        a.account_to_ui();
                        let t = a.t("Вход закончился — войдите снова.", "Your sign-in expired — sign in again.");
                        a.toast(Tone::Warning, "", &t);
                    }
                    Err(e) => a.toast(Tone::Danger, "", &format!("{e:#}")),
                }
            });
        });
    }

    pub(super) fn account_login(&mut self) {
        if self.cat.login_cancel.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cat.login_cancel = Some(cancel.clone());
        self.account_to_ui();
        std::thread::spawn(move || {
            let open = |url: &str| {
                // Только в отладочной сборке: адрес входа — в файл, чтобы проверять вход автоматически.
                #[cfg(debug_assertions)]
                if let Some(f) = std::env::var_os("MODLY_TEST_OPEN") {
                    let _ = std::fs::write(f, url);
                    return;
                }
                modly_core::win::shell_open(url);
            };
            let r = site::login(&site::device_name(), &cancel, &open);
            post(move |a| a.on_login(r));
        });
    }

    pub(super) fn account_cancel(&mut self) {
        if let Some(c) = self.cat.login_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.account_to_ui();
    }

    fn on_login(&mut self, r: anyhow::Result<Account>) {
        self.cat.login_cancel = None;
        match r {
            Ok(acc) => {
                let text = format!("{}{}", self.t("Вы вошли как ", "Signed in as "), acc.user.nick);
                self.cat.account = Some(acc);
                self.toast(Tone::Success, "Hackerstone", &text);
                #[cfg(windows)]
                modly_core::win::focus_window("Modly");
                // Свежие «мои лайки»
                self.cat.loaded = false;
                if self.ui().is_some_and(|ui| ui.global::<Store>().get_page() == "catalog") {
                    self.catalog_refresh();
                }
            }
            Err(e) if e.downcast_ref::<SiteError>().is_some() => {}
            Err(e) => {
                let title = self.t("Не получилось войти", "Sign-in failed");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
        }
        self.account_to_ui();
    }

    pub(super) fn account_logout(&mut self) {
        let Some(acc) = self.cat.account.take() else { return };
        // Забываем сразу, а токен на сайте отзываем в фоне.
        site::forget_account();
        std::thread::spawn(move || site::logout(&acc));
        for m in &mut self.cat.mods {
            m.liked = false;
        }
        self.account_to_ui();
        self.catalog_cards();
        let text = self.t("Вход в Modly закрыт.", "Signed out of Modly.");
        self.toast(Tone::Info, "Hackerstone", &text);
    }

    // ---------------------------------------------------------------------------------------
    // Ссылки modly://

    /// `modly://install/<адрес>` — показать мод и спросить, ставить ли.
    pub(super) fn open_url(&mut self, url: &str) {
        let lower = url.trim().to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("modly://install/") else {
            log::info!("unknown modly link: {url}");
            return;
        };
        let slug = rest.trim_end_matches('/').to_owned();
        if !site::slug_ok(&slug) {
            return;
        }
        #[cfg(windows)]
        modly_core::win::focus_window("Modly");
        std::thread::spawn(move || {
            let r = site::details(&slug, None);
            post(move |a| match r {
                Ok(d) => {
                    let mut m = d.brief.clone();
                    // в карточке подтверждения — версия и пакет именно свежего одобренного выпуска
                    m.latest = d.package().map(|(rel, _)| site::Latest { version: rel.version.clone(), modly: true, download: true });
                    if m.adult && !a.settings.show_adult {
                        a.age_ask(Some(m)); // мод 18+, а показ выключен — сначала спросить возраст
                    } else {
                        a.ask_install(m);
                    }
                }
                Err(e) => {
                    let title = a.t("Мод не найден", "Mod not found");
                    a.toast(Tone::Danger, &title, &format!("{e:#}"));
                }
            });
        });
    }

    fn ask_install(&mut self, m: SiteMod) {
        let card = self.site_card(&m);
        if !m.logo.is_empty() {
            self.art_want((Kind::Logo, m.logo.clone()));
        }
        self.cat.confirm = Some(m);
        if let Some(ui) = self.ui() {
            let store = ui.global::<Store>();
            store.set_confirm_mod(card);
            store.set_confirm_open(true);
        }
        self.art_pump();
    }

    pub(super) fn confirm_install(&mut self) {
        let Some(m) = self.cat.confirm.take() else { return };
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_confirm_open(false);
        }
        if !self.cat.mods.iter().any(|x| x.slug == m.slug) {
            self.cat.mods.push(m.clone());
        }
        self.navigate("catalog");
        self.catalog_install(&m.slug);
        self.catalog_cards();
    }

    pub(super) fn confirm_cancel(&mut self) {
        self.cat.confirm = None;
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_confirm_open(false);
        }
    }

    // ---------------------------------------------------------------------------------------
    // Моды 18+

    pub(super) fn adult_to_ui(&self) {
        if let Some(ui) = self.ui() {
            let store = ui.global::<Store>();
            store.set_show_adult(self.settings.show_adult);
            store.set_catalog_adult_hidden(if self.settings.show_adult { 0 } else { self.cat.adult_hidden.min(i32::MAX as usize) as i32 });
        }
    }

    /// Показ модов 18+ в настройках или кнопкой «18+» в каталоге. Включить — только через «Вам есть 18?».
    pub(super) fn set_show_adult(&mut self, on: bool) {
        if on == self.settings.show_adult {
            return;
        }
        if on {
            self.age_ask(None);
            return;
        }
        self.settings.show_adult = false;
        self.settings.save();
        self.adult_to_ui();
        self.catalog_refresh();
    }

    /// Окно «Вам есть 18?». `then` — мод 18+ из ссылки modly://: после «Да» спросить, ставить ли его.
    fn age_ask(&mut self, then: Option<SiteMod>) {
        let card = then.as_ref().map(|m| self.site_card(m));
        if let Some(m) = then.as_ref().filter(|m| !m.logo.is_empty()) {
            self.art_want((Kind::Logo, m.logo.clone()));
        }
        self.cat.age_then = then;
        if let Some(ui) = self.ui() {
            let store = ui.global::<Store>();
            store.set_age_for_mod(card.is_some());
            store.set_age_mod(card.unwrap_or_default());
            store.set_age_open(true);
        }
        self.art_pump();
    }

    pub(super) fn age_confirm(&mut self) {
        self.settings.show_adult = true;
        self.settings.save();
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_age_open(false);
        }
        self.adult_to_ui();
        self.catalog_refresh();
        if let Some(m) = self.cat.age_then.take() {
            self.ask_install(m);
        }
    }

    pub(super) fn age_cancel(&mut self) {
        self.cat.age_then = None;
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_age_open(false);
        }
    }
}

/// Мод `m` — для игры `g`: по номеру в Steam, по описанию игры из пакета (`game.id` — с тем, как
/// его понимают моды на этом ПК), по названию или по exe (кроме общих вроде `Game.exe`).
fn game_fits(g: &Game, m: &SiteMod) -> bool {
    const GENERIC_EXE: &[&str] = &["game.exe", "launcher.exe", "start.exe", "play.exe", "client.exe"];
    let info = &m.game_info;
    let key = site::game_key(&m.game);
    let exe = info.exe.to_lowercase();
    (info.steam_appid.is_some() && info.steam_appid == g.steam_appid)
        || (!info.id.is_empty() && g.has_def(&info.id))
        || (!key.is_empty() && site::game_key(&g.name) == key)
        || (!exe.is_empty() && !GENERIC_EXE.contains(&exe.as_str()) && g.exe.to_lowercase() == exe)
}

/// Прогресс скачивания из фонового потока — в карточку, не чаще шага в 1 %.
fn progress_poster(slug: String) -> impl FnMut(f32) {
    let mut last = -1.0f32;
    move |p: f32| {
        if p - last >= 0.01 || p >= 1.0 {
            last = p;
            let s = slug.clone();
            post(move |a| {
                if let Some(v) = a.cat.busy.get_mut(&s) {
                    *v = p;
                    a.catalog_card_update(&s);
                }
            });
        }
    }
}

/// Подвязать кнопки каталога (вызывается из `wire_callbacks`).
pub(super) fn wire(s: &Store) {
    s.on_catalog_refresh(|| with(|a| a.catalog_refresh()));
    s.on_catalog_search_changed(|t| with(move |a| a.catalog_search(t.to_string())));
    s.on_catalog_install(|slug| with(move |a| a.catalog_install(&slug)));
    s.on_catalog_download(|slug| with(move |a| a.catalog_download(&slug)));
    s.on_catalog_reveal(|slug| with(move |a| a.catalog_reveal(&slug)));
    s.on_catalog_open(|slug| with(move |a| a.catalog_open(&slug)));
    s.on_catalog_goto(|slug| with(move |a| a.catalog_goto(&slug)));
    s.on_catalog_like(|slug| with(move |a| a.catalog_like(&slug)));
    s.on_catalog_art(|slug, near| with(move |a| a.catalog_art(&slug, near)));
    s.on_account_login(|| with(|a| a.account_login()));
    s.on_account_cancel(|| with(|a| a.account_cancel()));
    s.on_account_logout(|| with(|a| a.account_logout()));
    s.on_confirm_install(|| with(|a| a.confirm_install()));
    s.on_confirm_cancel(|| with(|a| a.confirm_cancel()));
    s.on_set_show_adult(|v| with(move |a| a.set_show_adult(v)));
    s.on_age_confirm(|| with(|a| a.age_confirm()));
    s.on_age_cancel(|| with(|a| a.age_cancel()));
}
