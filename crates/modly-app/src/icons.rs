//! Иконки: встроенный набор (по имени — для модов) и иконки игр из их exe.

use slint::{Color, Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Image>>> = RefCell::new(HashMap::new());
}

/// Синонимы: моды могут называть иконки по-своему.
fn alias(name: &str) -> &str {
    match name {
        "settings" | "gear" | "cog" => "sliders",
        "game" | "controller" => "gamepad",
        "mod" | "mods" | "plugin" => "puzzle",
        "lightning" | "zap" | "live" => "bolt",
        "delete" | "remove" | "bin" => "trash",
        "add" => "plus",
        "close" | "cross" => "x",
        "ok" | "done" => "check",
        "warning" | "warn" => "alert",
        "time" => "clock",
        "weather" => "cloud",
        "health" | "hp" => "heart",
        "god" | "armor" => "shield",
        "money" | "gold" | "coin" => "coins",
        "speed" | "fast" => "gauge",
        "teleport" | "location" | "pin" => "map-pin",
        "build" | "construct" => "hammer",
        "magic" | "spell" => "wand",
        "item" | "items" | "inventory" => "box",
        "pup" | "pups" | "dog" => "wolf",
        "animal" | "animals" | "pet" | "pets" => "paw",
        "food" => "fish",
        "water" => "droplet",
        "fire" => "flame",
        "kill" | "death" => "skull",
        "freeze" | "ice" => "snowflake",
        "harvest" | "chop" => "axe",
        "farm" => "house",
        "dig" | "terrain" => "shovel",
        "player" | "person" => "user",
        "visible" | "show" => "eye",
        "key" | "hotkey" => "keyboard",
        other => other,
    }
}

/// Встроенная иконка по имени (с учётом синонимов). Неизвестное имя — `None`.
pub fn named(name: &str) -> Option<Image> {
    let key = alias(&name.trim().to_lowercase()).to_owned();
    CACHE.with(|c| {
        c.borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| {
                ICONS
                    .iter()
                    .find(|(n, _)| *n == key)
                    .and_then(|(_, data)| Image::load_from_svg_data(data).ok())
            })
            .clone()
    })
}

pub fn named_opt(name: Option<&str>) -> (Image, bool) {
    match name.and_then(named) {
        Some(img) => (img, true),
        None => (Image::default(), false),
    }
}

/// Иконка игры из exe и её главный цвет.
pub fn exe_icon(exe: &Path) -> Option<(Image, Option<Color>)> {
    #[cfg(windows)]
    {
        let (w, h, rgba) = modly_core::win::exe_icon(exe, 128)?;
        let accent = modly_core::win::accent_color(&rgba).map(|(r, g, b)| Color::from_rgb_u8(r, g, b));
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h);
        Some((Image::from_rgba8(buf), accent))
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        None
    }
}

/// Цвет обложки по имени игры, если у неё нет своей иконки.
pub fn fallback_accent(seed: &str) -> Color {
    const PALETTE: [(u8, u8, u8); 6] =
        [(139, 92, 246), (59, 130, 246), (20, 184, 166), (236, 72, 153), (249, 115, 22), (34, 197, 94)];
    let h = seed.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = PALETTE[(h as usize) % PALETTE.len()];
    Color::from_rgb_u8(r, g, b)
}

/// `"#7c5cff"` → цвет.
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_builtin_icons_are_valid_svg() {
        for (name, data) in super::ICONS {
            assert!(slint::Image::load_from_svg_data(data).is_ok(), "{name}");
        }
        assert!(super::named("god").is_some());
        assert!(super::named("does-not-exist").is_none());
    }
}
