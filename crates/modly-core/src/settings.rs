use crate::hotkeys::Hotkey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "" — как в Windows, "ru" или "en".
    pub language: String,
    /// Дополнительные папки с пакетами модов (к папкам по умолчанию).
    pub mod_dirs: Vec<PathBuf>,
    /// Дополнительные папки, где искать игры.
    pub scan_roots: Vec<PathBuf>,
    /// Режим разработчика: консоль протокола и сборка модов из исходников.
    pub developer_mode: bool,
    /// Рисовать интерфейс видеокартой (OpenGL). По умолчанию — программно: в разы меньше памяти.
    pub gpu_rendering: bool,
    /// Игры уже искали при первом запуске.
    pub first_scan_done: bool,
    /// Последняя открытая игра.
    pub last_game: String,
    /// Горячие клавиши, назначенные пользователем: id мода → id элемента → сочетание (`"Ctrl+F1"`).
    /// Пустая строка — клавишу сняли, подсказку мода (`hotkey` в интерфейсе) не использовать.
    pub hotkeys: BTreeMap<String, BTreeMap<String, String>>,
    /// Игры Steam, которые человек убрал из библиотеки: поиск игр их не возвращает.
    pub hidden_steam: Vec<u32>,
    /// Связка с WeMod: на странице игры — кнопка «Открыть в WeMod» (трейнеры WeMod работают в самом WeMod).
    pub wemod: bool,
    /// Клавиша оверлея в игре: "" — по умолчанию (Ctrl+Shift+O), "off" — оверлей выключен.
    pub overlay_hotkey: String,
    /// Пульт с телефона: веб-сервер в домашней сети (выключен, пока человек его не включит).
    pub remote: bool,
    /// Канал обновлений: "" — как у текущей версии (бета получает беты), "stable" или "beta".
    pub update_channel: String,
    /// Не проверять обновления самому (по умолчанию Modly проверяет при запуске и раз в 6 часов).
    pub no_update_check: bool,
    /// Игры не из Steam, которые человек убрал из библиотеки (id записи): поиск их не возвращает.
    pub hidden_games: Vec<String>,
    /// Какой версией поиска игр уже искали: новая версия поиска ищет ещё раз сама.
    pub scan_version: u32,
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(crate::paths::settings_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let data = serde_json::to_vec_pretty(self).expect("settings serialize");
        if let Err(e) = crate::write_atomic(&crate::paths::settings_file(), &data) {
            log::warn!("settings save failed: {e}");
        }
    }

    /// Горячая клавиша элемента: назначенная пользователем, а если он ничего не назначал — предложенная модом.
    pub fn hotkey(&self, mod_id: &str, control_id: &str, suggested: Option<&str>) -> Option<Hotkey> {
        match self.hotkeys.get(mod_id).and_then(|m| m.get(control_id)) {
            Some(own) => Hotkey::parse(own),
            None => suggested.and_then(Hotkey::parse),
        }
    }

    /// Назначить клавишу элементу; `None` — снять (и не возвращаться к подсказке мода).
    pub fn set_hotkey(&mut self, mod_id: &str, control_id: &str, key: Option<Hotkey>) {
        let value = key.map(|k| k.to_string()).unwrap_or_default();
        self.hotkeys.entry(mod_id.to_owned()).or_default().insert(control_id.to_owned(), value);
    }

    /// Язык интерфейса с учётом "как в Windows".
    pub fn effective_language(&self) -> &'static str {
        match self.language.as_str() {
            "ru" => "ru",
            "en" => "en",
            _ => system_language(),
        }
    }
}

/// Русский для русской, украинской, белорусской и казахской Windows, иначе английский.
pub fn system_language() -> &'static str {
    #[cfg(windows)]
    {
        let primary = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3FF;
        if matches!(primary, 0x19 | 0x22 | 0x23 | 0x3F) {
            return "ru";
        }
    }
    "en"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_fall_back_to_mod_suggestion() {
        let mut s = Settings::default();
        assert_eq!(s.hotkey("demo", "god", Some("F1")), Hotkey::parse("F1"));
        assert_eq!(s.hotkey("demo", "god", Some("not a key")), None);
        s.set_hotkey("demo", "god", Hotkey::parse("ctrl+num1"));
        assert_eq!(s.hotkey("demo", "god", Some("F1")), Hotkey::parse("Ctrl+Num1"));
        // сняли — подсказка мода больше не действует
        s.set_hotkey("demo", "god", None);
        assert_eq!(s.hotkey("demo", "god", Some("F1")), None);

        s.set_hotkey("demo", "heal", Hotkey::parse("F5"));
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""hotkeys":{"demo":{"god":"","heal":"F5"}}"#), "{json}");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hotkey("demo", "heal", None), Hotkey::parse("F5"));
        // старые настройки без поля читаются
        let old: Settings = serde_json::from_str(r#"{"language":"ru"}"#).unwrap();
        assert!(old.hotkeys.is_empty());
    }
}
