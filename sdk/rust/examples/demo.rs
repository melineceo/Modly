//! Демо-мод: притворяется игрой и показывает в Modly все виды элементов.
//!
//!     cargo run -p modly-sdk --example demo
//!
//! Запустите Modly — на странице «Для мододелов» появится «Modly Demo», а в библиотеке — игра,
//! из которой он запущен (сам этот exe).

use modly_sdk::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct World {
    god: bool,
    infinite_ammo: bool,
    speed: f64,
    jump: f64,
    coins: i64,
    name: String,
    weather: String,
    time_of_day: String,
    spawn: Vec<String>,
    health: f64,
    enemies: i64,
    day: f64,
    /// Игрок на карте: ходит кругами вокруг (cx, cz); телепорт переносит центр.
    cx: f64,
    cz: f64,
    angle: f64,
    /// Волчонок бежит за игроком.
    pup: (f64, f64),
    /// Сундук забрали — убрать его с карты.
    chest_taken: bool,
}

impl World {
    fn player(&self) -> (f64, f64) {
        (self.cx + 60.0 * self.angle.cos(), self.cz + 60.0 * self.angle.sin())
    }
}

fn main() {
    let w = Rc::new(RefCell::new(World {
        speed: 1.0,
        jump: 2.0,
        coins: 250,
        name: "Player".into(),
        weather: "clear".into(),
        time_of_day: "noon".into(),
        spawn: vec!["chicken".into()],
        health: 80.0,
        enemies: 12,
        ..Default::default()
    }));

    let mut panel = Panel::new(
        ModInfo {
            author: Some("Modly".into()),
            description: Some(Text::tr(&[("en", "Every control kind Modly can draw"), ("ru", "Все виды элементов, которые умеет Modly")])),
            ..ModInfo::new("modly-demo", "Modly Demo", "1.0.0")
        },
        Some(GameInfo::new("modly-demo-game", "Demo Game", None)),
    );

    macro_rules! get {
        ($f:ident) => {{
            let w = w.clone();
            move || w.borrow().$f.clone()
        }};
    }
    macro_rules! set {
        ($f:ident) => {{
            let w = w.clone();
            move |v| w.borrow_mut().$f = v
        }};
    }

    panel
        .page("player", Text::tr(&[("en", "Player"), ("ru", "Игрок")]), Some("user"))
        .section("cheats", Text::tr(&[("en", "Cheats"), ("ru", "Читы")]))
        .add(
            toggle("god", Text::tr(&[("en", "God mode"), ("ru", "Бессмертие")]))
                .icon("shield")
                .describe(Text::tr(&[("en", "Nothing can hurt you"), ("ru", "Ничто не может навредить")])),
            value(get!(god), set!(god)),
        )
        .add(toggle("ammo", Text::tr(&[("en", "Infinite ammo"), ("ru", "Бесконечные патроны")])).icon("flame"), value(get!(infinite_ammo), set!(infinite_ammo)))
        .add(
            slider("speed", Text::tr(&[("en", "Move speed"), ("ru", "Скорость бега")]), 0.5, 5.0).step(0.1).unit("×").icon("gauge"),
            value(get!(speed), set!(speed)),
        )
        .add(slider("jump", Text::tr(&[("en", "Jump height"), ("ru", "Высота прыжка")]), 1.0, 20.0).step(1.0).unit("m").icon("rocket"), value(get!(jump), set!(jump)))
        .add(
            number("coins", Text::tr(&[("en", "Coins"), ("ru", "Монеты")])).range(0.0, 999_999.0).presets(&[100.0, 1000.0, 10_000.0, 100_000.0]).icon("coins"),
            value(get!(coins), set!(coins)),
        )
        .add(
            text("name", Text::tr(&[("en", "Nickname"), ("ru", "Ник")])).placeholder(Text::tr(&[("en", "Type and press Enter"), ("ru", "Введите и нажмите Enter")])).icon("user"),
            value(get!(name), set!(name)),
        );

    {
        let w2 = w.clone();
        let w3 = w.clone();
        let w4 = w.clone();
        let w5 = w.clone();
        panel
            .page("player", "", None)
            .section("status", Text::tr(&[("en", "Status"), ("ru", "Состояние")]))
            .add(stat("health", Text::tr(&[("en", "Health"), ("ru", "Здоровье")])).unit("HP").tone(Tone::Success).icon("heart"), read(move || w2.borrow().health.round()))
            .add(progress("day", Text::tr(&[("en", "Day progress"), ("ru", "Прошло дня")])).icon("clock"), read(move || w3.borrow().day))
            .add(
                button("heal", Text::tr(&[("en", "Heal to full"), ("ru", "Вылечить полностью")])).tone(Tone::Accent).icon("heart").hotkey("F7"),
                action_with(move || {
                    w4.borrow_mut().health = 100.0;
                    Ok(Some("Healed!".into()))
                }),
            )
            .add(
                button("kill", Text::tr(&[("en", "Kill all enemies"), ("ru", "Убить всех врагов")]))
                    .tone(Tone::Danger)
                    .confirm(Text::tr(&[("en", "Really kill all?"), ("ru", "Точно всех?")]))
                    .icon("skull"),
                action_with(move || {
                    let n = std::mem::take(&mut w5.borrow_mut().enemies);
                    Ok(Some(format!("Enemies killed: {n}")))
                }),
            );
    }

    {
        let w2 = w.clone();
        panel
            .page("world", Text::tr(&[("en", "World"), ("ru", "Мир")]), Some("globe"))
            .section("env", Text::tr(&[("en", "Environment"), ("ru", "Окружение")]))
            .add(
                select("weather", Text::tr(&[("en", "Weather"), ("ru", "Погода")]), [("clear", Text::tr(&[("en", "Clear"), ("ru", "Ясно")])), ("rain", Text::tr(&[("en", "Rain"), ("ru", "Дождь")]))])
                    .option_icons(&["sun", "rain"])
                    .icon("cloud"),
                value(get!(weather), set!(weather)),
            )
            .add(
                select(
                    "time",
                    Text::tr(&[("en", "Time of day"), ("ru", "Время суток")]),
                    [("morning", Text::tr(&[("en", "Morning"), ("ru", "Утро")])), ("noon", Text::tr(&[("en", "Noon"), ("ru", "Полдень")])), ("evening", Text::tr(&[("en", "Evening"), ("ru", "Вечер")]))],
                )
                .icon("clock"),
                value(get!(time_of_day), set!(time_of_day)),
            )
            .add(note("tip", Text::tr(&[("en", "Changes apply instantly — no restart needed."), ("ru", "Изменения применяются сразу, без перезапуска.")])), Binding::None)
            .add(stat("enemies", Text::tr(&[("en", "Enemies nearby"), ("ru", "Врагов рядом")])).tone(Tone::Warning).icon("skull"), read(move || w2.borrow().enemies));

        panel
            .page("world", "", None)
            .section("spawn", Text::tr(&[("en", "Spawn"), ("ru", "Создать")]))
            .describe(Text::tr(&[("en", "Pick what to spawn — several at once"), ("ru", "Выберите, что создать — можно несколько")]))
            .add(
                multi_select(
                    "spawn",
                    Text::tr(&[("en", "Animals"), ("ru", "Животные")]),
                    [
                        ("chicken", Text::tr(&[("en", "Chicken"), ("ru", "Курица")])),
                        ("pig", Text::tr(&[("en", "Pig"), ("ru", "Свинья")])),
                        ("wolf", Text::tr(&[("en", "Wolf pup"), ("ru", "Волчонок")])),
                        ("fish", Text::tr(&[("en", "Fish"), ("ru", "Рыба")])),
                        ("egg", Text::tr(&[("en", "Egg"), ("ru", "Яйцо")])),
                    ],
                )
                .option_icons(&["chicken", "pig", "wolf", "fish", "egg"]),
                value(get!(spawn), set!(spawn)),
            );
    }

    // Карта: картинка вшита в мод (data:-адрес), края — в координатах «игры»: x от −800 до 800,
    // z от −533 до 533, z растёт вверх (поэтому top больше bottom). Метки — в тех же координатах.
    {
        let tr = |en: &str, ru: &str| Text::tr(&[("en", en), ("ru", ru)]);
        let image = data_url("image/webp", include_bytes!("demo-map.webp"));
        let (p, pup, tp) = (w.clone(), w.clone(), w.clone());
        let loot = w.clone();
        let mut map = panel.map("island", tr("Demo island", "Демо-остров"), &image);
        map.bounds(-800.0, 533.0, 800.0, -533.0)
            .category(category("places", tr("Places", "Места")).icon("map-pin").color("#8b5cf6"))
            .category(category("items", tr("Items", "Предметы")).icon("gem").color("#fbbf24"))
            .category(category("danger", tr("Danger", "Опасность")).icon("skull").color("#f87171"))
            .marker(marker("mountain", -112.0, 32.0).cat("places").title(tr("Old mountain", "Старая гора")).text(tr("The highest point.", "Самая высокая точка острова.")).action(act("tp", tr("Teleport", "Телепорт")).tone(Tone::Accent)))
            .marker(marker("lake", 192.0, 85.0).cat("places").title(tr("Blue lake", "Синее озеро")).action(act("tp", tr("Teleport", "Телепорт")).tone(Tone::Accent)))
            .marker(
                marker("chest", -160.0, -128.0)
                    .cat("items")
                    .title(tr("Coin chest", "Сундук с монетами"))
                    .text(tr("250 coins inside.", "Внутри 250 монет."))
                    .action(act("loot", tr("Take the coins", "Забрать монеты")).tone(Tone::Success)),
            )
            .marker(marker("crystal", 336.0, 213.0).cat("items").title(tr("Rare crystal", "Редкий кристалл")))
            .marker(marker("den", 480.0, -53.0).cat("danger").title(tr("Wolf den", "Логово волков")).text(tr("Dangerous at night!", "Ночью опасно!")))
            .marker(marker("cliff", -560.0, 107.0).cat("danger").title(tr("Cliff", "Обрыв")))
            .live(marker("player", 0.0, 0.0).dot().color("#22d3ee").title(tr("You", "Вы")), move || p.borrow().player())
            .live(marker("pup", 0.0, 0.0).dot().color("#34d399").title(tr("Wolf pup", "Волчонок")), move || pup.borrow().pup)
            .click_action(act("tp", tr("Teleport here", "Телепорт сюда")).tone(Tone::Accent))
            .on("tp", move |hit| {
                let mut s = tp.borrow_mut();
                s.cx = hit.x - 60.0 * s.angle.cos();
                s.cz = hit.y - 60.0 * s.angle.sin();
                Ok(Some(format!("Teleported to ({:.0}, {:.0})", hit.x, hit.y)))
            })
            .on("loot", move |_| {
                let mut s = loot.borrow_mut();
                if s.chest_taken {
                    return Err("The chest is empty".into());
                }
                s.coins += 250;
                s.chest_taken = true;
                Ok(Some("+250 coins".into()))
            })
            .focus("player");
    }

    println!("Modly Demo: waiting for Modly ({}), Ctrl+C to quit", protocol::PIPE_NAME);
    let started = Instant::now();
    let mut was_connected = false;
    let mut chest_removed = false;
    loop {
        panel.pump();
        // «игра» живёт своей жизнью: день идёт, здоровье тает (если не бессмертие), враги прибывают,
        // игрок бродит по кругу (быстрее — с ползунком «Скорость бега»), волчонок бежит следом
        {
            let mut s = w.borrow_mut();
            s.day = (started.elapsed().as_secs_f64() / 60.0).fract();
            if !s.god {
                s.health = (s.health - 0.02).max(1.0);
            }
            if started.elapsed().as_millis() % 7000 < 16 {
                s.enemies += 1;
            }
            s.angle += 0.01 * s.speed;
            let (px, pz) = s.player();
            s.pup.0 += (px - s.pup.0) * 0.03;
            s.pup.1 += (pz - s.pup.1) * 0.03;
        }
        if w.borrow().chest_taken && !chest_removed {
            chest_removed = true;
            panel.map_remove_marker("island", "chest");
        }
        if panel.is_connected() != was_connected {
            was_connected = panel.is_connected();
            println!("{}", if was_connected { "connected to Modly" } else { "Modly is gone, waiting…" });
            if was_connected {
                panel.status(Tone::Info, Text::tr(&[("en", "Demo world is running"), ("ru", "Демо-мир работает")]));
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
