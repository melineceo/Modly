//! Карты модов: на странице игры, отдельной страницей и во вкладке «Карта» оверлея.
//!
//! Карту делает автор мода — он знает, как устроен мир его игры: присылает её по протоколу
//! (`map`, `map_patch` — живые метки, действия вроде «Телепорт сюда») или кладёт в пакет
//! (`modly.json` → `maps`) — такая карта видна и без запущенной игры. Если мод подключён и прислал
//! карту с тем же id, что в пакете, показывается его — живая.
//!
//! Картинки читаются и декодируются в фоновых потоках, на поток интерфейса приходят готовые
//! пиксели. Карта в памяти — не больше 2560 px по стороне (~17 МБ). Живые метки (игрок, питомцы)
//! обновляются по одной строке модели — карту при этом не перестраиваем.

use super::{App, post, with};
use crate::thumbs::{self, Kind};
use crate::{MapActionItem, MapCard, MapCat, MapPin, MapRow, Store};
use modly_core::hub::SessionId;
use modly_core::library::Game;
use modly_core::maps as core_maps;
use modly_protocol::{GameMap, MapAction, MapBounds, MapMarker, MarkerStyle, Tone};
use slint::{Color, ComponentHandle, Image, Model, ModelRc, VecModel};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

const MAP_SIDE: u32 = 2560;

/// Откуда карта.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// Из пакета мода: папка пакета (картинка — путь внутри неё).
    Package(PathBuf),
    /// От подключённого мода.
    Live(SessionId),
    /// Мод отключился, а в пакете такой карты нет: показываем последнее, без действий.
    Offline,
}

#[derive(Clone)]
struct Entry {
    /// «мод/карта» — один и тот же у карты из пакета и у живой карты того же мода.
    key: String,
    mod_name: String,
    map: GameMap,
    source: Source,
}

/// Открытая карта.
struct OpenMap {
    key: String,
    source: Source,
    mod_name: String,
    map: GameMap,
    bounds: MapBounds,
    /// Какая картинка показана — чтобы не грузить её заново при каждом обновлении карты.
    image: String,
    /// id метки → строка в `map-pins`.
    index: HashMap<String, usize>,
    /// Строка в `map-pins` → номер метки в `map.markers` (порядок рисования, см. [`draw_order`]).
    order: Vec<usize>,
}

impl OpenMap {
    /// Метка строки `map-pins`.
    fn marker(&self, pin: usize) -> Option<&MapMarker> {
        self.order.get(pin).and_then(|&k| self.map.markers.get(k))
    }
}

/// Порядок рисования меток: точки под булавками, метка `focus` (обычно игрок) — поверх всех,
/// иначе её закроют метки, присланные позже (`map_patch` дописывает новые в конец).
/// Внутри — как прислал мод.
fn draw_order(map: &GameMap) -> Vec<usize> {
    let mut order: Vec<usize> = (0..map.markers.len()).collect();
    order.sort_by_key(|&k| {
        let m = &map.markers[k];
        (map.focus.as_deref() == Some(m.id.as_str()), m.style != MarkerStyle::Dot)
    });
    order
}

pub struct MapsState {
    /// Карты открытой в Modly игры.
    entries: Vec<Entry>,
    /// Карты пакетов (чтобы не читать JSON при каждой перерисовке страницы игры).
    package_cache: HashMap<PathBuf, Vec<GameMap>>,
    /// Миниатюры: «картинка» → изображение (`None` — загружается или не вышло).
    thumbs: HashMap<String, Option<Image>>,
    open: Option<OpenMap>,
    /// Карта, картинка которой сейчас грузится.
    loading: String,
    hidden: HashSet<String>,
    query: String,
    /// Выбранная метка (id): номер строки меняется, когда метки до неё исчезают.
    selected: Option<String>,
    /// Куда вернуться со страницы карты.
    back: String,
    pub(super) m_cards: Rc<VecModel<MapCard>>,
    pub(super) m_pins: Rc<VecModel<MapPin>>,
    pub(super) m_cats: Rc<VecModel<MapCat>>,
    pub(super) m_rows: Rc<VecModel<MapRow>>,
    pub(super) m_actions: Rc<VecModel<MapActionItem>>,
    pub(super) m_click: Rc<VecModel<MapActionItem>>,
}

