//! Пульт с телефона: сервер из `modly_core::remote` ↔ панель мода в Modly.
//!
//! Телефон видит ту же панель, что открыта в Modly (или в оверлее): снимок строится на потоке
//! интерфейса из тех же моделей, а команды идут через те же `live_*`, что и клики мышью. Нажать с
//! телефона можно только то, что сейчас на экране и включено.

use super::{App, post, with};
use crate::{LiveControl, Store};
use modly_core::remote::{self, Command, Panel};
use modly_protocol::{ControlKind, Tone};
use serde_json::{Value, json};
use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer};
use std::sync::Arc;

const ASSETS: remote::Assets = remote::Assets {
    html: include_str!("../../remote/index.html"),
    css: include_str!("../../remote/remote.css"),
    js: include_str!("../../remote/remote.js"),
    icon: include_str!("../../ui/icons/logo.svg"),
};

#[derive(Default)]
pub struct Remote {
    server: Option<remote::Server>,
    /// Адрес, для которого нарисован QR-код (IP мог смениться — тогда рисуем заново).
    url: String,
}

/// Мост сервер → приложение. Сервер живёт в потоках tokio, приложение — на потоке интерфейса.
struct Bridge;

impl Panel for Bridge {
    fn snapshot(&self) -> tokio::sync::oneshot::Receiver<Value> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        post(move |a| {
            let _ = tx.send(a.remote_snapshot());
        });
        rx
    }

    fn command(&self, cmd: Command) {
        post(move |a| a.remote_command(cmd));
    }
}

pub(super) fn wire(s: &Store) {
    s.on_set_remote(|on| with(move |a| a.set_remote(on)));
    s.on_reset_remote(|| with(|a| a.reset_remote()));
}

impl App {
    fn set_remote(&mut self, on: bool) {
        self.settings.remote = on;
        self.settings.save();
        self.sync_remote();
    }

    /// Запустить или остановить сервер по настройке и показать адрес с QR-кодом.
    pub(super) fn sync_remote(&mut self) {
        if !self.settings.remote {
            self.remote.server = None; // Drop останавливает сервер
            self.remote.url.clear();
            self.remote_ui(String::new(), String::new());
            return;
        }
        if self.remote.server.is_none() {
            let started = remote::load_or_create_key(false)
                .and_then(|(key, port)| remote::start(&self.rt, key, port, Arc::new(Bridge), ASSETS));
            match started {
                Ok(server) => self.remote.server = Some(server),
                Err(e) => {
                    log::warn!("remote: {e:#}");
                    let text = format!("{}: {e:#}", self.t("Пульт не запустился", "The remote didn't start"));
                    self.remote_ui(String::new(), text);
                    return;
                }
            }
        }
        self.refresh_remote_url(false);
    }

    /// Новый ключ: все телефоны отвязываются, новый QR-код подключает заново.
    fn reset_remote(&mut self) {
        let Some(server) = &self.remote.server else { return };
        match remote::load_or_create_key(true) {
            Ok((key, _)) => {
                server.set_key(key);
                self.refresh_remote_url(true);
                let text = self.t("Доступ сброшен: старые ссылки больше не работают.", "Access reset: old links no longer work.");
                self.toast(Tone::Success, "", &text);
            }
            Err(e) => {
                let title = self.t("Не удалось сбросить доступ", "Couldn't reset access");
                self.toast(Tone::Danger, &title, &format!("{e:#}"));
            }
        }
    }

    /// Адрес для телефона: IP этого компьютера в сети, порт и ключ после `#`.
    pub(super) fn refresh_remote_url(&mut self, force: bool) {
        let Some(server) = &self.remote.server else { return };
        let port = server.port;
        let Some(ip) = remote::lan_ip() else {
            self.remote.url.clear();
            let text = self.t(
                "Компьютер не подключён к сети. Подключите его к Wi-Fi или кабелю — телефон должен быть в той же сети.",
                "This PC isn't on a network. Connect it to Wi-Fi or a cable — the phone must be on the same network.",
            );
            self.remote_ui(String::new(), text);
            return;
        };
        let key = match remote::load_or_create_key(false) {
            Ok((key, _)) => key,
            Err(e) => return self.remote_ui(String::new(), format!("{e:#}")),
        };
        let host = match ip {
            std::net::IpAddr::V6(v6) => format!("[{v6}]"),
            v4 => v4.to_string(),
        };
        let url = format!("http://{host}:{port}/#k={key}");
        if url != self.remote.url || force {
            self.remote.url = url.clone();
            self.remote_ui(url, String::new());
        }
    }

