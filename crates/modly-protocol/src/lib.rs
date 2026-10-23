//! Протокол Modly ↔ мод (версия 1).
//!
//! Транспорт — именованный канал Windows [`PIPE_NAME`]: Modly — сервер, моды — клиенты.
//! Каждое сообщение — одна строка JSON в UTF-8, завершённая `\n` (NDJSON).
//! Мод описывает свой интерфейс ([`Layout`]) — страницы, секции и элементы управления, — а Modly
//! рисует его и присылает обратно изменения ([`ServerMsg::Set`]) и нажатия ([`ServerMsg::Invoke`]).
//!
//! Полное описание для авторов модов на любом языке — `docs/PROTOCOL.md`.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Версия протокола. Растёт только при несовместимых изменениях;
/// новые необязательные поля и виды элементов добавляются без смены версии.
pub const PROTOCOL_VERSION: u32 = 1;

/// Имя канала, который слушает Modly.
pub const PIPE_NAME: &str = r"\\.\pipe\modly";

/// Предел длины одного сообщения (строки) в байтах.
pub const MAX_MESSAGE: usize = 4 << 20;

// ---------------------------------------------------------------------------------------------
// Текст

/// Текст для интерфейса: просто строка или переводы `{"en": "...", "ru": "..."}`.
/// Modly показывает перевод на своём языке, иначе английский, иначе первый попавшийся.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Text {
    Plain(String),
    Localized(BTreeMap<String, String>),
}

impl Default for Text {
    fn default() -> Self {
        Text::Plain(String::new())
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Text::Plain(s.to_owned())
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Text::Plain(s)
    }
}

impl Text {
    /// Переводы парами: `Text::tr(&[("en", "Speed"), ("ru", "Скорость")])`.
    pub fn tr(pairs: &[(&str, &str)]) -> Self {
        Text::Localized(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
    }

    pub fn resolve(&self, lang: &str) -> &str {
        match self {
            Text::Plain(s) => s,
            Text::Localized(map) => map
                .get(lang)
                .or_else(|| map.get("en"))
                .or_else(|| map.values().next())
                .map(String::as_str)
                .unwrap_or(""),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Text::Plain(s) => s.is_empty(),
            Text::Localized(map) => map.values().all(String::is_empty),
        }
    }
}

/// Оттенок: для кнопок, уведомлений, статусов и показателей.
/// Неизвестное значение читается как `Default` — моды из будущих версий не ломают разбор.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Default,
    Muted,
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub fn as_str(self) -> &'static str {
        match self {
            Tone::Default => "default",
            Tone::Muted => "muted",
            Tone::Accent => "accent",
            Tone::Info => "info",
            Tone::Success => "success",
            Tone::Warning => "warning",
            Tone::Danger => "danger",
        }
    }
}