impl Default for MapsState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            package_cache: HashMap::new(),
            thumbs: HashMap::new(),
            open: None,
            loading: String::new(),
            hidden: HashSet::new(),
            query: String::new(),
            selected: None,
            back: String::new(),
            m_cards: Rc::new(VecModel::default()),
            m_pins: Rc::new(VecModel::default()),
            m_cats: Rc::new(VecModel::default()),
            m_rows: Rc::new(VecModel::default()),
            m_actions: Rc::new(VecModel::default()),
            m_click: Rc::new(VecModel::default()),
        }
    }
}

impl MapsState {
    /// Пакеты перечитаны — карты из них тоже.
    pub(super) fn forget_packages(&mut self) {
        self.package_cache.clear();
    }
}

/// Модели карт — в `Store` окна (главного и оверлея: у каждого окна свой `Store`, модели общие).
pub(super) fn attach(s: &Store, m: &MapsState) {
    s.set_game_maps(ModelRc::from(m.m_cards.clone()));
    s.set_map_pins(ModelRc::from(m.m_pins.clone()));
    s.set_map_cats(ModelRc::from(m.m_cats.clone()));
    s.set_map_rows(ModelRc::from(m.m_rows.clone()));
    s.set_map_actions(ModelRc::from(m.m_actions.clone()));
    s.set_map_click_actions(ModelRc::from(m.m_click.clone()));
}

pub(super) fn wire(s: &Store) {
    s.on_open_map(|key| with(move |a| a.open_map(&key, false)));
    s.on_close_map(|| with(|a| a.close_map()));
    s.on_map_toggle_cat(|id| with(move |a| a.map_toggle_cat(&id)));
    s.on_map_search(|q| with(move |a| a.map_search(&q)));
    s.on_map_select(|i, _from_list| with(move |a| a.map_select(i)));
    s.on_map_action(|id| with(move |a| a.map_action(&id)));
    s.on_map_click_action(|id, u, v| with(move |a| a.map_click_action(&id, u as f64, v as f64)));
}

fn color(hex: Option<&str>) -> Option<Color> {
    hex.and_then(crate::icons::parse_hex)
}

const VIOLET: Color = Color::from_rgb_u8(0x8b, 0x5c, 0xf6);

fn icon(name: Option<&str>) -> Image {
    crate::icons::named_opt(Some(name.filter(|n| !n.is_empty()).unwrap_or("map-pin"))).0
}

/// Картинка карты — по источнику: из пакета или от мода (путь на диске / data:-адрес).
fn read_image(source: &Source, image: &str) -> anyhow::Result<Vec<u8>> {
    match source {
        Source::Package(dir) => core_maps::package_image(dir, image),
        Source::Live(_) | Source::Offline => core_maps::mod_image(image),
    }
}

fn action_item(a: &MapAction, lang: &str) -> MapActionItem {
    let label = a.label.resolve(lang);
    MapActionItem {
        id: a.id.as_str().into(),
        label: if label.is_empty() { a.id.as_str().into() } else { label.into() },
        kind: match a.tone {
            Tone::Accent => "primary",
            Tone::Danger => "danger",
            Tone::Success => "success",
            _ => "secondary",
        }
        .into(),
        confirm: a.confirm.as_ref().map(|c| c.resolve(lang)).unwrap_or("").into(),
    }
}

impl App {
    // -----------------------------------------------------------------------------------------
    // Карты игры на её странице

