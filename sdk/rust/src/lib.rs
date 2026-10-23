//! # Modly SDK для Rust
//!
//! Опишите переключатели, ползунки и кнопки своего мода — Modly нарисует их в своём окне и будет
//! присылать изменения. Мод работает и без Modly: пока Modly не запущен, SDK тихо ждёт в фоне.
//!
//! ```no_run
//! use modly_sdk::*;
//! use std::{cell::RefCell, rc::Rc};
//!
//! #[derive(Default)]
//! struct Cheats { god: bool, speed: f64 }
//!
//! let state = Rc::new(RefCell::new(Cheats { speed: 1.0, ..Default::default() }));
//! let mut panel = Panel::new(ModInfo::new("my-mod", "My Mod", "1.0.0"), None);
//! {
//!     let s = state.clone();
//!     let s2 = state.clone();
//!     panel
//!         .page("main", "Main", Some("star"))
//!         .section("player", "Player")
//!         .add(toggle("god", "God mode").icon("shield"), value(move || s.borrow().god, move |v| s2.borrow_mut().god = v));
//! }
//! loop {
//!     panel.pump(); // в игровом цикле, на главном потоке
//! #   break;
//! }
//! ```

use modly_protocol::{ClientMsg, Control, ControlKind, ControlPatch, GameMap, Layout, MapAction, MapBounds, MapCategory, MapMarker, MarkerStyle, Page, Section, ServerMsg, encode};
pub use modly_protocol::{self as protocol, GameInfo, ModInfo, SelectOption, Text, Tone};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SDK_NAME: &str = concat!("modly-rust/", env!("CARGO_PKG_VERSION"));

// =============================================================================================
// Соединение

/// Низкоуровневое соединение с Modly: фоновый поток, сам переподключается.
/// Обычно удобнее [`Panel`], но `Client` можно использовать и напрямую.
pub struct Client {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Shared {
    hello: String,
    outgoing: Mutex<VecDeque<String>>,
    incoming: Mutex<VecDeque<ServerMsg>>,
    connected: AtomicBool,
    generation: AtomicU64,
    stop: AtomicBool,
    pipe: String,
    started: Instant,
    /// Когда главный поток последний раз вызывал pump (мс от `started`).
    last_pump: AtomicU64,
}

impl Client {
    pub fn start(info: ModInfo, game: Option<GameInfo>) -> Self {
        Self::start_on(modly_protocol::PIPE_NAME, info, game)
    }

