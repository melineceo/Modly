//! Оверлей в игре: панель мода поверх игры по горячей клавише (по умолчанию Ctrl+Shift+O).
//!
//! Это второе окно Modly (`OverlayWindow`, ui/overlay.slint) — без рамки, поверх всех окон, у правого
//! края окна игры. Фокус у игры оно не забирает: игра продолжает идти, а клики по оверлею работают.
//! Работает в оконном и «полноэкранном оконном» режимах; поверх эксклюзивного полноэкранного режима
//! Windows не даёт рисовать никому (оверлей Steam и WeMod тоже рисуются иначе).
//!
//! Модели панели (вкладки, страницы, секции) у обоих окон общие; простые свойства (заголовок,
//! состояние, текущая вкладка) копируются в оверлей после каждого действия — `sync_overlay`.

use super::{App, with};
use crate::{Lang, OverlayWindow, Store};
use modly_core::hotkeys::Hotkey;
use slint::{ComponentHandle, ModelRc};

/// Клавиша оверлея по умолчанию.
pub const DEFAULT_KEY: &str = "Ctrl+Shift+O";

#[derive(Default)]
pub struct Overlay {
    window: Option<OverlayWindow>,
    visible: bool,
}

impl App {
    /// Клавиша оверлея из настроек: "" — по умолчанию, "off" — выключен.
    pub(super) fn overlay_key(&self) -> Option<Hotkey> {
        match self.settings.overlay_hotkey.as_str() {
            "off" => None,
            "" => Hotkey::parse(DEFAULT_KEY),
            k => Hotkey::parse(k),
        }
    }

    fn overlay_window(&mut self) -> Option<OverlayWindow> {
        if self.overlay.window.is_none() {
            let w = OverlayWindow::new().map_err(|e| log::warn!("overlay: {e}")).ok()?;
            let s = w.global::<Store>();
            s.set_live_tabs(ModelRc::from(self.m_live_tabs.clone()));
            s.set_live_pages(ModelRc::from(self.m_live_pages.clone()));
            s.set_live_sections(ModelRc::from(self.m_live_sections.clone()));
            super::maps::attach(&s, &self.maps);
            super::wire_callbacks(&s);
            // карта из оверлея открывается в нём же, главное окно остаётся на своей странице
            s.on_open_map(|slug| with(move |a| a.open_map(&slug, true)));
            w.on_hide_overlay(|| with(|a| a.hide_overlay()));
            self.overlay.window = Some(w);
        }
        self.overlay.window.as_ref().map(|w| w.clone_strong())
    }

    /// Горячая клавиша оверлея: показать поверх игры на переднем плане или спрятать.
    pub(super) fn toggle_overlay(&mut self, foreground_pid: Option<u32>) {
        if self.overlay.visible {
            self.hide_overlay();
        } else {
            self.show_overlay(foreground_pid);
        }
    }