    /// Карты открытой игры: из пакетов её модов и от подключённых модов (живые — главнее).
    pub(super) fn refresh_game_maps(&mut self, game: &Game) {
        let mut entries: Vec<Entry> = Vec::new();
        for m in self.mods_for(game) {
            let maps = self.maps.package_cache.entry(m.dir.clone()).or_insert_with(|| core_maps::package_maps(&m)).clone();
            for map in maps {
                entries.push(Entry { key: format!("{}/{}", m.id, map.id), mod_name: m.name.clone(), map, source: Source::Package(m.dir.clone()) });
            }
        }
        for s in self.sessions.values().filter(|s| s.game_id.as_deref() == Some(game.id.as_str())) {
            for map in &s.maps {
                let e = Entry {
                    key: format!("{}/{}", s.hello.mod_info.id, map.id),
                    mod_name: s.name().to_owned(),
                    map: map.clone(),
                    source: Source::Live(s.id),
                };
                match entries.iter_mut().find(|x| x.key == e.key) {
                    Some(x) => *x = e,
                    None => entries.push(e),
                }
            }
        }
        self.maps.entries = entries;
        self.game_maps_cards();
        self.sync_open_map();
    }

    /// Страница игры ещё не открыта — но карты могут смениться (мод прислал новую).
    fn refresh_current_maps(&mut self) {
        if let Some(game) = self.current() {
            self.refresh_game_maps(&game);
        }
    }

    fn game_maps_cards(&mut self) {
        let lang = self.lang();
        let mut want = Vec::new();
        let cards: Vec<MapCard> = self
            .maps
            .entries
            .iter()
            .map(|e| {
                let thumb_key = format!("{:?}|{}", e.source_dir(), e.map.image);
                let thumb = match self.maps.thumbs.get(&thumb_key) {
                    Some(img) => img.clone(),
                    None => {
                        want.push((thumb_key, e.source.clone(), e.map.image.clone()));
                        None
                    }
                };
                let title = e.map.title.resolve(lang);
                MapCard {
                    key: e.key.as_str().into(),
                    title: if title.is_empty() { e.map.id.as_str().into() } else { title.into() },
                    author: e.mod_name.as_str().into(),
                    markers: e.map.markers.len() as i32,
                    has_thumb: thumb.is_some(),
                    thumb: thumb.unwrap_or_default(),
                    live: matches!(e.source, Source::Live(_)),
                }
            })
            .collect();
        self.maps.m_cards.set_vec(cards);
        let scale = self.ui().map(|ui| ui.window().scale_factor()).unwrap_or(1.0);
        for (key, source, image) in want {
            self.maps.thumbs.insert(key.clone(), None);
            std::thread::spawn(move || {
                let px = read_image(&source, &image)
                    .and_then(|d| thumbs::decode(&d, Kind::MapThumb, Kind::MapThumb.size(scale)))
                    .map_err(|e| log::info!("map thumb {image}: {e:#}"))
                    .ok();
                post(move |a| {
                    a.maps.thumbs.insert(key, px.map(|p| p.to_image()));
                    a.game_maps_cards();
                });
            });
        }
    }

    // -----------------------------------------------------------------------------------------
    // Открытая карта

    /// Открыть карту: на странице "map" главного окна или (`in_overlay`) во вкладке «Карта» оверлея —
    /// тогда главное окно остаётся, где было.
    pub(super) fn open_map(&mut self, key: &str, in_overlay: bool) {
        let Some(entry) = self.maps.entries.iter().find(|e| e.key == key).cloned() else { return };
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        let page = s.get_page().to_string();
        if page != "map" && !in_overlay {
            self.maps.back = page;
        }
        s.set_map_key(key.into());
        s.set_map_selected(-1);
        self.maps.selected = None;
        if in_overlay {
            self.overlay_show_map();
        } else {
            self.navigate("map");
        }
        // та же карта с той же картинкой уже загружена — только обновить
        if self.maps.open.as_ref().is_some_and(|o| o.key == key && o.image == image_id(&entry)) {
            self.adopt_entry(entry);
            s.set_map_state("ready".into());
            return;
        }
        let lang = self.lang();
        s.set_map_title(entry.map.title.resolve(lang).into());
        s.set_map_subtitle(entry.mod_name.as_str().into());
        s.set_map_state("loading".into());
        self.load_map_image(entry);
    }