    pub fn start_on(pipe: &str, info: ModInfo, game: Option<GameInfo>) -> Self {
        let hello = encode(&ClientMsg::Hello {
            protocol: modly_protocol::PROTOCOL_VERSION,
            mod_info: info,
            game,
            pid: Some(std::process::id()),
            sdk: Some(SDK_NAME.into()),
        });
        let shared = Arc::new(Shared {
            hello,
            outgoing: Mutex::new(VecDeque::new()),
            incoming: Mutex::new(VecDeque::new()),
            connected: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            pipe: pipe.to_owned(),
            started: Instant::now(),
            last_pump: AtomicU64::new(0),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("modly-sdk".into())
            .spawn(move || io_loop(&s))
            .expect("spawn modly thread");
        Client { shared, thread: Some(thread) }
    }

    /// Отправить сообщение. Пока Modly не подключён, сообщения не копятся, а отбрасываются.
    pub fn send(&self, msg: &ClientMsg) {
        if self.is_connected() {
            self.shared.outgoing.lock().unwrap().push_back(encode(msg));
        }
    }

    /// Забрать пришедшие сообщения.
    pub fn poll(&self) -> Vec<ServerMsg> {
        self.shared.incoming.lock().unwrap().drain(..).collect()
    }

    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    /// Отметить, что главный поток жив (это делает [`Panel::pump`]). Если отметок нет дольше 1,5 с,
    /// SDK сообщит Modly, что игра на паузе и команды подождут.
    pub fn touch(&self) {
        self.shared.last_pump.store(self.shared.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// Номер подключения: меняется при каждом новом подключении к Modly
    /// (после него нужно заново прислать интерфейс).
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.is_connected() {
            self.shared.outgoing.lock().unwrap().push_back(encode(&ClientMsg::Bye));
        }
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn io_loop(s: &Shared) {
    while !s.stop.load(Ordering::Acquire) {
        let Some(mut pipe) = pipe::open(&s.pipe) else {
            sleep_unless_stopped(s, Duration::from_secs(2));
            continue;
        };
        if pipe.write_all(s.hello.as_bytes()).is_err() {
            continue;
        }
        s.outgoing.lock().unwrap().clear();
        s.last_pump.store(s.started.elapsed().as_millis() as u64, Ordering::Relaxed);
        s.connected.store(true, Ordering::Release);
        s.generation.fetch_add(1, Ordering::AcqRel);

        let mut buf: Vec<u8> = Vec::new();
        let mut paused_sent = false;
        'conn: while !s.stop.load(Ordering::Acquire) {
            let out: Vec<String> = s.outgoing.lock().unwrap().drain(..).collect();
            for line in out {
                if pipe.write_all(line.as_bytes()).is_err() {
                    break 'conn;
                }
            }
            // главный поток игры встал (пауза без фокуса) — сказать Modly, что команды подождут
            let now = s.started.elapsed().as_millis() as u64;
            let stalled = now.saturating_sub(s.last_pump.load(Ordering::Relaxed)) > 1500;
            if stalled != paused_sent {
                paused_sent = stalled;
                if pipe.write_all(encode(&ClientMsg::Paused { paused: stalled }).as_bytes()).is_err() {
                    break 'conn;
                }
            }
            match pipe.read_available(&mut buf) {
                Err(()) => break,
                Ok(0) => std::thread::sleep(Duration::from_millis(15)),
                Ok(_) => {
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let raw: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&raw);
                        if let Ok(msg) = modly_protocol::decode_server(&line) {
                            if matches!(msg, ServerMsg::Ping) {
                                s.outgoing.lock().unwrap().push_back(encode(&ClientMsg::Pong));
                                continue;
                            }
                            s.incoming.lock().unwrap().push_back(msg);
                        }
                    }
                    if buf.len() > modly_protocol::MAX_MESSAGE {
                        break;
                    }
                }
            }
        }
        // дослать «до свидания», если нас закрывают
        let out: Vec<String> = s.outgoing.lock().unwrap().drain(..).collect();
        for line in out {
            let _ = pipe.write_all(line.as_bytes());
        }
        s.connected.store(false, Ordering::Release);
    }
}

fn sleep_unless_stopped(s: &Shared, total: Duration) {
    let step = Duration::from_millis(50);
    let mut left = total;
    while !left.is_zero() && !s.stop.load(Ordering::Acquire) {
        std::thread::sleep(step.min(left));
        left = left.saturating_sub(step);
    }
}

#[cfg(windows)]
mod pipe {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    pub struct Pipe(File);

    pub fn open(name: &str) -> Option<Pipe> {
        std::fs::OpenOptions::new().read(true).write(true).open(name).ok().map(Pipe)
    }

    impl Pipe {
        pub fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.0.write_all(data)
        }

        /// Прочитать то, что уже пришло, не блокируясь (чтение и запись идут из одного потока,
        /// а синхронный канал Windows не даёт писать, пока висит чтение).
        pub fn read_available(&mut self, buf: &mut Vec<u8>) -> Result<usize, ()> {
            let mut avail: u32 = 0;
            let ok = unsafe {
                PeekNamedPipe(self.0.as_raw_handle() as _, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut())
            };
            if ok == 0 {
                return Err(());
            }
            if avail == 0 {
                return Ok(0);
            }
            let mut chunk = vec![0u8; avail.min(64 * 1024) as usize];
            match self.0.read(&mut chunk) {
                Ok(0) | Err(_) => Err(()),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    Ok(n)
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod pipe {
    pub struct Pipe;
    pub fn open(_name: &str) -> Option<Pipe> {
        None
    }
    impl Pipe {
        pub fn write_all(&mut self, _data: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        pub fn read_available(&mut self, _buf: &mut Vec<u8>) -> Result<usize, ()> {
            Err(())
        }
    }
}

// =============================================================================================
// Элементы интерфейса

/// Описание элемента. Создаётся функциями [`toggle`], [`button`], [`slider`]… и настраивается цепочкой.
#[derive(Clone, Debug)]
pub struct Spec {
    control: Control,
}

fn spec(id: &str, kind: ControlKind, label: impl Into<Text>) -> Spec {
    Spec { control: Control::new(id, kind, label) }
}

pub fn toggle(id: &str, label: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Toggle, label)
}

pub fn button(id: &str, label: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Button { tone: Tone::Default, confirm: None }, label)
}

pub fn slider(id: &str, label: impl Into<Text>, min: f64, max: f64) -> Spec {
    spec(id, ControlKind::Slider { min, max, step: None, unit: None }, label)
}

pub fn number(id: &str, label: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Number { min: None, max: None, step: None, presets: Vec::new(), unit: None }, label)
}

pub fn text(id: &str, label: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Text { placeholder: None, max_length: None }, label)
}

/// Выбор одного варианта: `select("weather", "Weather", [("clear", "Clear"), ("rain", "Rain")])`.
pub fn select<V: Into<String>, L: Into<Text>>(id: &str, label: impl Into<Text>, options: impl IntoIterator<Item = (V, L)>) -> Spec {
    spec(id, ControlKind::Select { options: to_options(options), multiple: false }, label)
}

/// Выбор нескольких вариантов (значение — `Vec<String>`).
pub fn multi_select<V: Into<String>, L: Into<Text>>(id: &str, label: impl Into<Text>, options: impl IntoIterator<Item = (V, L)>) -> Spec {
    spec(id, ControlKind::Select { options: to_options(options), multiple: true }, label)
}

pub fn stat(id: &str, label: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Stat { unit: None, tone: Tone::Default }, label)
}

pub fn progress(id: &str, label: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Progress, label)
}

pub fn note(id: &str, text: impl Into<Text>) -> Spec {
    spec(id, ControlKind::Note { tone: Tone::Default }, text)
}

pub fn separator(id: &str) -> Spec {
    spec(id, ControlKind::Separator, "")
}

fn to_options<V: Into<String>, L: Into<Text>>(options: impl IntoIterator<Item = (V, L)>) -> Vec<SelectOption> {
    options
        .into_iter()
        .map(|(v, l)| SelectOption { value: v.into(), label: l.into(), icon: None, description: None })
        .collect()
}

impl Spec {
    pub fn icon(mut self, name: &str) -> Self {
        self.control.icon = Some(name.into());
        self
    }

