//! Глобальные горячие клавиши: срабатывают, даже когда активно окно игры.
//!
//! [`Hotkey`] — сочетание клавиш, записывается строкой вида `"F1"` или `"Ctrl+Num1"` (так же их
//! предлагают моды в поле `hotkey`). [`Hotkeys`] держит отдельный поток: он регистрирует сочетания
//! через `RegisterHotKey` и ждёт `WM_HOTKEY` в цикле `GetMessage`. Новый набор клавиш передаётся
//! потоку через `PostThreadMessage`, нажатия и ошибки приходят обратно событиями [`HotkeyEvent`].

use std::fmt;

/// Модификаторы — те же биты, что у `RegisterHotKey` (`MOD_ALT`, `MOD_CONTROL`, `MOD_SHIFT`, `MOD_WIN`).
pub const ALT: u8 = 1;
pub const CTRL: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;

/// Сочетание клавиш: модификаторы + виртуальный код клавиши Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hotkey {
    pub mods: u8,
    pub vk: u16,
}

/// Имена клавиш, которые нельзя записать одной буквой или цифрой. Первое имя — основное.
const NAMES: &[(u16, &[&str])] = &[
    (0x08, &["Backspace"]),
    (0x09, &["Tab"]),
    (0x0D, &["Enter", "Return"]),
    (0x13, &["Pause", "Break"]),
    (0x14, &["CapsLock"]),
    (0x1B, &["Esc", "Escape"]),
    (0x20, &["Space"]),
    (0x21, &["PageUp", "PgUp"]),
    (0x22, &["PageDown", "PgDn"]),
    (0x23, &["End"]),
    (0x24, &["Home"]),
    (0x25, &["Left"]),
    (0x26, &["Up"]),
    (0x27, &["Right"]),
    (0x28, &["Down"]),
    (0x2C, &["PrintScreen", "PrtSc"]),
    (0x2D, &["Insert", "Ins"]),
    (0x2E, &["Delete", "Del"]),
    (0x6A, &["Num*", "NumMultiply"]),
    (0x6B, &["Num+", "NumAdd", "NumPlus"]),
    (0x6D, &["Num-", "NumSubtract", "NumMinus"]),
    (0x6E, &["Num.", "NumDecimal", "NumDel"]),
    (0x6F, &["Num/", "NumDivide"]),
    (0x90, &["NumLock"]),
    (0x91, &["ScrollLock"]),
    (0xBA, &[";"]),
    (0xBB, &["="]),
    (0xBC, &[","]),
    (0xBD, &["-"]),
    (0xBE, &["."]),
    (0xBF, &["/"]),
    (0xC0, &["`", "~"]),
    (0xDB, &["["]),
    (0xDC, &["\\"]),
    (0xDD, &["]"]),
    (0xDE, &["'"]),
];

/// Клавиши-модификаторы сами по себе горячей клавишей быть не могут.
fn is_modifier_vk(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Имя клавиши без модификаторов: `F1`, `A`, `7`, `Num1`, `PageUp`.
pub fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        0x60..=0x69 => Some(format!("Num{}", vk - 0x60)),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        _ => NAMES.iter().find(|(v, _)| *v == vk).map(|(_, n)| n[0].to_owned()),
    }
}

fn key_from_name(name: &str) -> Option<u16> {
    let upper = name.trim().to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_uppercase() || bytes[0].is_ascii_digit()) {
        return Some(bytes[0] as u16);
    }
    for prefix in ["NUMPAD", "NUM"] {
        if let Some(d) = upper.strip_prefix(prefix).map(str::trim) {
            if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() {
                return Some(0x60 + (d.as_bytes()[0] - b'0') as u16);
            }
        }
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&n) && !upper[1..].starts_with('0') {
            return Some(0x6F + n);
        }
    }
    NAMES.iter().find(|(_, names)| names.iter().any(|n| n.eq_ignore_ascii_case(name.trim()))).map(|(vk, _)| *vk)
}

impl Hotkey {
    pub fn new(mods: u8, vk: u16) -> Option<Hotkey> {
        (key_name(vk).is_some() && !is_modifier_vk(vk)).then_some(Hotkey { mods: mods & (ALT | CTRL | SHIFT | WIN), vk })
    }