    fn show_overlay(&mut self, foreground_pid: Option<u32>) {
        // Панель — того мода, чья игра сейчас на экране. Если игра на экране не наша (или мод
        // не узнан), а у открытой в Modly игры модов нет — берём любой подключённый мод.
        let in_front = foreground_pid.and_then(|pid| self.sessions.values().find(|s| s.hello.pid == Some(pid)));
        let front_is_game = in_front.is_some();
        let pick = in_front
            .or_else(|| {
                let current_has = self.sessions.values().any(|s| s.game_id.is_some() && s.game_id == self.current_game);
                if current_has { None } else { self.sessions.values().find(|s| s.game_id.is_some()) }
            })
            .map(|s| (s.id, s.game_id.clone()));
        if let Some((sid, Some(game))) = pick {
            if self.current_game.as_deref() != Some(game.as_str()) || self.live_session != Some(sid) {
                self.current_game = Some(game);
                self.live_session = Some(sid);
                self.live_page = 0;
                self.refresh_game_page();
            }
        }
        let Some(w) = self.overlay_window() else { return };
        let scale = w.window().scale_factor();
        let px = |v: f32| (v * scale).round() as i32;
        // у правого края окна игры, если игра на переднем плане; иначе — у края экрана
        #[cfg(windows)]
        let area = {
            use modly_core::win;
            match win::foreground_window() {
                Some((hwnd, pid, rect)) if front_is_game && Some(pid) == foreground_pid && rect.width() > px(500.0) && rect.height() > px(400.0) => {
                    let work = win::work_area(hwnd);
                    // окно игры может выходить за край экрана (рамка, полноэкранный режим) — обрезаем
                    win::Rect {
                        left: rect.left.max(work.left),
                        top: rect.top.max(work.top),
                        right: rect.right.min(work.right),
                        bottom: rect.bottom.min(work.bottom),
                    }
                }
                _ => win::work_area(0),
            }
        };
        #[cfg(not(windows))]
        let area = modly_core::win::Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        let width = px(440.0).min(area.width() - px(20.0)).max(px(320.0));
        let height = (area.height() - px(48.0)).clamp(px(360.0), px(880.0));
        let x = area.right - width - px(16.0);
        let y = area.top + ((area.height() - height) / 2).max(px(12.0));
        w.window().set_position(slint::PhysicalPosition::new(x, y));
        w.window().set_size(slint::PhysicalSize::new(width as u32, height as u32));
        w.set_hotkey(self.overlay_key().map(|k| k.to_string()).unwrap_or_default().into());
        self.overlay.visible = true;
        self.sync_overlay();
        if let Err(e) = w.show() {
            log::warn!("overlay show: {e}");
            self.overlay.visible = false;
            return;
        }
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = w.window().window_handle().window_handle() {
                if let RawWindowHandle::Win32(h) = h.as_raw() {
                    modly_core::win::make_overlay(h.hwnd.get());
                }
            }
        }
    }

    /// Карту открыли из оверлея — показать её там (вкладка «Карта»).
    pub(super) fn overlay_show_map(&mut self) {
        if let Some(w) = &self.overlay.window {
            w.set_tab("map".into());
            w.set_map_view(true);
        }
    }

    pub(super) fn hide_overlay(&mut self) {
        if let Some(w) = &self.overlay.window {
            let _ = w.hide();
        }
        self.overlay.visible = false;
    }

    /// Скопировать в оверлей то, что панель показывает в главном окне (только пока оверлей открыт).
    pub(super) fn sync_overlay(&self) {
        if !self.overlay.visible {
            return;
        }
        let (Some(ui), Some(w)) = (self.ui(), self.overlay.window.as_ref()) else { return };
        let (a, b) = (ui.global::<Store>(), w.global::<Store>());
        macro_rules! copy {
            ($($get:ident => $set:ident),* $(,)?) => {
                $( let v = a.$get(); if b.$get() != v { b.$set(v); } )*
            };
        }
        copy!(
            get_live_tab => set_live_tab,
            get_live_connected => set_live_connected,
            get_live_title => set_live_title,
            get_live_subtitle => set_live_subtitle,
            get_live_status => set_live_status,
            get_live_status_tone => set_live_status_tone,
            get_live_paused => set_live_paused,
            get_live_page => set_live_page,
            get_hotkey_capture => set_hotkey_capture,
            get_developer_mode => set_developer_mode,
            get_map_key => set_map_key,
            get_map_live => set_map_live,
            get_map_focus_index => set_map_focus_index,
            get_map_title => set_map_title,
            get_map_subtitle => set_map_subtitle,
            get_map_state => set_map_state,
            get_map_error => set_map_error,
            get_map_image => set_map_image,
            get_map_ratio => set_map_ratio,
            get_map_selected => set_map_selected,
            get_map_query => set_map_query,
        );
        let english = ui.global::<Lang>().get_english();
        if w.global::<Lang>().get_english() != english {
            w.global::<Lang>().set_english(english);
        }
    }
}
