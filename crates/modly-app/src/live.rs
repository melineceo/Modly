//! Подключённые моды: их интерфейс и значения, и перевод в элементы Slint.

use crate::{LiveControl, LivePage, LiveSection, OptionItem, PresetItem};
use modly_core::hub::{Hello, SessionId};
use modly_protocol::{Control, ControlKind, ControlPatch, GameMap, Page, Text, Tone};
use serde_json::Value;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::rc::Rc;

pub struct Session {
    pub id: SessionId,
    pub hello: Hello,
    /// Запись библиотеки, к которой относится мод (по процессу игры).
    pub game_id: Option<String>,
    pub pages: Vec<Page>,
    pub values: HashMap<String, Value>,
    pub status: Option<(Tone, Text)>,
    /// Главный поток игры стоит (пауза без фокуса): команды выполнятся позже.
    pub paused: bool,
    /// Карты, которые прислал мод (с живыми метками), по порядку прихода.
    pub maps: Vec<GameMap>,
}

impl Session {
    pub fn new(id: SessionId, hello: Hello, game_id: Option<String>) -> Self {
        Session { id, hello, game_id, pages: Vec::new(), values: HashMap::new(), status: None, paused: false, maps: Vec::new() }
    }

    /// Карта пришла целиком: заменить карту с тем же id или добавить новую (не больше 16).
    pub fn put_map(&mut self, mut map: GameMap) {
        modly_core::maps::limit(&mut map);
        if let Some(i) = self.maps.iter().position(|m| m.id == map.id) {
            self.maps[i] = map;
        } else if self.maps.len() < modly_core::maps::MAX_MAPS {
            self.maps.push(map);
        } else {
            log::warn!("session #{}: больше {} карт — «{}» не принята", self.id, modly_core::maps::MAX_MAPS, map.id);
        }
    }

    /// Изменить метки карты. Возвращает false, если такой карты нет.
    pub fn patch_map(&mut self, id: &str, set: &[modly_protocol::MapMarker], remove: &[String]) -> bool {
        let Some(map) = self.maps.iter_mut().find(|m| m.id == id) else { return false };
        map.markers.retain(|m| !remove.contains(&m.id));
        for s in set {
            let mut s = s.clone();
            s.actions.truncate(modly_core::maps::MAX_ACTIONS);
            if let Some(i) = map.markers.iter().position(|m| m.id == s.id) {
                map.markers[i] = s;
            } else if map.markers.len() < modly_core::maps::MAX_MARKERS {
                map.markers.push(s);
            }
        }
        true
    }

    pub fn name(&self) -> &str {
        if self.hello.mod_info.name.is_empty() { &self.hello.mod_info.id } else { &self.hello.mod_info.name }
    }

    pub fn set_layout(&mut self, pages: Vec<Page>) {
        self.values.clear();
        for p in &pages {
            for s in &p.sections {
                for c in &s.controls {
                    if let Some(v) = &c.value {
                        self.values.insert(c.id.clone(), v.clone());
                    }
                }
            }
        }
        self.pages = pages;
    }

    pub fn control(&self, id: &str) -> Option<&Control> {
        self.pages.iter().flat_map(|p| &p.sections).flat_map(|s| &s.controls).find(|c| c.id == id)
    }

    /// Применить изменения свойств. Возвращает true, если поменялась видимость (нужна перестройка).
    pub fn patch(&mut self, id: &str, patch: &ControlPatch) -> bool {
        for p in &mut self.pages {
            for s in &mut p.sections {
                for c in &mut s.controls {
                    if c.id == id {
                        let was = c.visible;
                        c.apply(patch);
                        return was != c.visible;
                    }
                }
            }
        }
        false
    }
}

pub fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn display(v: Option<&Value>, lang: &str) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.as_f64().map(fmt_num).unwrap_or_default(),
        Some(Value::Bool(b)) => match (*b, lang) {
            (true, "ru") => "Да".into(),
            (false, "ru") => "Нет".into(),
            (true, _) => "Yes".into(),
            (false, _) => "No".into(),
        },
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn resolve(t: &Option<Text>, lang: &str) -> SharedString {
    t.as_ref().map(|t| t.resolve(lang)).unwrap_or("").into()
}