    /// Разобрать `"Ctrl+Shift+F1"`, `"num1"`, `"Alt + Num+"`. Регистр и пробелы вокруг `+` не важны.
    pub fn parse(s: &str) -> Option<Hotkey> {
        let mut rest = s.trim();
        let mut mods = 0;
        // модификаторы снимаем с начала строки: так клавиша `Num+` не путается с разделителем
        while let Some((head, tail)) = rest.split_once('+') {
            let bit = match head.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => CTRL,
                "alt" => ALT,
                "shift" => SHIFT,
                "win" | "meta" | "super" => WIN,
                _ => break,
            };
            mods |= bit;
            rest = tail.trim_start();
        }
        if rest.is_empty() {
            return None;
        }
        Hotkey::new(mods, key_from_name(rest)?)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (bit, name) in [(CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift"), (WIN, "Win")] {
            if self.mods & bit != 0 {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.vk).unwrap_or_else(|| format!("0x{:02X}", self.vk)))
    }
}

impl std::str::FromStr for Hotkey {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Hotkey::parse(s).ok_or(())
    }
}

/// Код клавиши по тексту нажатия из интерфейса (Slint): служебные символы Slint для F1…F24,
/// стрелок и т. п., остальное — через текущую раскладку (`ф` → `A`).
pub fn vk_from_key_text(text: &str) -> Option<u16> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let vk = match c {
        '\u{8}' => 0x08,
        '\t' => 0x09,
        '\n' | '\r' => 0x0D,
        '\u{1b}' => 0x1B,
        ' ' => 0x20,
        '\u{7f}' => 0x2E,
        '\u{F700}' => 0x26,
        '\u{F701}' => 0x28,
        '\u{F702}' => 0x25,
        '\u{F703}' => 0x27,
        '\u{F704}'..='\u{F71B}' => 0x70 + (c as u32 - 0xF704) as u16,
        '\u{F727}' => 0x2D,
        '\u{F729}' => 0x24,
        '\u{F72B}' => 0x23,
        '\u{F72C}' => 0x21,
        '\u{F72D}' => 0x22,
        '\u{F72F}' => 0x91,
        '\u{F730}' => 0x13,
        '\u{F731}' => 0x2C,
        // Ctrl+буква иногда приходит управляющим символом
        '\u{1}'..='\u{1a}' => 0x40 + c as u16,
        'a'..='z' | 'A'..='Z' | '0'..='9' => c.to_ascii_uppercase() as u16,
        _ => return vk_for_char(c),
    };
    Some(vk)
}

#[cfg(windows)]
fn vk_for_char(c: char) -> Option<u16> {
    let mut buf = [0u16; 2];
    let units = c.encode_utf16(&mut buf);
    if units.len() != 1 {
        return None;
    }
    let r = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(units[0]) };
    (r != -1).then_some((r as u16) & 0xFF)
}

#[cfg(not(windows))]
fn vk_for_char(_c: char) -> Option<u16> {
    None
}

/// Какая клавиша (не модификатор) нажата прямо сейчас — по состоянию клавиатуры потока, который
/// обрабатывает нажатие. Только так можно отличить `Num1` от `1`: интерфейс видит в обоих случаях «1».
/// Если нажато несколько, берётся та, что совпадает с `hint`.
#[cfg(windows)]
pub fn held_key(hint: Option<u16>) -> Option<u16> {
    let mut state = [0u8; 256];
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardState(&mut state).ok()? };
    let down: Vec<u16> = (0x08u16..0xFF)
        .filter(|&vk| state[vk as usize] & 0x80 != 0 && !is_modifier_vk(vk) && key_name(vk).is_some())
        .collect();
    if let Some(h) = hint {
        if down.contains(&h) {
            return Some(h);
        }
        // цифра основного ряда и та же цифра на цифровом блоке
        if let Some(&num) = down.iter().find(|&&vk| (0x60..=0x69).contains(&vk) && vk - 0x30 == h) {
            return Some(num);
        }
        if down.len() != 1 {
            return Some(h);
        }
    }
    down.first().copied()
}

#[cfg(not(windows))]
pub fn held_key(hint: Option<u16>) -> Option<u16> {
    hint
}

