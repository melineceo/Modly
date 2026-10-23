// Пример мода на C++: консольная «игра», которой можно управлять из Modly.
//
//   cl /std:c++17 /EHsc /utf-8 /I.. demo.cpp
//
// Запустите Modly, потом demo.exe — в Modly появится «C++ Demo» с переключателями и кнопками.

#include "modly.hpp"

#include <cstdio>

int main() {
    bool god = false;
    float speed = 1.0f;
    int coins = 100;
    std::string weather = "clear";
    double health = 100;

    modly::Client modly({"cpp-demo", "C++ Demo", "1.0.0", "Modly", "cpp-demo-game", "C++ Demo Game"});

    auto& player = modly.page("player", modly::Text("Player").ru("Игрок"), "user").section("cheats", modly::Text("Cheats").ru("Читы"));
    player.toggle("god", modly::Text("God mode").ru("Бессмертие"), &god).icon("shield");
    player.slider("speed", modly::Text("Speed").ru("Скорость"), 0.5, 5.0, &speed).step(0.1).unit("x").icon("gauge");
    player.number("coins", modly::Text("Coins").ru("Монеты"), &coins).range(0, 99999).presets({100, 1000, 10000}).icon("coins");
    player.stat("health", modly::Text("Health").ru("Здоровье"), [&] { return modly::json::Value(std::round(health)); }).unit("HP").icon("heart");
    player.button_with_result("heal", modly::Text("Heal").ru("Вылечить"), [&] {
        health = 100;
        return std::string("Healed!");
    }).tone(modly::Tone::Accent).icon("heart");

    auto& world = modly.page("world", modly::Text("World").ru("Мир"), "globe").section("env");
    world.select("weather", modly::Text("Weather").ru("Погода"), {{"clear", modly::Text("Clear").ru("Ясно"), "sun"}, {"rain", modly::Text("Rain").ru("Дождь"), "rain"}}, &weather).icon("cloud");

    // Карта: картинка лежит рядом с exe (demo-map.webp — из sdk/rust/examples), края — в координатах
    // «игры»: x от −800 до 800, z от −533 до 533, z растёт вверх. Игрок — живая метка по указателям.
    double px = 0, pz = 0;
    wchar_t exe[MAX_PATH] = {};
    GetModuleFileNameW(nullptr, exe, MAX_PATH);
    char utf8[MAX_PATH * 3] = {};
    WideCharToMultiByte(CP_UTF8, 0, exe, -1, utf8, sizeof utf8, nullptr, nullptr);
    std::string dir(utf8);
    dir = dir.substr(0, dir.find_last_of("\\/") + 1);
    auto& map = modly.map("island", modly::Text("Island").ru("Остров"), dir + "demo-map.webp");
    map.bounds(-800, 533, 800, -533).focus("player");
    map.category("places", modly::Text("Places").ru("Места")).icon("map-pin").color("#8b5cf6");
    map.marker("lake", 192, 85).cat("places").title(modly::Text("Blue lake").ru("Синее озеро")).action("tp", modly::Text("Teleport").ru("Телепорт"));
    map.live("player", &px, &pz).dot().color("#22d3ee").title(modly::Text("You").ru("Вы"));
    map.click_action("tp", modly::Text("Teleport here").ru("Телепорт сюда")).tone(modly::Tone::Accent);
    map.on("tp", [&](const modly::MapHit& hit) {
        px = hit.x;
        pz = hit.y;
        return std::string("Teleported");
    });

    modly.on_connected = [] { std::puts("connected to Modly"); };
    modly.on_disconnected = [] { std::puts("Modly is gone, waiting..."); };
    modly.start();
    std::puts("C++ Demo: waiting for Modly, Ctrl+C to quit");

    for (;;) {
        if (!god) health = health > 1 ? health - 0.02 : 1;
        modly.update();
        std::this_thread::sleep_for(std::chrono::milliseconds(16));
    }
}