/// Значение для отправки моду: целые — целыми, дробные — без хвостов float.
pub fn number_value(v: f64, step: f64) -> Value {
    let v = if step > 0.0 { (v / step).round() * step } else { v };
    let v = (v * 1e6).round() / 1e6;
    if v.fract() == 0.0 && v.abs() < 9e15 { Value::from(v as i64) } else { Value::from(v) }
}

pub fn to_live(c: &Control, value: Option<&Value>, lang: &str) -> LiveControl {
    let v = value.or(c.value.as_ref());
    let num = v.and_then(Value::as_f64).unwrap_or(0.0);
    let (icon, has_icon) = crate::icons::named_opt(c.icon.as_deref());
    let mut lc = LiveControl {
        id: c.id.as_str().into(),
        kind: SharedString::new(),
        label: c.label.resolve(lang).into(),
        description: resolve(&c.description, lang),
        icon,
        has_icon,
        enabled: c.enabled,
        checked: false,
        value: 0.0,
        text: SharedString::new(),
        min: 0.0,
        max: 0.0,
        step: 0.0,
        unit: SharedString::new(),
        presets: ModelRc::default(),
        options: ModelRc::default(),
        tone: "default".into(),
        confirm: SharedString::new(),
        placeholder: SharedString::new(),
        hotkey: SharedString::new(),
        hotkey_busy: false,
    };
    match &c.kind {
        ControlKind::Toggle => {
            lc.kind = "toggle".into();
            lc.checked = v.and_then(Value::as_bool).unwrap_or(false);
        }
        ControlKind::Button { tone, confirm } => {
            lc.kind = "button".into();
            lc.tone = tone.as_str().into();
            lc.confirm = resolve(confirm, lang);
            if confirm.is_some() && lc.confirm.is_empty() {
                lc.confirm = if lang == "ru" { "Точно?".into() } else { "Sure?".into() };
            }
            lc.text = v.and_then(Value::as_str).unwrap_or("").into();
        }
        ControlKind::Slider { min, max, step, unit } => {
            lc.kind = "slider".into();
            lc.min = *min as f32;
            lc.max = *max as f32;
            lc.step = step.unwrap_or(0.0) as f32;
            lc.value = num.clamp(min.min(*max), max.max(*min)) as f32;
            lc.unit = unit.as_deref().unwrap_or("").into();
        }
        ControlKind::Number { min, max, step, presets, unit } => {
            lc.kind = "number".into();
            lc.min = min.unwrap_or(-1e9) as f32;
            lc.max = max.unwrap_or(1e9) as f32;
            lc.step = step.unwrap_or(1.0) as f32;
            lc.value = num as f32;
            lc.text = fmt_num(num).into();
            lc.unit = unit.as_deref().unwrap_or("").into();
            let items: Vec<PresetItem> = presets
                .iter()
                .map(|p| PresetItem { value: *p as f32, label: fmt_num(*p).into(), selected: (*p - num).abs() < 1e-9 })
                .collect();
            lc.presets = ModelRc::new(VecModel::from(items));
        }
        ControlKind::Text { placeholder, .. } => {
            lc.kind = "text".into();
            lc.text = v.and_then(Value::as_str).unwrap_or("").into();
            lc.placeholder = resolve(placeholder, lang);
        }
        ControlKind::Select { options, multiple } => {
            let selected: Vec<String> = match v {
                Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Number(n)) => vec![n.to_string()],
                _ => Vec::new(),
            };
            let items: Vec<OptionItem> = options
                .iter()
                .map(|o| {
                    let (icon, has_icon) = crate::icons::named_opt(o.icon.as_deref());
                    let label = o.label.resolve(lang);
                    OptionItem {
                        value: o.value.as_str().into(),
                        label: if label.is_empty() { o.value.as_str().into() } else { label.into() },
                        icon,
                        has_icon,
                        selected: selected.contains(&o.value),
                    }
                })
                .collect();
            let label_chars: usize = items.iter().map(|o| o.label.chars().count()).sum();
            lc.kind = if !*multiple && items.len() <= 4 && label_chars <= 44 { "segmented".into() } else { "chips".into() };
            lc.options = ModelRc::new(VecModel::from(items));
        }
        ControlKind::Stat { unit, tone } => {
            lc.kind = "stat".into();
            lc.text = display(v, lang).into();
            lc.unit = unit.as_deref().unwrap_or("").into();
            lc.tone = tone.as_str().into();
        }
        ControlKind::Progress => {
            lc.kind = "progress".into();
            lc.value = num.clamp(0.0, 1.0) as f32;
            lc.text = format!("{}%", (num.clamp(0.0, 1.0) * 100.0).round()).into();
        }
        ControlKind::Note { tone } => {
            lc.kind = "note".into();
            lc.tone = tone.as_str().into();
        }
        ControlKind::Separator => lc.kind = "separator".into(),
    }
    lc
}

