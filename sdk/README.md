# Modly SDK

Подключите свой мод к Modly: опишите переключатели, ползунки, поля и кнопки — Modly нарисует их в своём
окне и будет присылать изменения. **Мод не зависит от Modly**: пока Modly не запущен, SDK тихо ждёт
в фоне, а мод работает как обычно.

| | Файл | Для чего |
|---|---|---|
| **C#** | [`unity/Modly.cs`](unity/Modly.cs) | Unity (Mono и IL2CPP), BepInEx, MelonLoader, любые .NET-игры |
| **C++** | [`cpp/modly.hpp`](cpp/modly.hpp) | Unreal Engine (UE4SS), инжектируемые DLL, свои движки |
| **Rust** | [`rust`](rust) (крейт `modly-sdk`) | моды и инструменты на Rust |
| другой язык | [`docs/PROTOCOL.md`](../docs/PROTOCOL.md) | протокол: строки JSON по именованному каналу |

Все три SDK устроены одинаково: **значения привязываются к данным мода** (геттер + сеттер или указатель),
SDK сам рассылает изменившиеся значения и вызывает сеттеры, когда пользователь что-то меняет в Modly.

## C# (Unity и .NET)

Скопируйте `Modly.cs` в проект мода (или подключите его к сборке, как в [`examples/unity-mod`](../examples/unity-mod)).

```csharp
using Modly;

var modly = new ModlyClient("my-mod", "My Mod", "1.0.0") { Author = "me", GameId = "my-game", GameName = "My Game" };

var s = modly.Page("player", Txt.Of("Игрок", "Player"), "user").Section("cheats", "Cheats");
s.Toggle("god", Txt.Of("Бессмертие", "God mode"), () => god, v => god = v).Icon("shield");
s.Slider("speed", "Speed", 0.5f, 5f, () => speed, v => speed = v).Step(0.1).Unit("×");
s.Number("coins", "Coins", () => coins, v => coins = v).Range(0, 99999).Presets(100, 1000, 10000);
s.Select("weather", "Weather", new[] { Option.Of("clear", "Clear", "sun"), Option.Of("rain", "Rain", "rain") },
    () => weather, v => weather = v);
s.Button("heal", "Heal", () => { Heal(); return "Healed!"; }).Tone(Tone.Accent);   // текст — уведомлением
s.Button("kill", "Kill all", KillAll).Tone(Tone.Danger).Confirm("Really?");         // со второго нажатия
s.Stat("health", "Health", () => player.health).Unit("HP");
modly.Start();

// в Update() какого-нибудь MonoBehaviour — команды Modly выполняются здесь, на главном потоке:
modly.Update();
```

Ещё: `.EnabledWhen(() => worldLoaded)`, `.VisibleWhen(...)`, `.OptionsFrom(() => …)` (варианты, которые
меняются по ходу игры), `.Describe(...)`, `modly.Notify(...)`, `modly.Status(...)` (строка состояния —
например, «Загрузите сохранение»), события `ConnectedToModly` / `DisconnectedFromModly` / `LanguageChanged`.

**Важно для Unity:**

- **Всё — на главном потоке.** `modly.Update()` вызывайте каждый кадр: там выполняются сеттеры и кнопки
  и читаются значения. Фоновый поток SDK только передаёт сообщения.
- **Урезанные сборки.** В сборках игр вырезано всё, что игра не использует (`Stopwatch.StartNew`,
  `System.IO.Pipes`, иногда даже `Time.timeScale`). SDK этим не пользуется: канал открывается через
  `kernel32`, JSON — свой. Собирайте мод против DLL самой игры — компилятор сразу покажет, чего нет.
- **Пауза без фокуса.** Многие Unity-игры замирают, когда их окно не активно, — тогда нажатия в Modly
  выполнятся после возврата в игру. SDK сам сообщает Modly о паузе, и Modly показывает пояснение.
  Если в игре есть `Application.runInBackground` (сеттер не вырезан) — можно включать его на время
  связи с Modly.
- **IL2CPP.** P/Invoke работает и в IL2CPP; при моддинге через BepInEx IL2CPP подключайте `Modly.cs`
  к своему плагину как обычный исходник.