impl<'de> Deserialize<'de> for Tone {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "muted" => Tone::Muted,
            "accent" | "primary" => Tone::Accent,
            "info" => Tone::Info,
            "success" | "ok" => Tone::Success,
            "warning" | "warn" => Tone::Warning,
            "danger" | "error" => Tone::Danger,
            _ => Tone::Default,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Описание мода и игры

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl ModInfo {
    pub fn new(id: &str, name: &str, version: &str) -> Self {
        ModInfo { id: id.into(), name: name.into(), version: version.into(), ..Default::default() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameInfo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

impl GameInfo {
    pub fn new(id: &str, name: &str, exe: Option<&str>) -> Self {
        GameInfo { id: id.into(), name: name.into(), exe: exe.map(Into::into) }
    }
}

// ---------------------------------------------------------------------------------------------
// Интерфейс мода

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub pages: Vec<Page>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    #[serde(default)]
    pub title: Text,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub sections: Vec<Section>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Элементы неизвестного вида или с ошибкой пропускаются, а не ломают весь интерфейс.
    #[serde(default, deserialize_with = "skip_invalid")]
    pub controls: Vec<Control>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    #[serde(flatten)]
    pub kind: ControlKind,
    #[serde(default)]
    pub label: Text,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Текущее значение (для toggle — bool, slider/number — число, text — строка,
    /// select — строка или массив строк при `multiple`, stat — строка или число, progress — 0..1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Предлагаемая горячая клавиша, например `"F1"` или `"Ctrl+Num1"`. Пользователь может переназначить.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlKind {
    /// Переключатель. Значение — bool.
    Toggle,
    /// Кнопка-действие. `confirm` — нужно нажать второй раз для подтверждения (для опасных действий).
    Button {
        #[serde(default)]
        tone: Tone,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirm: Option<Text>,
    },
    /// Ползунок. Значение — число.
    Slider {
        min: f64,
        max: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// Поле для числа с кнопками −/+ и быстрыми вариантами (`presets`).
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        presets: Vec<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// Текстовое поле. Значение — строка; отправляется по Enter.
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<Text>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<u32>,
    },
    /// Выбор из вариантов. `multiple` — можно отметить несколько (значение — массив строк).
    Select {
        options: Vec<SelectOption>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        multiple: bool,
    },
    /// Показатель только для чтения: «Волчат в стае: 128».
    Stat {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default)]
        tone: Tone,
    },
    /// Полоса прогресса, значение 0..1.
    Progress,
    /// Поясняющий текст. Текст — в `label`.
    Note {
        #[serde(default)]
        tone: Tone,
    },
    /// Разделитель.
    Separator,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectOption {
    pub value: String,
    #[serde(default)]
    pub label: Text,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
}

/// Частичное изменение свойств элемента (не значения — для значений есть `state`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<SelectOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presets: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Text>,
}

// ---------------------------------------------------------------------------------------------
// Карты

/// Края картинки карты в координатах игры. Метки и точки кликов — в тех же координатах, поэтому
/// мод не пересчитывает их в пиксели: у каждой игры свой мир, и Modly подстраивается под него.
/// Оси могут идти в любую сторону: если y мира растёт вверх, `top` больше `bottom`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapBounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl MapBounds {
    /// Пиксели картинки: левый верхний угол — (0, 0).
    pub fn pixels(width: f64, height: f64) -> Self {
        MapBounds { left: 0.0, top: 0.0, right: width, bottom: height }
    }

    /// Можно ли по этим краям считать: ширина и высота не нулевые, числа конечные.
    pub fn is_valid(&self) -> bool {
        [self.left, self.top, self.right, self.bottom].iter().all(|v| v.is_finite())
            && (self.right - self.left).abs() > f64::EPSILON
            && (self.bottom - self.top).abs() > f64::EPSILON
    }

    /// Точка игры → доли картинки (0…1 — внутри карты).
    pub fn to_unit(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.left) / (self.right - self.left), (y - self.top) / (self.bottom - self.top))
    }

    /// Доли картинки → точка игры.
    pub fn from_unit(&self, u: f64, v: f64) -> (f64, f64) {
        (self.left + u * (self.right - self.left), self.top + v * (self.bottom - self.top))
    }
}