    fn load_map_image(&mut self, entry: Entry) {
        self.maps.loading = entry.key.clone();
        std::thread::spawn(move || {
            let r = read_image(&entry.source, &entry.map.image).and_then(|data| thumbs::decode_full_sized(&data, MAP_SIDE));
            post(move |a| a.on_map_loaded(entry, r));
        });
    }

    fn on_map_loaded(&mut self, entry: Entry, r: anyhow::Result<(thumbs::Pixels, (u32, u32))>) {
        if self.maps.loading != entry.key {
            return; // пока грузилась, открыли другую
        }
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        match r {
            Ok((px, (w, h))) => {
                s.set_map_image(px.to_image());
                s.set_map_ratio(px.width as f32 / px.height.max(1) as f32);
                let fresh = self.maps.open.as_ref().is_none_or(|o| o.key != entry.key);
                if fresh {
                    self.maps.hidden = entry.map.categories.iter().filter(|c| c.hidden).map(|c| c.id.clone()).collect();
                    self.maps.query.clear();
                    s.set_map_query("".into());
                }
                let bounds = entry.map.bounds.filter(MapBounds::is_valid).unwrap_or(MapBounds::pixels(w as f64, h as f64));
                self.maps.open = Some(OpenMap {
                    key: entry.key.clone(),
                    source: entry.source.clone(),
                    mod_name: entry.mod_name.clone(),
                    image: image_id(&entry),
                    map: entry.map,
                    bounds,
                    index: HashMap::new(),
                    order: Vec::new(),
                });
                self.map_render();
                s.set_map_state("ready".into());
            }
            Err(e) => {
                log::warn!("map {}: {e:#}", entry.key);
                self.maps.open = None;
                s.set_map_state("error".into());
                let text = format!("{}: {e:#}", self.t("Карту не удалось открыть", "Couldn't open the map"));
                s.set_map_error(text.into());
            }
        }
    }

    /// Карта открыта, а её описание сменилось (мод прислал новое, мод отключился, пакет обновился).
    fn sync_open_map(&mut self) {
        let Some(open) = &self.maps.open else { return };
        match self.maps.entries.iter().find(|e| e.key == open.key).cloned() {
            Some(e) if image_id(&e) != open.image => self.load_map_image(e),
            Some(e) => {
                if e.source != open.source || e.map != open.map {
                    self.adopt_entry(e);
                }
            }
            // мод отключился, а в пакете такой карты нет — оставляем последнее, но без действий
            None => {
                if let Some(o) = &mut self.maps.open {
                    o.source = Source::Offline;
                }
                self.map_render();
            }
        }
    }

    /// Взять новое описание открытой карты (картинка та же).
    fn adopt_entry(&mut self, e: Entry) {
        if let Some(o) = &mut self.maps.open {
            if let Some(b) = e.map.bounds.filter(MapBounds::is_valid) {
                o.bounds = b;
            }
            o.source = e.source;
            o.mod_name = e.mod_name;
            o.map = e.map;
        }
        self.map_render();
    }