/// Сочетание, которое пользователь нажал в окне Modly (режим «назначить клавишу»).
pub fn captured(text: &str, ctrl: bool, alt: bool, shift: bool, win: bool) -> Option<Hotkey> {
    let hint = vk_from_key_text(text);
    let vk = held_key(hint).or(hint)?;
    let mods = if ctrl { CTRL } else { 0 } | if alt { ALT } else { 0 } | if shift { SHIFT } else { 0 } | if win { WIN } else { 0 };
    Hotkey::new(mods, vk)
}

// =============================================================================================
// Поток регистрации

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Нажато зарегистрированное сочетание. `foreground_pid` — процесс активного окна в этот момент.
    Pressed { key: Hotkey, foreground_pid: Option<u32> },
    /// Итог очередной регистрации: эти сочетания занял кто-то другой (другая программа или Windows).
    Failed(Vec<Hotkey>),
}

#[cfg(windows)]
pub use imp::Hotkeys;

#[cfg(windows)]
mod imp {
    use super::{Hotkey, HotkeyEvent};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
        WM_APP, WM_HOTKEY, WM_QUIT,
    };

    /// «Перечитай желаемый набор клавиш».
    const WM_SYNC: u32 = WM_APP + 1;

    #[derive(Default)]
    struct Wanted {
        keys: Vec<Hotkey>,
        /// Пока пользователь назначает клавишу, все сочетания сняты — иначе нажатие не дойдёт до окна.
        suspended: bool,
    }

    /// Поток горячих клавиш. Останавливается, когда `Hotkeys` удаляют.
    pub struct Hotkeys {
        thread: u32,
        wanted: Arc<Mutex<Wanted>>,
    }

    impl Hotkeys {
        pub fn start(on_event: impl Fn(HotkeyEvent) + Send + 'static) -> std::io::Result<Hotkeys> {
            let wanted = Arc::new(Mutex::new(Wanted::default()));
            let shared = wanted.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new().name("modly-hotkeys".into()).spawn(move || {
                // очередь сообщений потока создаётся первым вызовом Peek/GetMessage — до него PostThreadMessage не дойдёт
                let mut msg = MSG::default();
                unsafe {
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                    let _ = tx.send(GetCurrentThreadId());
                }
                run(shared, on_event);
            })?;
            let thread = rx.recv().map_err(|_| std::io::Error::other("hotkey thread died"))?;
            Ok(Hotkeys { thread, wanted })
        }

        /// Зарегистрировать ровно этот набор сочетаний (лишние снимаются).
        pub fn set(&self, keys: Vec<Hotkey>) {
            self.wanted.lock().unwrap().keys = keys;
            self.wake();
        }

        /// Временно снять все сочетания (пока пользователь назначает клавишу) или вернуть их.
        pub fn suspend(&self, on: bool) {
            self.wanted.lock().unwrap().suspended = on;
            self.wake();
        }

        fn wake(&self) {
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_SYNC, WPARAM(0), LPARAM(0));
            }
        }
    }

    impl Drop for Hotkeys {
        fn drop(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn foreground_pid() -> Option<u32> {
        let mut pid = 0u32;
        unsafe {
            let hwnd: HWND = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        (pid != 0).then_some(pid)
    }

    fn run(wanted: Arc<Mutex<Wanted>>, on_event: impl Fn(HotkeyEvent)) {
        // id регистрации → сочетание
        let mut registered: BTreeMap<i32, Hotkey> = BTreeMap::new();
        let mut next_id: i32 = 1;
        let mut last_failed: Vec<Hotkey> = Vec::new();
        let mut msg = MSG::default();
        loop {
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 <= 0 {
                break; // WM_QUIT или ошибка
            }
            match msg.message {
                WM_HOTKEY => {
                    if let Some(key) = registered.get(&(msg.wParam.0 as i32)) {
                        on_event(HotkeyEvent::Pressed { key: *key, foreground_pid: foreground_pid() });
                    }
                }
                WM_SYNC => {
                    let target: Vec<Hotkey> = {
                        let w = wanted.lock().unwrap();
                        if w.suspended { Vec::new() } else { w.keys.clone() }
                    };
                    registered.retain(|id, key| {
                        if target.contains(key) {
                            return true;
                        }
                        unsafe {
                            let _ = UnregisterHotKey(None, *id);
                        }
                        false
                    });
                    let mut failed = Vec::new();
                    for key in target {
                        if registered.values().any(|k| *k == key) {
                            continue;
                        }
                        let id = next_id;
                        next_id = if next_id >= 0xBFFF { 1 } else { next_id + 1 };
                        let mods = HOT_KEY_MODIFIERS(key.mods as u32) | MOD_NOREPEAT;
                        match unsafe { RegisterHotKey(None, id, mods, key.vk as u32) } {
                            Ok(()) => {
                                registered.insert(id, key);
                            }
                            Err(e) => {
                                log::warn!("hotkey {key}: {e}");
                                failed.push(key);
                            }
                        }
                    }
                    let suspended = wanted.lock().unwrap().suspended;
                    if !suspended && failed != last_failed {
                        last_failed = failed.clone();
                        on_event(HotkeyEvent::Failed(failed));
                    }
                }
                _ => {}
            }
        }
        for id in registered.keys() {
            unsafe {
                let _ = UnregisterHotKey(None, *id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        let k = Hotkey::parse("Ctrl+Num1").unwrap();
        assert_eq!(k, Hotkey { mods: CTRL, vk: 0x61 });
        assert_eq!(k.to_string(), "Ctrl+Num1");
        assert_eq!(Hotkey::parse("F1").unwrap().to_string(), "F1");
        assert_eq!(Hotkey::parse("f24").unwrap().vk, 0x87);
        assert_eq!(Hotkey::parse(" shift + alt + ctrl + k ").unwrap().to_string(), "Ctrl+Alt+Shift+K");
        assert_eq!(Hotkey::parse("Alt+Num+").unwrap(), Hotkey { mods: ALT, vk: 0x6B });
        assert_eq!(Hotkey::parse("NumPad7").unwrap().to_string(), "Num7");
        assert_eq!(Hotkey::parse("Ctrl+PgUp").unwrap().to_string(), "Ctrl+PageUp");
        assert_eq!(Hotkey::parse("Ctrl+=").unwrap().vk, 0xBB);
        assert_eq!(Hotkey::parse("5").unwrap().vk, 0x35);
    }

    #[test]
    fn parse_rejects_garbage() {
        for bad in ["", "Ctrl+", "Ctrl", "F0", "F25", "F01", "Hyper+F1", "Ctrl+Banana", "Shift+Shift"] {
            assert_eq!(Hotkey::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Hotkey::new(CTRL, 0x11), None); // одна клавиша Ctrl
    }

    #[test]
    fn every_name_round_trips() {
        for vk in 0u16..=0xFF {
            if let Some(k) = Hotkey::new(CTRL | SHIFT, vk) {
                assert_eq!(Hotkey::parse(&k.to_string()), Some(k), "{k}");
            }
        }
    }

    #[test]
    fn key_text_from_ui() {
        assert_eq!(vk_from_key_text("\u{F704}"), Some(0x70)); // F1
        assert_eq!(vk_from_key_text("\u{F71B}"), Some(0x87)); // F24
        assert_eq!(vk_from_key_text("g"), Some(0x47));
        assert_eq!(vk_from_key_text("7"), Some(0x37));
        assert_eq!(vk_from_key_text("\u{1}"), Some(0x41)); // Ctrl+A
        assert_eq!(vk_from_key_text("ab"), None);
    }

    #[cfg(windows)]
    #[test]
    fn thread_registers_and_releases() {
        use std::sync::mpsc;
        use std::time::Duration;
        let (tx, rx) = mpsc::channel();
        let hk = Hotkeys::start(move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        // редкое сочетание, чтобы не задеть ничего у пользователя
        let key = Hotkey::parse("Ctrl+Alt+Shift+F23").unwrap();
        hk.set(vec![key]);
        hk.set(vec![]);
        hk.set(vec![key]);
        // ошибок регистрации нет — событие Failed не приходит
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        // второй поток не может занять то же сочетание
        let (tx2, rx2) = mpsc::channel();
        let other = Hotkeys::start(move |e| {
            let _ = tx2.send(e);
        })
        .unwrap();
        other.set(vec![key]);
        assert_eq!(rx2.recv_timeout(Duration::from_secs(2)).unwrap(), HotkeyEvent::Failed(vec![key]));
        // первый отпустил — второй при следующей попытке смог
        hk.suspend(true);
        std::thread::sleep(Duration::from_millis(100));
        other.set(vec![key]);
        assert_eq!(rx2.recv_timeout(Duration::from_secs(2)).unwrap(), HotkeyEvent::Failed(vec![]));
    }
}