    pub fn describe(mut self, text: impl Into<Text>) -> Self {
        self.control.description = Some(text.into());
        self
    }

    pub fn hotkey(mut self, key: &str) -> Self {
        self.control.hotkey = Some(key.into());
        self
    }

    pub fn disabled(mut self) -> Self {
        self.control.enabled = false;
        self
    }

    pub fn tone(mut self, t: Tone) -> Self {
        match &mut self.control.kind {
            ControlKind::Button { tone, .. } | ControlKind::Stat { tone, .. } | ControlKind::Note { tone } => *tone = t,
            _ => {}
        }
        self
    }

    /// Кнопка срабатывает только со второго нажатия.
    pub fn confirm(mut self, text: impl Into<Text>) -> Self {
        if let ControlKind::Button { confirm, .. } = &mut self.control.kind {
            *confirm = Some(text.into());
        }
        self
    }

    pub fn step(mut self, v: f64) -> Self {
        match &mut self.control.kind {
            ControlKind::Slider { step, .. } | ControlKind::Number { step, .. } => *step = Some(v),
            _ => {}
        }
        self
    }

    pub fn unit(mut self, u: &str) -> Self {
        match &mut self.control.kind {
            ControlKind::Slider { unit, .. } | ControlKind::Number { unit, .. } | ControlKind::Stat { unit, .. } => *unit = Some(u.into()),
            _ => {}
        }
        self
    }

    pub fn range(mut self, lo: f64, hi: f64) -> Self {
        match &mut self.control.kind {
            ControlKind::Slider { min, max, .. } => {
                *min = lo;
                *max = hi;
            }
            ControlKind::Number { min, max, .. } => {
                *min = Some(lo);
                *max = Some(hi);
            }
            _ => {}
        }
        self
    }

    pub fn presets(mut self, values: &[f64]) -> Self {
        if let ControlKind::Number { presets, .. } = &mut self.control.kind {
            *presets = values.to_vec();
        }
        self
    }

    pub fn placeholder(mut self, text: impl Into<Text>) -> Self {
        if let ControlKind::Text { placeholder, .. } = &mut self.control.kind {
            *placeholder = Some(text.into());
        }
        self
    }

    /// Иконки вариантов выбора: по порядку.
    pub fn option_icons(mut self, icons: &[&str]) -> Self {
        if let ControlKind::Select { options, .. } = &mut self.control.kind {
            for (o, i) in options.iter_mut().zip(icons) {
                o.icon = Some((*i).into());
            }
        }
        self
    }
}

// =============================================================================================
// Привязки

/// Как элемент связан с данными мода.
pub enum Binding {
    None,
    Read(Box<dyn FnMut() -> Value>),
    Value { get: Box<dyn FnMut() -> Value>, set: Box<dyn FnMut(Value) -> Result<(), String>> },
    Action(Box<dyn FnMut() -> Result<Option<String>, String>>),
}

/// Тип, который можно получить из значения протокола.
pub trait FromValue: Sized {
    fn from_value(v: &Value) -> Option<Self>;
}

impl FromValue for bool {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_bool()
    }
}
impl FromValue for f64 {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_f64()
    }
}
impl FromValue for f32 {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_f64().map(|x| x as f32)
    }
}
impl FromValue for i64 {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_f64().map(|x| x.round() as i64)
    }
}
impl FromValue for i32 {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_f64().map(|x| x.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
    }
}
impl FromValue for u32 {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_f64().map(|x| x.round().clamp(0.0, u32::MAX as f64) as u32)
    }
}
impl FromValue for String {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_str().map(str::to_owned)
    }
}
impl FromValue for Vec<String> {
    fn from_value(v: &Value) -> Option<Self> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
    }
}