    fn pin(&self, m: &MapMarker, lang: &str) -> MapPin {
        let Some(open) = &self.maps.open else { return MapPin::default() };
        let cat = open.map.categories.iter().find(|c| !m.cat.is_empty() && c.id == m.cat);
        let (u, v) = open.bounds.to_unit(m.x, m.y);
        let inside = (-0.001..=1.001).contains(&u) && (-0.001..=1.001).contains(&v);
        let title = m.title.resolve(lang);
        let q = self.maps.query.to_lowercase();
        let found = q.is_empty()
            || title.to_lowercase().contains(&q)
            || m.text.as_ref().is_some_and(|t| t.resolve(lang).to_lowercase().contains(&q));
        MapPin {
            x: u as f32,
            y: v as f32,
            title: if title.is_empty() { m.id.as_str().into() } else { title.into() },
            text: m.text.as_ref().map(|t| t.resolve(lang)).unwrap_or("").into(),
            cat_label: cat.map(|c| c.label.resolve(lang)).unwrap_or("").into(),
            color: color(m.color.as_deref()).or_else(|| color(cat.and_then(|c| c.color.as_deref()))).unwrap_or(VIOLET),
            icon: icon(m.icon.as_deref().or(cat.and_then(|c| c.icon.as_deref()))),
            shown: inside && found && !self.maps.hidden.contains(&m.cat),
            dot: m.style == MarkerStyle::Dot,
            has_actions: !m.actions.is_empty(),
        }
    }

    /// Всё о открытой карте: заголовок, категории, метки, список, действия (и после смены языка).
    pub(super) fn map_render(&mut self) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        let lang = self.lang();
        let Some(open) = &self.maps.open else { return };
        let live = matches!(open.source, Source::Live(_));
        let marks = if lang == "ru" { "меток" } else { "markers" };
        s.set_map_title(open.map.title.resolve(lang).into());
        s.set_map_subtitle(format!("{} · {} {marks}", open.mod_name, open.map.markers.len()).into());
        s.set_map_live(live);

        let mut counts: HashMap<&str, i32> = HashMap::new();
        for m in &open.map.markers {
            *counts.entry(m.cat.as_str()).or_default() += 1;
        }
        let cats: Vec<MapCat> = open
            .map
            .categories
            .iter()
            .filter(|c| counts.contains_key(c.id.as_str()))
            .map(|c| MapCat {
                id: c.id.as_str().into(),
                label: c.label.resolve(lang).into(),
                color: color(c.color.as_deref()).unwrap_or(VIOLET),
                icon: icon(c.icon.as_deref()),
                count: counts[c.id.as_str()],
                on: !self.maps.hidden.contains(&c.id),
            })
            .collect();
        self.maps.m_cats.set_vec(cats);