/// Категория меток: «Места», «Предметы», «Опасность». Её можно скрыть на карте.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapCategory {
    pub id: String,
    #[serde(default)]
    pub label: Text,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Цвет `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Скрыта, пока игрок сам её не включит (например, «Все сундуки» на огромной карте).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Вид метки: булавка (место) или точка (то, что движется: игрок, питомец, повозка).
/// Неизвестный вид читается как булавка.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerStyle {
    #[default]
    Pin,
    Dot,
}

impl<'de> Deserialize<'de> for MarkerStyle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(if String::deserialize(d)? == "dot" { MarkerStyle::Dot } else { MarkerStyle::Pin })
    }
}

fn is_pin(s: &MarkerStyle) -> bool {
    *s == MarkerStyle::Pin
}

/// Действие на карте: кнопка в подписи метки («Телепорт», «Позвать питомца») или по клику в
/// пустое место («Телепорт сюда»). Modly присылает моду [`ServerMsg::MapAction`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapAction {
    pub id: String,
    #[serde(default)]
    pub label: Text,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub tone: Tone,
    /// Срабатывать со второго нажатия (для опасных действий).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Text>,
}

/// Метка на карте. `x`, `y` — в координатах игры (см. [`MapBounds`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapMarker {
    pub id: String,
    pub x: f64,
    pub y: f64,
    /// Категория (`MapCategory::id`); без неё метку нельзя скрыть фильтром.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cat: String,
    #[serde(default)]
    pub title: Text,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Text>,
    /// Иконка и цвет — свои (иначе — категории).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "is_pin")]
    pub style: MarkerStyle,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<MapAction>,
}

/// Карта мода: картинка, её края в координатах игры, категории и метки.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameMap {
    pub id: String,
    #[serde(default)]
    pub title: Text,
    /// Картинка: от мода — полный путь к PNG, JPEG или WebP на этом компьютере
    /// (`C:\…\map.png`) или `data:image/png;base64,…`; в `modly.json` — путь внутри пакета.
    pub image: String,
    /// Края картинки в координатах игры. Нет — координаты меток в пикселях картинки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<MapBounds>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<MapCategory>,
    /// Метки с ошибкой в полях пропускаются, а не ломают карту.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "skip_invalid")]
    pub markers: Vec<MapMarker>,
    /// Действия по клику в пустое место карты (Modly пришлёт координаты точки).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub click_actions: Vec<MapAction>,
    /// Метка, к которой карта подъезжает кнопкой «Где я» (обычно — игрок).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Сообщения

/// Сообщения мода → Modly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// `hello` крупнее остальных, но приходит один раз за сессию — упаковывать в Box незачем
#[allow(clippy::large_enum_variant)]
pub enum ClientMsg {
    /// Первое сообщение после подключения.
    Hello {
        protocol: u32,
        #[serde(rename = "mod")]
        mod_info: ModInfo,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        game: Option<GameInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdk: Option<String>,
    },
    /// Весь интерфейс мода. Можно присылать повторно — он заменит прежний.
    Layout {
        #[serde(default)]
        pages: Vec<Page>,
    },
    /// Новые значения элементов: `{"values": {"god": true, "speed": 2.5}}`.
    State { values: Map<String, Value> },
    /// Изменение свойств элементов: `{"controls": {"summon": {"enabled": false}}}`.
    Patch { controls: BTreeMap<String, ControlPatch> },
    /// Всплывающее уведомление.
    Notify {
        #[serde(default)]
        tone: Tone,
        text: Text,
    },
    /// Строка состояния мода (например, «Ждём загрузку мира»). Пустой текст — убрать.
    Status {
        #[serde(default)]
        tone: Tone,
        #[serde(default)]
        text: Text,
    },
    /// Ответ на `set` / `invoke`.
    Result {
        req: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<Text>,
    },
    /// Строка в консоль разработчика Modly.
    Log {
        #[serde(default)]
        tone: Tone,
        text: String,
    },
    /// Главный поток игры встал (обычно игра на паузе без фокуса) или снова пошёл.
    /// SDK присылают это сами: команды Modly выполнятся, когда игра продолжит работу.
    Paused { paused: bool },
    /// Карта мода целиком. Карта с тем же `id` заменяет прежнюю.
    Map(GameMap),
    /// Изменить метки карты: `set` — добавить или заменить (по `id`), `remove` — убрать.
    /// Так двигаются «живые» метки — игрок, питомцы.
    MapPatch {
        map: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "skip_invalid")]
        set: Vec<MapMarker>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
    },
    /// Убрать карту.
    MapRemove { map: String },
    Pong,
    Bye,
}

/// Сообщения Modly → мод.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Ответ на `hello`.
    Welcome { protocol: u32, app: String, version: String, lang: String },
    /// Пользователь изменил значение элемента.
    Set { req: u64, id: String, value: Value },
    /// Пользователь нажал кнопку.
    Invoke { req: u64, id: String },
    /// Пользователь сменил язык Modly.
    Lang { lang: String },
    /// Действие на карте: кнопка в подписи метки (`marker` — её id, `x`, `y` — её место) или
    /// клик в пустое место (`marker` нет, `x`, `y` — точка в координатах игры).
    MapAction {
        req: u64,
        map: String,
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marker: Option<String>,
        x: f64,
        y: f64,
    },
    Ping,
    /// Ошибка протокола; после неё Modly закрывает соединение.
    Error { text: String },
}

// ---------------------------------------------------------------------------------------------
// Кодирование

/// Сообщение в строку NDJSON (с завершающим `\n`).
pub fn encode<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).expect("protocol messages always serialize");
    s.push('\n');
    s
}

pub fn decode_client(line: &str) -> Result<ClientMsg, serde_json::Error> {
    serde_json::from_str(line.trim_end())
}

pub fn decode_server(line: &str) -> Result<ServerMsg, serde_json::Error> {
    serde_json::from_str(line.trim_end())
}

// ---------------------------------------------------------------------------------------------

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn skip_invalid<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let raw: Vec<Value> = Vec::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

impl Control {
    pub fn new(id: impl Into<String>, kind: ControlKind, label: impl Into<Text>) -> Self {
        Control {
            id: id.into(),
            kind,
            label: label.into(),
            description: None,
            icon: None,
            enabled: true,
            visible: true,
            value: None,
            hotkey: None,
        }
    }

    /// Можно ли пользователю менять значение (элементы ввода, а не кнопки и показатели).
    pub fn is_input(&self) -> bool {
        matches!(
            self.kind,
            ControlKind::Toggle
                | ControlKind::Slider { .. }
                | ControlKind::Number { .. }
                | ControlKind::Text { .. }
                | ControlKind::Select { .. }
        )
    }

    pub fn apply(&mut self, p: &ControlPatch) {
        if let Some(v) = &p.label {
            self.label = v.clone();
        }
        if let Some(v) = &p.description {
            self.description = Some(v.clone());
        }
        if let Some(v) = &p.icon {
            self.icon = Some(v.clone());
        }
        if let Some(v) = p.enabled {
            self.enabled = v;
        }
        if let Some(v) = p.visible {
            self.visible = v;
        }
        match &mut self.kind {
            ControlKind::Slider { min, max, step, unit } => {
                if let Some(v) = p.min {
                    *min = v;
                }
                if let Some(v) = p.max {
                    *max = v;
                }
                if p.step.is_some() {
                    *step = p.step;
                }
                if p.unit.is_some() {
                    *unit = p.unit.clone();
                }
            }
            ControlKind::Number { min, max, step, presets, unit } => {
                if p.min.is_some() {
                    *min = p.min;
                }
                if p.max.is_some() {
                    *max = p.max;
                }
                if p.step.is_some() {
                    *step = p.step;
                }
                if let Some(v) = &p.presets {
                    *presets = v.clone();
                }
                if p.unit.is_some() {
                    *unit = p.unit.clone();
                }
            }
            ControlKind::Select { options, .. } => {
                if let Some(v) = &p.options {
                    *options = v.clone();
                }
            }
            ControlKind::Button { tone, confirm } => {
                if let Some(v) = p.tone {
                    *tone = v;
                }
                if p.confirm.is_some() {
                    *confirm = p.confirm.clone();
                }
            }
            ControlKind::Stat { unit, tone } => {
                if let Some(v) = p.tone {
                    *tone = v;
                }
                if p.unit.is_some() {
                    *unit = p.unit.clone();
                }
            }
            ControlKind::Note { tone } => {
                if let Some(v) = p.tone {
                    *tone = v;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_resolves_language_with_fallback() {
        let t = Text::tr(&[("en", "Speed"), ("ru", "Скорость")]);
        assert_eq!(t.resolve("ru"), "Скорость");
        assert_eq!(t.resolve("de"), "Speed");
        assert_eq!(Text::from("x").resolve("ru"), "x");
    }

    #[test]
    fn hello_roundtrip() {
        let line = r#"{"type":"hello","protocol":1,"mod":{"id":"creatly","name":"Creatly","version":"1.0.1"},"pid":42}"#;
        let msg = decode_client(line).unwrap();
        match &msg {
            ClientMsg::Hello { protocol, mod_info, pid, .. } => {
                assert_eq!(*protocol, 1);
                assert_eq!(mod_info.id, "creatly");
                assert_eq!(*pid, Some(42));
            }
            other => panic!("unexpected {other:?}"),
        }
        let again = decode_client(&encode(&msg)).unwrap();
        assert_eq!(msg, again);
    }

    #[test]
    fn layout_parses_all_kinds_and_skips_unknown() {
        let v = json!({"type":"layout","pages":[{"id":"main","title":{"en":"Main","ru":"Главное"},"sections":[{"id":"s","controls":[
            {"id":"god","kind":"toggle","label":"God mode","value":true},
            {"id":"heal","kind":"button","label":"Heal","tone":"accent","confirm":"Sure?"},
            {"id":"speed","kind":"slider","label":"Speed","min":0.5,"max":5,"step":0.1,"unit":"x","value":1},
            {"id":"coins","kind":"number","label":"Coins","min":0,"presets":[100,1000]},
            {"id":"name","kind":"text","label":"Name"},
            {"id":"weather","kind":"select","label":"Weather","options":[{"value":"clear","label":"Clear"},{"value":"rain","label":"Rain"}],"value":"clear"},
            {"id":"kinds","kind":"select","multiple":true,"options":[{"value":"a"}],"value":["a"]},
            {"id":"pack","kind":"stat","label":"Pack","value":128,"tone":"success"},
            {"id":"p","kind":"progress","value":0.5},
            {"id":"n","kind":"note","label":"hi","tone":"warning"},
            {"id":"sep","kind":"separator"},
            {"id":"future","kind":"hologram","label":"from the future"},
            {"kind":"toggle","label":"no id"}
        ]}]}]});
        let msg = decode_client(&v.to_string()).unwrap();
        let ClientMsg::Layout { pages } = msg else { panic!() };
        let controls = &pages[0].sections[0].controls;
        assert_eq!(controls.len(), 11);
        assert!(matches!(controls[1].kind, ControlKind::Button { tone: Tone::Accent, .. }));
        assert!(matches!(controls[6].kind, ControlKind::Select { multiple: true, .. }));
        assert_eq!(pages[0].title.resolve("ru"), "Главное");
    }

    #[test]
    fn unknown_tone_falls_back_to_default() {
        let msg = decode_client(r#"{"type":"notify","tone":"sparkly","text":"hi"}"#).unwrap();
        assert!(matches!(msg, ClientMsg::Notify { tone: Tone::Default, .. }));
        let msg = decode_client(r#"{"type":"notify","tone":"error","text":"x"}"#).unwrap();
        assert!(matches!(msg, ClientMsg::Notify { tone: Tone::Danger, .. }));
    }

    #[test]
    fn server_messages_are_compact_lines() {
        let s = encode(&ServerMsg::Set { req: 3, id: "god".into(), value: json!(true) });
        assert_eq!(s, "{\"type\":\"set\",\"req\":3,\"id\":\"god\",\"value\":true}\n");
        assert_eq!(decode_server(&s).unwrap(), ServerMsg::Set { req: 3, id: "god".into(), value: json!(true) });
    }

    #[test]
    fn maps_parse_and_skip_broken_markers() {
        let v = json!({"type":"map","id":"world","title":{"en":"Island","ru":"Остров"},"image":"C:\\mods\\map.png",
            "bounds":{"left":-1000,"top":1000,"right":1000,"bottom":-1000},
            "categories":[{"id":"places","label":"Places","icon":"map-pin","color":"#8b5cf6"},{"id":"loot","hidden":true}],
            "markers":[
                {"id":"home","x":120.5,"y":-40,"cat":"places","title":"Home","actions":[{"id":"tp","label":"Teleport","tone":"accent"}]},
                {"id":"player","x":0,"y":0,"style":"dot","color":"#22d3ee"},
                {"id":"future","x":1,"y":1,"style":"hologram"},
                {"id":"broken","x":"far"}
            ],
            "click_actions":[{"id":"tp","label":"Teleport here","confirm":"Sure?"}],"focus":"player"});
        let ClientMsg::Map(m) = decode_client(&v.to_string()).unwrap() else { panic!() };
        assert_eq!(m.markers.len(), 3, "битая метка пропущена");
        assert_eq!(m.markers[1].style, MarkerStyle::Dot);
        assert_eq!(m.markers[2].style, MarkerStyle::Pin, "неизвестный вид — булавка");
        assert!(m.categories[1].hidden);
        assert_eq!(m.focus.as_deref(), Some("player"));
        // y мира растёт вверх: верх картинки — 1000
        let b = m.bounds.unwrap();
        assert!(b.is_valid());
        assert_eq!(b.to_unit(0.0, 0.0), (0.5, 0.5));
        assert_eq!(b.to_unit(-1000.0, 1000.0), (0.0, 0.0));
        assert_eq!(b.from_unit(1.0, 1.0), (1000.0, -1000.0));
        assert!(!MapBounds { left: 1.0, top: 0.0, right: 1.0, bottom: 5.0 }.is_valid());
        // туда и обратно
        assert_eq!(decode_client(&encode(&ClientMsg::Map(m.clone()))).unwrap(), ClientMsg::Map(m));

        let p = decode_client(r#"{"type":"map_patch","map":"world","set":[{"id":"player","x":5,"y":6,"style":"dot"}],"remove":["home"]}"#).unwrap();
        assert!(matches!(&p, ClientMsg::MapPatch { set, remove, .. } if set.len() == 1 && remove == &["home"]));
        let r = decode_client(r#"{"type":"map_remove","map":"world"}"#).unwrap();
        assert_eq!(r, ClientMsg::MapRemove { map: "world".into() });

        let a = encode(&ServerMsg::MapAction { req: 9, map: "world".into(), action: "tp".into(), marker: None, x: 1.5, y: -2.0 });
        assert_eq!(a, "{\"type\":\"map_action\",\"req\":9,\"map\":\"world\",\"action\":\"tp\",\"x\":1.5,\"y\":-2.0}\n");
    }

    #[test]
    fn patch_applies_to_control() {
        let mut c = Control::new("speed", ControlKind::Slider { min: 0.0, max: 1.0, step: None, unit: None }, "Speed");
        c.apply(&ControlPatch { enabled: Some(false), max: Some(10.0), ..Default::default() });
        assert!(!c.enabled);
        assert!(matches!(c.kind, ControlKind::Slider { max, .. } if max == 10.0));
    }
}