/// Значение, которое можно и читать, и менять (toggle, slider, number, text, select).
pub fn value<T, G, S>(mut get: G, mut set: S) -> Binding
where
    T: Into<Value> + FromValue + 'static,
    G: FnMut() -> T + 'static,
    S: FnMut(T) + 'static,
{
    Binding::Value {
        get: Box::new(move || get().into()),
        set: Box::new(move |v| {
            let t = T::from_value(&v).ok_or_else(|| format!("неверное значение: {v}"))?;
            set(t);
            Ok(())
        }),
    }
}

/// Значение только для чтения (stat, progress).
pub fn read<T: Into<Value> + 'static>(mut get: impl FnMut() -> T + 'static) -> Binding {
    Binding::Read(Box::new(move || get().into()))
}

/// Действие кнопки.
pub fn action(mut f: impl FnMut() + 'static) -> Binding {
    Binding::Action(Box::new(move || {
        f();
        Ok(None)
    }))
}

/// Действие кнопки с ответом: `Ok(Some("Готово"))` покажется уведомлением, `Err` — ошибкой.
pub fn action_with(f: impl FnMut() -> Result<Option<String>, String> + 'static) -> Binding {
    Binding::Action(Box::new(f))
}

// =============================================================================================
// Панель

struct PageDef {
    id: String,
    title: Text,
    icon: Option<String>,
    sections: Vec<SectionDef>,
}

struct SectionDef {
    id: String,
    title: Option<Text>,
    description: Option<Text>,
    controls: Vec<String>,
}

/// Интерфейс мода в Modly: элементы + привязки к данным. Вызывайте [`Panel::pump`] регулярно.
pub struct Panel {
    client: Client,
    pages: Vec<PageDef>,
    controls: HashMap<String, (Control, Binding)>,
    sent: HashMap<String, Value>,
    generation: u64,
    last_sync: Instant,
    sync_every: Duration,
    lang: String,
    maps: Vec<MapDef>,
}

impl Panel {
    pub fn new(info: ModInfo, game: Option<GameInfo>) -> Self {
        Self::with_client(Client::start(info, game))
    }

    pub fn with_client(client: Client) -> Self {
        Panel {
            client,
            pages: Vec::new(),
            controls: HashMap::new(),
            sent: HashMap::new(),
            generation: 0,
            last_sync: Instant::now(),
            sync_every: Duration::from_millis(200),
            lang: "en".into(),
            maps: Vec::new(),
        }
    }