        let draw = draw_order(&open.map);
        let pins: Vec<MapPin> = draw.iter().map(|&k| self.pin(&open.map.markers[k], lang)).collect();
        let index: HashMap<String, usize> = draw.iter().enumerate().map(|(i, &k)| (open.map.markers[k].id.clone(), i)).collect();
        // список — по порядку категорий, внутри — по названию; живые точки (игрок) — сверху
        let order: HashMap<&str, usize> = open.map.categories.iter().enumerate().map(|(i, c)| (c.id.as_str(), i)).collect();
        let mut rows: Vec<(bool, usize, MapRow)> = pins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.shown)
            .map(|(i, p)| {
                let m = &open.map.markers[draw[i]];
                let row = MapRow { index: i as i32, title: p.title.clone(), cat_label: p.cat_label.clone(), color: p.color, icon: p.icon.clone() };
                (!p.dot, order.get(m.cat.as_str()).copied().unwrap_or(usize::MAX), row)
            })
            .collect();
        rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| a.2.title.to_lowercase().cmp(&b.2.title.to_lowercase())));
        rows.truncate(500);
        let focus = open.map.focus.as_ref().and_then(|f| index.get(f)).map(|&i| i as i32).unwrap_or(-1);
        let click: Vec<MapActionItem> = if live { open.map.click_actions.iter().map(|a| action_item(a, lang)).collect() } else { Vec::new() };

        // выбор держится за id метки: метки до неё могли исчезнуть, и номер строки сменился;
        // сама метка скрылась или исчезла — снять выбор
        let sel = self.maps.selected.as_ref().and_then(|id| index.get(id)).copied().filter(|&i| pins[i].shown);
        if sel.is_none() {
            self.maps.selected = None;
        }
        s.set_map_selected(sel.map(|i| i as i32).unwrap_or(-1));
        self.maps.m_pins.set_vec(pins);
        self.maps.m_rows.set_vec(rows.into_iter().map(|r| r.2).collect::<Vec<_>>());
        self.maps.m_click.set_vec(click);
        s.set_map_focus_index(focus);
        if let Some(o) = &mut self.maps.open {
            o.index = index;
            o.order = draw;
        }
        self.map_selected_actions();
    }

    /// Действия выбранной метки (только пока мод подключён).
    fn map_selected_actions(&mut self) {
        let Some(ui) = self.ui() else { return };
        let lang = self.lang();
        let sel = ui.global::<Store>().get_map_selected();
        let items: Vec<MapActionItem> = match &self.maps.open {
            Some(o) if matches!(o.source, Source::Live(_)) && sel >= 0 => {
                o.marker(sel as usize).map(|m| m.actions.iter().map(|a| action_item(a, lang)).collect()).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        self.maps.m_actions.set_vec(items);
    }

    // -----------------------------------------------------------------------------------------
    // Сообщения мода

    /// Мод прислал карту целиком.
    pub(super) fn on_mod_map(&mut self, sid: SessionId, map: GameMap) {
        if let Some(s) = self.sessions.get_mut(&sid) {
            s.put_map(map);
        }
        self.refresh_current_maps();
    }

    /// Мод передвинул, добавил или убрал метки. Если двигаются только точки — обновляем их строки.
    pub(super) fn on_mod_map_patch(&mut self, sid: SessionId, map: &str, set: Vec<MapMarker>, remove: Vec<String>) {
        let Some(s) = self.sessions.get_mut(&sid) else { return };
        if !s.patch_map(map, &set, &remove) {
            return;
        }
        let key = format!("{}/{}", s.hello.mod_info.id, map);
        let count_changed = !remove.is_empty();
        // карта не открыта — обновить только список карт игры (число меток)
        let open_here = self.maps.open.as_ref().is_some_and(|o| o.key == key && o.source == Source::Live(sid));
        if !open_here {
            if let Some(e) = self.maps.entries.iter_mut().find(|e| e.key == key && e.source == Source::Live(sid)) {
                if let Some(m) = self.sessions.get(&sid).and_then(|s| s.maps.iter().find(|m| m.id == map)) {
                    let before = e.map.markers.len();
                    e.map = m.clone();
                    if before != e.map.markers.len() {
                        self.game_maps_cards();
                    }
                }
            }
            return;
        }
        let mut full = count_changed;
        let mut changed = Vec::new();
        if let Some(o) = &mut self.maps.open {
            for m in set {
                match o.index.get(&m.id) {
                    Some(&i) => {
                        let k = o.order[i];
                        let old = &o.map.markers[k];
                        // сменилось не только место — список и фильтры надо перестроить
                        if old.title != m.title || old.cat != m.cat || old.text != m.text || old.icon != m.icon || old.color != m.color
                            || old.style != m.style || old.actions != m.actions
                        {
                            full = true;
                        }
                        o.map.markers[k] = m;
                        changed.push(i);
                    }
                    None => {
                        o.map.markers.push(m);
                        full = true;
                    }
                }
            }
            if !remove.is_empty() {
                o.map.markers.retain(|m| !remove.contains(&m.id));
            }
        }
        if let Some(e) = self.maps.entries.iter_mut().find(|e| e.key == key) {
            if let Some(o) = &self.maps.open {
                e.map = o.map.clone();
            }
        }
        if full {
            self.map_render();
            self.game_maps_cards();
            return;
        }
        let lang = self.lang();
        for i in changed {
            let Some(m) = self.maps.open.as_ref().and_then(|o| o.marker(i)).cloned() else { continue };
            let pin = self.pin(&m, lang);
            self.maps.m_pins.set_row_data(i, pin);
        }
    }

    /// Мод убрал карту.
    pub(super) fn on_mod_map_remove(&mut self, sid: SessionId, map: &str) {
        if let Some(s) = self.sessions.get_mut(&sid) {
            s.maps.retain(|m| m.id != map);
        }
        self.refresh_current_maps();
    }

    // -----------------------------------------------------------------------------------------
    // Действия пользователя

    fn map_toggle_cat(&mut self, id: &str) {
        if !self.maps.hidden.remove(id) {
            self.maps.hidden.insert(id.to_owned());
        }
        self.map_render();
    }

    fn map_search(&mut self, q: &str) {
        self.maps.query = q.chars().take(60).collect();
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_map_query(self.maps.query.as_str().into());
        }
        self.map_render();
    }

    /// Выбрать метку (-1 — снять выбор). Подвести карту к метке из списка — дело самой карты
    /// (`MapViewer.focus-on`).
    fn map_select(&mut self, index: i32) {
        let Some(ui) = self.ui() else { return };
        let valid = index >= 0 && (index as usize) < self.maps.m_pins.row_count();
        ui.global::<Store>().set_map_selected(if valid { index } else { -1 });
        self.maps.selected = if valid { self.maps.open.as_ref().and_then(|o| o.marker(index as usize)).map(|m| m.id.clone()) } else { None };
        self.map_selected_actions();
    }

    /// Кнопка действия у выбранной метки → моду `map_action` с меткой и её местом.
    fn map_action(&mut self, action: &str) {
        let Some(ui) = self.ui() else { return };
        let sel = ui.global::<Store>().get_map_selected();
        let Some(open) = &self.maps.open else { return };
        let Source::Live(sid) = open.source else { return };
        let Some(m) = (sel >= 0).then(|| open.marker(sel as usize)).flatten() else { return };
        if !m.actions.iter().any(|a| a.id == action) {
            return;
        }
        log::info!("map action {action} on {}/{} → session #{sid}", open.map.id, m.id);
        self.hub.map_action(sid, &open.map.id, action, Some(&m.id), m.x, m.y);
    }

    /// Действие по клику в пустое место → моду `map_action` с точкой в координатах игры.
    fn map_click_action(&mut self, action: &str, u: f64, v: f64) {
        let Some(open) = &self.maps.open else { return };
        let Source::Live(sid) = open.source else { return };
        if !open.map.click_actions.iter().any(|a| a.id == action) || !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return;
        }
        let (x, y) = open.bounds.from_unit(u, v);
        log::info!("map click action {action} at ({x:.2}, {y:.2}) → session #{sid}");
        self.hub.map_action(sid, &open.map.id, action, None, x, y);
    }

    fn close_map(&mut self) {
        let back = match self.maps.back.as_str() {
            "" | "map" => "game".to_owned(),
            b => b.to_owned(),
        };
        self.navigate(&back);
    }
}

impl Entry {
    /// Папка пакета или пусто (для ключа миниатюры: картинка пакета и картинка мода — разные файлы).
    fn source_dir(&self) -> Option<&PathBuf> {
        match &self.source {
            Source::Package(d) => Some(d),
            Source::Live(_) | Source::Offline => None,
        }
    }
}

/// Что за картинка: путь пакета или адрес от мода.
fn image_id(e: &Entry) -> String {
    format!("{:?}|{}", e.source_dir(), e.map.image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn focus_is_drawn_last_and_dots_under_pins() {
        let map: GameMap = serde_json::from_value(json!({
            "id": "world", "image": "map.png", "focus": "player",
            "markers": [
                {"id": "home", "x": 0, "y": 0},
                {"id": "player", "x": 0, "y": 0, "style": "dot"},
                {"id": "mama", "x": 0, "y": 0, "style": "dot"},
                {"id": "lake", "x": 0, "y": 0},
                {"id": "pup", "x": 0, "y": 0, "style": "dot"},
            ],
        }))
        .unwrap();
        let ids: Vec<&str> = draw_order(&map).iter().map(|&k| map.markers[k].id.as_str()).collect();
        assert_eq!(ids, ["mama", "pup", "home", "lake", "player"]);
    }
}