/// Модели Slint для одной страницы мода.
pub struct PageModels {
    pub pages: Vec<LivePage>,
    pub sections: Vec<LiveSection>,
    pub section_models: Vec<Rc<VecModel<LiveControl>>>,
    /// id элемента → (секция, строка) на показанной странице.
    pub rows: HashMap<String, (usize, usize)>,
}

/// Горячая клавиша элемента для показа: (сочетание или "", занята ли она другой программой).
pub type HotkeyView<'a> = &'a dyn Fn(&Control) -> (SharedString, bool);

/// Элемент для Slint вместе с его горячей клавишей.
pub fn to_live_with_key(c: &Control, value: Option<&Value>, lang: &str, hotkey: HotkeyView) -> LiveControl {
    let mut lc = to_live(c, value, lang);
    (lc.hotkey, lc.hotkey_busy) = hotkey(c);
    lc
}

/// Переключатель или кнопка — им можно назначить горячую клавишу.
pub fn takes_hotkey(c: &Control) -> bool {
    matches!(c.kind, ControlKind::Toggle | ControlKind::Button { .. })
}

pub fn build_page(s: &Session, page: usize, lang: &str, hotkey: HotkeyView) -> PageModels {
    let pages = s
        .pages
        .iter()
        .map(|p| {
            let (icon, has_icon) = crate::icons::named_opt(p.icon.as_deref());
            LivePage { id: p.id.as_str().into(), title: p.title.resolve(lang).into(), icon, has_icon }
        })
        .collect();
    let mut sections = Vec::new();
    let mut section_models = Vec::new();
    let mut rows = HashMap::new();
    if let Some(p) = s.pages.get(page) {
        for sec in &p.sections {
            let controls: Vec<LiveControl> = sec
                .controls
                .iter()
                .filter(|c| c.visible)
                .map(|c| to_live_with_key(c, s.values.get(&c.id), lang, hotkey))
                .collect();
            if controls.is_empty() {
                continue;
            }
            let si = sections.len();
            for (ci, c) in controls.iter().enumerate() {
                rows.insert(c.id.to_string(), (si, ci));
            }
            let model = Rc::new(VecModel::from(controls));
            sections.push(LiveSection {
                id: sec.id.as_str().into(),
                title: resolve(&sec.title, lang),
                description: resolve(&sec.description, lang),
                controls: ModelRc::from(model.clone()),
            });
            section_models.push(model);
        }
    }
    PageModels { pages, sections, section_models, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_sent_clean() {
        assert_eq!(number_value(3.0000001, 1.0), json!(3));
        assert_eq!(number_value(0.1f32 as f64, 0.1), json!(0.1));
        assert_eq!(number_value(2.5, 0.0), json!(2.5));
        assert_eq!(fmt_num(1.5), "1.5");
        assert_eq!(fmt_num(1000.0), "1000");
        assert_eq!(fmt_num(0.125), "0.125");
    }

    #[test]
    fn select_kind_depends_on_options() {
        let few = Control::new(
            "w",
            ControlKind::Select {
                options: vec![
                    modly_protocol::SelectOption { value: "a".into(), label: "A".into(), icon: None, description: None },
                    modly_protocol::SelectOption { value: "b".into(), label: "B".into(), icon: None, description: None },
                ],
                multiple: false,
            },
            "W",
        );
        let lc = to_live(&few, Some(&json!("b")), "en");
        assert_eq!(lc.kind.as_str(), "segmented");
        let mut many = few.clone();
        if let ControlKind::Select { multiple, .. } = &mut many.kind {
            *multiple = true;
        }
        let lc = to_live(&many, Some(&json!(["a", "b"])), "en");
        assert_eq!(lc.kind.as_str(), "chips");
    }
}