    /// Язык Modly ("ru", "en"…) — если хотите сами выбирать тексты.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn page(&mut self, id: &str, title: impl Into<Text>, icon: Option<&str>) -> PageMut<'_> {
        let index = match self.pages.iter().position(|p| p.id == id) {
            Some(i) => i,
            None => {
                self.pages.push(PageDef { id: id.into(), title: title.into(), icon: icon.map(Into::into), sections: Vec::new() });
                self.pages.len() - 1
            }
        };
        PageMut { panel: self, page: index }
    }

    /// Обработать сообщения Modly и разослать изменившиеся значения. Вызывать на главном потоке игры.
    pub fn pump(&mut self) {
        self.client.touch();
        let gen_now = self.client.generation();
        if gen_now != self.generation && self.client.is_connected() {
            self.generation = gen_now;
            self.sent.clear();
            self.send_layout();
            for m in &mut self.maps {
                m.dirty = true;
            }
        }
        for msg in self.client.poll() {
            match msg {
                ServerMsg::Welcome { lang, .. } | ServerMsg::Lang { lang } => self.lang = lang,
                ServerMsg::Set { req, id, value } => {
                    let result = match self.controls.get_mut(&id) {
                        Some((_, Binding::Value { set, .. })) => set(value),
                        Some(_) => Err(format!("{id}: нельзя изменить")),
                        None => Err(format!("{id}: нет такого элемента")),
                    };
                    self.reply(req, result.map(|_| None));
                    self.sync_values(true);
                }
                ServerMsg::Invoke { req, id } => {
                    let result = match self.controls.get_mut(&id) {
                        Some((_, Binding::Action(f))) => f(),
                        Some(_) => Err(format!("{id}: это не кнопка")),
                        None => Err(format!("{id}: нет такого элемента")),
                    };
                    self.reply(req, result);
                    self.sync_values(true);
                }
                ServerMsg::MapAction { req, map, action, marker, x, y } => {
                    let hit = MapHit { marker, x, y };
                    let result = match self.maps.iter_mut().find(|m| m.map.id == map) {
                        Some(m) => match m.handlers.get_mut(&action) {
                            Some(f) => f(&hit),
                            None => Err(format!("{map}: нет действия {action}")),
                        },
                        None => Err(format!("нет карты {map}")),
                    };
                    self.reply(req, result);
                    self.sync_values(true);
                    self.sync_maps(true);
                }
                ServerMsg::Ping | ServerMsg::Error { .. } => {}
            }
        }
        self.sync_values(false);
        self.sync_maps(false);
    }

    pub fn notify(&self, tone: Tone, text: impl Into<Text>) {
        self.client.send(&ClientMsg::Notify { tone, text: text.into() });
    }

    pub fn status(&self, tone: Tone, text: impl Into<Text>) {
        self.client.send(&ClientMsg::Status { tone, text: text.into() });
    }

    pub fn log(&self, text: &str) {
        self.client.send(&ClientMsg::Log { tone: Tone::Default, text: text.into() });
    }

    /// Изменить свойства элемента (доступность, подпись, варианты…).
    pub fn patch(&mut self, id: &str, patch: ControlPatch) {
        if let Some((c, _)) = self.controls.get_mut(id) {
            c.apply(&patch);
        }
        self.client.send(&ClientMsg::Patch { controls: [(id.to_owned(), patch)].into_iter().collect() });
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if self.controls.get(id).is_some_and(|(c, _)| c.enabled != enabled) {
            self.patch(id, ControlPatch { enabled: Some(enabled), ..Default::default() });
        }
    }

    fn reply(&self, req: u64, result: Result<Option<String>, String>) {
        let msg = match result {
            Ok(text) => ClientMsg::Result { req, ok: true, text: text.map(Text::from) },
            Err(e) => ClientMsg::Result { req, ok: false, text: Some(Text::from(e)) },
        };
        self.client.send(&msg);
    }

    fn read_value(binding: &mut Binding) -> Option<Value> {
        match binding {
            Binding::Read(get) => Some(get()),
            Binding::Value { get, .. } => Some(get()),
            _ => None,
        }
    }

    fn send_layout(&mut self) {
        let layout = self.layout();
        for p in &layout.pages {
            for s in &p.sections {
                for c in &s.controls {
                    if let Some(v) = &c.value {
                        self.sent.insert(c.id.clone(), v.clone());
                    }
                }
            }
        }
        self.client.send(&ClientMsg::Layout { pages: layout.pages });
        self.last_sync = Instant::now();
    }

    fn sync_values(&mut self, now: bool) {
        if !self.client.is_connected() || (!now && self.last_sync.elapsed() < self.sync_every) {
            return;
        }
        self.last_sync = Instant::now();
        let mut changed = Map::new();
        for (id, (_, binding)) in self.controls.iter_mut() {
            if let Some(v) = Self::read_value(binding) {
                if self.sent.get(id) != Some(&v) {
                    self.sent.insert(id.clone(), v.clone());
                    changed.insert(id.clone(), v);
                }
            }
        }
        if !changed.is_empty() {
            self.client.send(&ClientMsg::State { values: changed });
        }
    }

    /// Весь интерфейс с текущими значениями — в виде протокола.
    pub fn layout(&mut self) -> Layout {
        let controls = &mut self.controls;
        let pages = self
            .pages
            .iter()
            .map(|p| Page {
                id: p.id.clone(),
                title: p.title.clone(),
                icon: p.icon.clone(),
                sections: p
                    .sections
                    .iter()
                    .map(|s| Section {
                        id: s.id.clone(),
                        title: s.title.clone(),
                        description: s.description.clone(),
                        icon: None,
                        controls: s
                            .controls
                            .iter()
                            .filter_map(|id| {
                                let (c, b) = controls.get_mut(id)?;
                                let mut c = c.clone();
                                c.value = Self::read_value(b);
                                Some(c)
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Layout { pages }
    }
}

// =============================================================================================
// Карты

/// Где сработало действие на карте: у метки (`marker` — её id) или по клику в пустое место.
/// `x`, `y` — в координатах игры (тех же, что края карты и метки).
#[derive(Clone, Debug, PartialEq)]
pub struct MapHit {
    pub marker: Option<String>,
    pub x: f64,
    pub y: f64,
}

type MapHandler = Box<dyn FnMut(&MapHit) -> Result<Option<String>, String>>;

/// Живая метка: SDK сам спрашивает её место и присылает Modly, если оно сменилось.
struct LiveMarker {
    marker: MapMarker,
    pos: Box<dyn FnMut() -> (f64, f64)>,
    sent: Option<(f64, f64)>,
}

struct MapDef {
    map: GameMap,
    lives: Vec<LiveMarker>,
    handlers: HashMap<String, MapHandler>,
    /// Прислать карту целиком (новое подключение или изменилось описание).
    dirty: bool,
    set: Vec<MapMarker>,
    remove: Vec<String>,
    last_poll: Instant,
}

/// Метка: `marker("home", 120.0, -40.0).cat("places").title("Home")`.
#[derive(Clone, Debug)]
pub struct MarkerSpec(MapMarker);

pub fn marker(id: &str, x: f64, y: f64) -> MarkerSpec {
    MarkerSpec(MapMarker {
        id: id.into(),
        x,
        y,
        cat: String::new(),
        title: Text::default(),
        text: None,
        icon: None,
        color: None,
        style: MarkerStyle::Pin,
        actions: Vec::new(),
    })
}

impl MarkerSpec {
    pub fn cat(mut self, id: &str) -> Self {
        self.0.cat = id.into();
        self
    }
    pub fn title(mut self, t: impl Into<Text>) -> Self {
        self.0.title = t.into();
        self
    }
    pub fn text(mut self, t: impl Into<Text>) -> Self {
        self.0.text = Some(t.into());
        self
    }
    pub fn icon(mut self, name: &str) -> Self {
        self.0.icon = Some(name.into());
        self
    }
    /// Цвет `#rrggbb` (иначе — цвет категории).
    pub fn color(mut self, hex: &str) -> Self {
        self.0.color = Some(hex.into());
        self
    }
    /// Точка вместо булавки — для того, что движется: игрок, питомец.
    pub fn dot(mut self) -> Self {
        self.0.style = MarkerStyle::Dot;
        self
    }
    /// Кнопка в подписи метки; что делать — [`MapMut::on`].
    pub fn action(mut self, a: ActionSpec) -> Self {
        self.0.actions.push(a.0);
        self
    }
    pub fn at(mut self, x: f64, y: f64) -> Self {
        self.0.x = x;
        self.0.y = y;
        self
    }
}

/// Категория меток: `category("places", "Places").icon("map-pin").color("#8b5cf6")`.
#[derive(Clone, Debug)]
pub struct CategorySpec(MapCategory);

pub fn category(id: &str, label: impl Into<Text>) -> CategorySpec {
    CategorySpec(MapCategory { id: id.into(), label: label.into(), icon: None, color: None, hidden: false })
}

impl CategorySpec {
    pub fn icon(mut self, name: &str) -> Self {
        self.0.icon = Some(name.into());
        self
    }
    pub fn color(mut self, hex: &str) -> Self {
        self.0.color = Some(hex.into());
        self
    }
    /// Скрыта, пока игрок её не включит.
    pub fn hidden(mut self) -> Self {
        self.0.hidden = true;
        self
    }
}

/// Действие на карте: `act("tp", "Teleport").tone(Tone::Accent)`.
#[derive(Clone, Debug)]
pub struct ActionSpec(MapAction);

pub fn act(id: &str, label: impl Into<Text>) -> ActionSpec {
    ActionSpec(MapAction { id: id.into(), label: label.into(), icon: None, tone: Tone::Default, confirm: None })
}

impl ActionSpec {
    pub fn tone(mut self, t: Tone) -> Self {
        self.0.tone = t;
        self
    }
    pub fn icon(mut self, name: &str) -> Self {
        self.0.icon = Some(name.into());
        self
    }
    /// Срабатывать со второго нажатия.
    pub fn confirm(mut self, text: impl Into<Text>) -> Self {
        self.0.confirm = Some(text.into());
        self
    }
}

/// Картинка, вшитая в мод: `data:`-адрес для [`Panel::map`] (до ~3 МБ — предел сообщения 4 МиБ).
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = format!("data:{mime};base64,");
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Настройка карты — цепочкой: `panel.map(...).bounds(...).category(...).marker(...)`.
pub struct MapMut<'a> {
    def: &'a mut MapDef,
}

impl MapMut<'_> {
    /// Края картинки в координатах игры. Без них координаты меток — пиксели картинки.
    /// Если y игры растёт вверх, `top` больше `bottom`.
    pub fn bounds(&mut self, left: f64, top: f64, right: f64, bottom: f64) -> &mut Self {
        self.def.map.bounds = Some(MapBounds { left, top, right, bottom });
        self.def.dirty = true;
        self
    }
    pub fn category(&mut self, c: CategorySpec) -> &mut Self {
        self.def.map.categories.retain(|x| x.id != c.0.id);
        self.def.map.categories.push(c.0);
        self.def.dirty = true;
        self
    }
    /// Метка (та же `id` заменяет прежнюю).
    pub fn marker(&mut self, m: MarkerSpec) -> &mut Self {
        self.def.map.markers.retain(|x| x.id != m.0.id);
        self.def.map.markers.push(m.0);
        self.def.dirty = true;
        self
    }
    /// Живая метка: место берётся из `pos()` и само присылается Modly, когда меняется.
    pub fn live(&mut self, m: MarkerSpec, pos: impl FnMut() -> (f64, f64) + 'static) -> &mut Self {
        self.def.lives.retain(|x| x.marker.id != m.0.id);
        self.def.lives.push(LiveMarker { marker: m.0, pos: Box::new(pos), sent: None });
        self.def.dirty = true;
        self
    }
    /// Действие по клику в пустое место карты («Телепорт сюда»); что делать — [`MapMut::on`].
    pub fn click_action(&mut self, a: ActionSpec) -> &mut Self {
        self.def.map.click_actions.retain(|x| x.id != a.0.id);
        self.def.map.click_actions.push(a.0);
        self.def.dirty = true;
        self
    }
    /// Что делать по действию `id` — у любой метки с этим действием и по клику в пустое место.
    /// `Ok(Some(текст))` Modly покажет уведомлением, `Err` — ошибкой.
    pub fn on(&mut self, id: &str, f: impl FnMut(&MapHit) -> Result<Option<String>, String> + 'static) -> &mut Self {
        self.def.handlers.insert(id.into(), Box::new(f));
        self
    }
    /// Метка, к которой карта подъезжает кнопкой «Где я» (обычно игрок).
    pub fn focus(&mut self, marker_id: &str) -> &mut Self {
        self.def.map.focus = Some(marker_id.into());
        self.def.dirty = true;
        self
    }
}

impl Panel {
    /// Карта мода. `image` — полный путь к PNG, JPEG или WebP на этом компьютере
    /// (`C:\…\map.png`) или [`data_url`]. Повторный вызов с тем же `id` даёт ту же карту.
    pub fn map(&mut self, id: &str, title: impl Into<Text>, image: &str) -> MapMut<'_> {
        let title = title.into();
        let index = match self.maps.iter().position(|m| m.map.id == id) {
            Some(i) => {
                let m = &mut self.maps[i];
                m.map.title = title;
                m.map.image = image.into();
                m.dirty = true;
                i
            }
            None => {
                self.maps.push(MapDef {
                    map: GameMap {
                        id: id.into(),
                        title,
                        image: image.into(),
                        bounds: None,
                        categories: Vec::new(),
                        markers: Vec::new(),
                        click_actions: Vec::new(),
                        focus: None,
                    },
                    lives: Vec::new(),
                    handlers: HashMap::new(),
                    dirty: true,
                    set: Vec::new(),
                    remove: Vec::new(),
                    last_poll: Instant::now(),
                });
                self.maps.len() - 1
            }
        };
        MapMut { def: &mut self.maps[index] }
    }

    /// Добавить или заменить метку на уже показанной карте — Modly получит только её.
    pub fn map_marker(&mut self, map: &str, m: MarkerSpec) {
        if let Some(d) = self.maps.iter_mut().find(|d| d.map.id == map) {
            d.map.markers.retain(|x| x.id != m.0.id);
            d.map.markers.push(m.0.clone());
            d.remove.retain(|x| x != &m.0.id);
            d.set.retain(|x| x.id != m.0.id);
            d.set.push(m.0);
        }
    }

    /// Убрать метку с карты.
    pub fn map_remove_marker(&mut self, map: &str, marker_id: &str) {
        if let Some(d) = self.maps.iter_mut().find(|d| d.map.id == map) {
            d.map.markers.retain(|x| x.id != marker_id);
            d.lives.retain(|x| x.marker.id != marker_id);
            d.set.retain(|x| x.id != marker_id);
            d.remove.push(marker_id.into());
        }
    }

    /// Убрать карту целиком.
    pub fn remove_map(&mut self, id: &str) {
        let before = self.maps.len();
        self.maps.retain(|m| m.map.id != id);
        if self.maps.len() != before {
            self.client.send(&ClientMsg::MapRemove { map: id.into() });
        }
    }

    /// Карта целиком — в виде протокола (живые метки — на текущем месте).
    pub fn map_message(&mut self, id: &str) -> Option<ClientMsg> {
        let d = self.maps.iter_mut().find(|m| m.map.id == id)?;
        Some(ClientMsg::Map(Self::full_map(d)))
    }

    fn full_map(d: &mut MapDef) -> GameMap {
        let mut map = d.map.clone();
        for l in &mut d.lives {
            let (x, y) = (l.pos)();
            l.sent = Some((x, y));
            map.markers.push(MapMarker { x, y, ..l.marker.clone() });
        }
        map
    }

    fn sync_maps(&mut self, now: bool) {
        if !self.client.is_connected() {
            return;
        }
        for d in &mut self.maps {
            if d.dirty {
                d.dirty = false;
                d.set.clear();
                d.remove.clear();
                d.last_poll = Instant::now();
                let map = Self::full_map(d);
                self.client.send(&ClientMsg::Map(map));
                continue;
            }
            if now || d.last_poll.elapsed() >= self.sync_every {
                d.last_poll = Instant::now();
                for l in &mut d.lives {
                    let (x, y) = (l.pos)();
                    if l.sent != Some((x, y)) {
                        l.sent = Some((x, y));
                        d.set.push(MapMarker { x, y, ..l.marker.clone() });
                    }
                }
            }
            if !d.set.is_empty() || !d.remove.is_empty() {
                let msg = ClientMsg::MapPatch { map: d.map.id.clone(), set: std::mem::take(&mut d.set), remove: std::mem::take(&mut d.remove) };
                self.client.send(&msg);
            }
        }
    }
}

pub struct PageMut<'a> {
    panel: &'a mut Panel,
    page: usize,
}

impl<'a> PageMut<'a> {
    pub fn section(self, id: &str, title: impl Into<Text>) -> SectionMut<'a> {
        let title = title.into();
        let page = &mut self.panel.pages[self.page];
        let index = match page.sections.iter().position(|s| s.id == id) {
            Some(i) => i,
            None => {
                page.sections.push(SectionDef {
                    id: id.into(),
                    title: (!title.is_empty()).then_some(title),
                    description: None,
                    controls: Vec::new(),
                });
                page.sections.len() - 1
            }
        };
        SectionMut { panel: self.panel, page: self.page, section: index }
    }
}