    fn remote_ui(&self, url: String, error: String) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        s.set_remote(self.settings.remote);
        s.set_remote_error(error.into());
        s.set_remote_qr(if url.is_empty() { Image::default() } else { qr_image(&url) });
        s.set_remote_url(url.as_str().into());
    }

    // -----------------------------------------------------------------------------------------
    // Снимок и команды

    /// Если в Modly открыта игра без модов (или никакая), а мод подключён к другой игре — пульт
    /// показывает его. Страницу игры, которую человек сейчас смотрит, не трогаем.
    fn follow_running_mod(&mut self) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        if s.get_live_connected() || s.get_page() == "game" {
            return;
        }
        let pick = self.sessions.values().find(|s| s.game_id.is_some()).map(|s| (s.id, s.game_id.clone()));
        if let Some((sid, Some(game))) = pick {
            self.current_game = Some(game);
            self.live_session = Some(sid);
            self.live_page = 0;
            self.refresh_game_page();
        }
    }

    fn remote_snapshot(&mut self) -> Value {
        self.follow_running_mod();
        let Some(ui) = self.ui() else { return Value::Null };
        let s = ui.global::<Store>();
        let pages: Vec<Value> = self.m_live_pages.iter().map(|p| json!({ "title": p.title.as_str() })).collect();
        let sections: Vec<Value> = self
            .m_live_sections
            .iter()
            .map(|sec| {
                json!({
                    "title": sec.title.as_str(),
                    "description": sec.description.as_str(),
                    "controls": sec.controls.iter().map(|c| control_json(&c)).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "lang": self.lang(),
            "connected": s.get_live_connected(),
            "title": s.get_live_title().as_str(),
            "subtitle": s.get_live_subtitle().as_str(),
            "status": s.get_live_status().as_str(),
            "status_tone": s.get_live_status_tone().as_str(),
            "paused": s.get_live_paused(),
            "page": s.get_live_page(),
            "pages": pages,
            "sections": sections,
        })
    }

    fn remote_command(&mut self, cmd: Command) {
        log::debug!("remote: {cmd:?}");
        if let Command::Page { index } = cmd {
            if (index as usize) < self.m_live_pages.row_count() {
                self.live_select_page(index);
            }
            return;
        }
        let id = match &cmd {
            Command::Set { id, .. } | Command::Number { id, .. } | Command::Pick { id, .. } | Command::Invoke { id } => id.clone(),
            Command::Page { .. } => return,
        };
        // только то, что сейчас на экране, видно и включено
        let Some(control) = self.live_session.and_then(|sid| self.sessions.get(&sid)).and_then(|s| s.control(&id)) else { return };
        if !self.rows.contains_key(&id) || !control.visible || !control.enabled {
            return;
        }
        let kind = control.kind.clone();
        match (cmd, kind) {
            (Command::Set { value: Value::Bool(b), .. }, ControlKind::Toggle) => self.live_set(&id, Value::Bool(b), true),
            (Command::Set { value: Value::String(t), .. }, ControlKind::Text { .. }) => self.live_set(&id, Value::String(t), true),
            (Command::Number { value, .. }, ControlKind::Slider { min, max, .. }) => {
                self.live_number(&id, value.clamp(min.min(max), max.max(min)) as f32)
            }
            (Command::Number { value, .. }, ControlKind::Number { min, max, .. }) => {
                let v = value.max(min.unwrap_or(f64::MIN)).min(max.unwrap_or(f64::MAX));
                self.live_number(&id, v as f32)
            }
            (Command::Pick { option, .. }, ControlKind::Select { options, .. }) if options.iter().any(|o| o.value == option) => {
                self.live_pick(&id, &option)
            }
            (Command::Invoke { .. }, ControlKind::Button { .. }) => self.live_invoke(&id),
            _ => {}
        }
    }
}

fn control_json(c: &LiveControl) -> Value {
    json!({
        "id": c.id.as_str(),
        "kind": c.kind.as_str(),
        "label": c.label.as_str(),
        "description": c.description.as_str(),
        "enabled": c.enabled,
        "checked": c.checked,
        "value": num(c.value),
        "text": c.text.as_str(),
        "min": num(c.min),
        "max": num(c.max),
        "step": num(c.step),
        "unit": c.unit.as_str(),
        "presets": c.presets.iter().map(|p| json!({ "value": num(p.value), "label": p.label.as_str(), "selected": p.selected })).collect::<Vec<_>>(),
        "options": c.options.iter().map(|o| json!({ "value": o.value.as_str(), "label": o.label.as_str(), "selected": o.selected })).collect::<Vec<_>>(),
        "tone": c.tone.as_str(),
        "confirm": c.confirm.as_str(),
        "placeholder": c.placeholder.as_str(),
    })
}

/// Число из Slint (f32) для JSON: 0.1 вместо 0.10000000149011612.
fn num(v: f32) -> f64 {
    let v = v as f64;
    if v.abs() >= 1e9 { v.round() } else { (v * 1e6).round() / 1e6 }
}

/// QR-код адреса: тёмные квадраты на белом, с полями по 4 клетки (так его читает любая камера).
fn qr_image(text: &str) -> Image {
    use qrcodegen::{QrCode, QrCodeEcc};
    let Ok(qr) = QrCode::encode_text(text, QrCodeEcc::Medium) else { return Image::default() };
    const BORDER: i32 = 4;
    const SCALE: i32 = 8;
    let cells = qr.size() + BORDER * 2;
    let side = (cells * SCALE) as u32;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(side, side);
    let dark = Rgba8Pixel { r: 0x0a, g: 0x0c, b: 0x11, a: 255 };
    let light = Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 };
    let px = buf.make_mut_slice();
    for y in 0..side as i32 {
        for x in 0..side as i32 {
            let on = qr.get_module(x / SCALE - BORDER, y / SCALE - BORDER);
            px[(y * side as i32 + x) as usize] = if on { dark } else { light };
        }
    }
    Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_code_fits_a_typical_address() {
        let url = format!("http://192.168.100.200:47777/#k={}", "A".repeat(32));
        let qr = qrcodegen::QrCode::encode_text(&url, qrcodegen::QrCodeEcc::Medium).unwrap();
        assert!(qr.size() <= 41, "QR слишком плотный: {}", qr.size());
    }
}