- Типы SDK — `internal`: файл вшивается в каждый мод, и несколько модов с SDK в одной игре не мешают
  друг другу.

Автотест SDK — [`unity/tests`](unity/tests): `powershell -ExecutionPolicy Bypass -File sdk\unity\tests\run.ps1`
(или `dotnet run` в этой папке). Тест сам поднимает канал вместо Modly (`ModlyClient.Pipe`) и проверяет протокол со стороны мода.

## C++ (Unreal и натив)

```cpp
#include "modly.hpp"

static bool god = false;
static float speed = 1.0f;
static int coins = 100;

modly::Client modly({"my-mod", "My Mod", "1.0.0", "me", "my-game", "My Game"});

void mod_init() {
    auto& s = modly.page("player", modly::Text("Player").ru("Игрок"), "user").section("cheats", "Cheats");
    s.toggle("god", "God mode", &god).icon("shield");
    s.slider("speed", "Speed", 0.5, 5.0, &speed).step(0.1);
    s.number("coins", "Coins", &coins).range(0, 99999).presets({100, 1000, 10000});
    s.button_with_result("heal", "Heal", [] { heal(); return std::string("Healed!"); }).tone(modly::Tone::Accent);
    modly.start();
}

void on_game_tick() {      // на игровом потоке, каждый кадр
    modly.update();
}
```

- Один заголовок, C++17, только WinAPI. Строки — UTF-8. Собирается MSVC (`/std:c++17 /EHsc /utf-8`),
  clang и MinGW. Пример — [`cpp/example/demo.cpp`](cpp/example/demo.cpp).
- **UE4SS (C++ моды):** создайте `modly::Client` при загрузке мода, а `update()` вызывайте из
  игрового потока — например, из хука на `ProcessEvent`/`Tick` или колбэка обновления UE4SS.
  Не вызывайте игровые функции из потока SDK.
- `std::function`-перегрузки — для значений, которые нельзя привязать указателем:
  `s.toggle("id", "Label", [] { return get(); }, [](bool v) { set(v); })`.

## Rust

```rust
use modly_sdk::*;

let mut panel = Panel::new(ModInfo::new("my-mod", "My Mod", "1.0.0"), Some(GameInfo::new("my-game", "My Game", None)));
panel.page("main", Text::tr(&[("en", "Main"), ("ru", "Главное")]), Some("star"))
    .section("player", "Player")
    .add(toggle("god", "God mode").icon("shield"), value(get_god, set_god))
    .add(slider("speed", "Speed", 0.5, 5.0).step(0.1), value(get_speed, set_speed))
    .add(button("heal", "Heal").tone(Tone::Accent), action(heal));

loop {
    panel.pump(); // в игровом цикле
}
```

Полный пример со всеми видами элементов: `cargo run -p modly-sdk --example demo`.

## Карты

Мод может показать карты своей игры: картинку, края в координатах игры, категории и метки — и живые
метки (игрок, питомцы), которые SDK сам двигает, и действия вроде «Телепорт сюда»:

```csharp
var map = modly.Map("island", Txt.Of("Остров", "Island"), @"C:\…\island.png").Bounds(-800, 533, 800, -533);
map.Live("player", () => player.x, () => player.z).Dot().Title("You");
map.ClickAction("tp", Txt.Of("Телепорт сюда", "Teleport here"));
map.On("tp", hit => { Teleport(hit.X, hit.Y); return null; });
```

В C++ — `modly.map(...)`, в Rust — `panel.map(...)`, те же методы. Подробно — [mods.hackerstone.xyz/docs.php?p=mod-maps](https://mods.hackerstone.xyz/docs.php?p=mod-maps).

## Пакет мода

Чтобы Modly ещё и **находил игру и ставил мод одной кнопкой**, положите рядом с модом
[`modly.json`](../docs/MANIFEST.md). Для Unity на Mono есть установка без загрузчиков (`unity-mono`):
Modly прописывает вашу DLL в списки Unity, и игра загружает мод сама.

## Отладка

**Настройки → Режим разработчика** — на странице «Для мододелов» появится консоль протокола со всеми
сообщениями в обе стороны, а у модов с `dev.build` в `modly.json` — кнопка «Собрать».