pub struct SectionMut<'a> {
    panel: &'a mut Panel,
    page: usize,
    section: usize,
}

impl SectionMut<'_> {
    pub fn describe(&mut self, text: impl Into<Text>) -> &mut Self {
        self.panel.pages[self.page].sections[self.section].description = Some(text.into());
        self
    }

    pub fn add(&mut self, spec: Spec, binding: Binding) -> &mut Self {
        let id = spec.control.id.clone();
        let list = &mut self.panel.pages[self.page].sections[self.section].controls;
        if !list.contains(&id) {
            list.push(id.clone());
        }
        self.panel.controls.insert(id, (spec.control, binding));
        self
    }
}

/// Сообщение `state` вручную (если не пользуетесь привязками).
pub fn values(pairs: &[(&str, Value)]) -> ClientMsg {
    ClientMsg::State { values: pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn builds_layout_with_bindings() {
        // канал, которого нет: клиент просто ждёт Modly в фоне
        let god = Rc::new(RefCell::new(false));
        let mut panel = Panel::with_client(Client::start_on(r"\\.\pipe\modly-sdk-test-none", ModInfo::new("t", "T", "1"), None));
        {
            let (g1, g2) = (god.clone(), god.clone());
            panel
                .page("main", "Main", Some("star"))
                .section("s", "Player")
                .add(toggle("god", "God").icon("shield"), value(move || *g1.borrow(), move |v| *g2.borrow_mut() = v))
                .add(button("heal", "Heal").tone(Tone::Accent).confirm("Sure?"), action(|| {}))
                .add(slider("speed", "Speed", 0.0, 5.0).step(0.5).unit("x"), Binding::None);
        }
        let layout = panel.layout();
        assert_eq!(layout.pages.len(), 1);
        let controls = &layout.pages[0].sections[0].controls;
        assert_eq!(controls.len(), 3);
        assert!(matches!(controls[1].kind, ControlKind::Button { tone: Tone::Accent, confirm: Some(_) }));

        // set через привязку
        if let Some((_, Binding::Value { set, .. })) = panel.controls.get_mut("god") {
            set(json!(true)).unwrap();
            assert!(set(json!("nope")).is_err());
        }
        assert!(*god.borrow());
    }

    #[test]
    fn maps_with_live_markers_and_actions() {
        let mut panel = Panel::with_client(Client::start_on(r"\\.\pipe\modly-sdk-test-none", ModInfo::new("t", "T", "1"), None));
        let player = Rc::new(RefCell::new((10.0, 20.0)));
        let tp = Rc::new(RefCell::new(None));
        {
            let (p, t) = (player.clone(), tp.clone());
            panel
                .map("world", "World", r"C:\maps\world.png")
                .bounds(-100.0, 100.0, 100.0, -100.0)
                .category(category("places", "Places").icon("map-pin").color("#8b5cf6"))
                .marker(marker("home", 1.0, 2.0).cat("places").title("Home").action(act("tp", "Teleport")))
                .live(marker("player", 0.0, 0.0).dot().title("You"), move || *p.borrow())
                .click_action(act("tp", "Teleport here").confirm("Sure?"))
                .on("tp", move |hit| {
                    *t.borrow_mut() = Some(hit.clone());
                    Ok(Some("ok".into()))
                })
                .focus("player");
        }
        let Some(ClientMsg::Map(m)) = panel.map_message("world") else { panic!() };
        assert_eq!(m.markers.len(), 2);
        let me = m.markers.iter().find(|x| x.id == "player").unwrap();
        assert_eq!((me.x, me.y, me.style), (10.0, 20.0, MarkerStyle::Dot), "живая метка — на текущем месте");
        assert_eq!(m.click_actions[0].confirm, Some(Text::from("Sure?")));
        // обработчик действия
        let hit = MapHit { marker: None, x: 5.0, y: -5.0 };
        let r = (panel.maps[0].handlers.get_mut("tp").unwrap())(&hit);
        assert_eq!(r, Ok(Some("ok".into())));
        assert_eq!(*tp.borrow(), Some(hit));
        // метку убрали — её нет и в карте
        panel.map_remove_marker("world", "home");
        let Some(ClientMsg::Map(m)) = panel.map_message("world") else { panic!() };
        assert!(m.markers.iter().all(|x| x.id != "home"));
        assert!(data_url("image/png", b"hi").ends_with("base64,aGk="));
    }
}
