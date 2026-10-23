# Modly

**Modly** — быстрый и красивый менеджер модов и панель управления ими, написанный на Rust.
Находит игры на диске, ставит и обновляет моды одной кнопкой, а пока игра запущена, показывает
переключатели, ползунки и кнопки мода прямо в своём окне — как WeMod, только для любых модов и
с открытым API для мододелов.

- **Лёгкий.** Один exe на 18 МБ, ~30 МБ памяти, ~0 % процессора в простое: интерфейс на
  [Slint](https://slint.dev) рисуется только там, где что-то изменилось.
- **Открытый API.** Мод описывает свой интерфейс по [простому протоколу](docs/PROTOCOL.md) —
  Modly рисует его сам. Готовые SDK: [C# для Unity и .NET](sdk/unity/Modly.cs),
  [C++ для Unreal и натива](sdk/cpp/modly.hpp), [Rust](sdk/rust). Остальные языки — по протоколу.
- **Мод не зависит от Modly.** Без Modly мод работает как обычно; связь появляется сама, когда
  Modly запущен.
- **Каталог модов.** Моды с [mods.hackerstone.xyz](https://mods.hackerstone.xyz) прямо в Modly: каждый
  выпуск проверен модератором, Modly сверяет SHA-256 и ставит мод в игру одной кнопкой (или кнопкой
  «Установить через Modly» на сайте — ссылка `modly://install/<адрес>`).
- **Unity без загрузчиков.** Для Unity-игр на Mono Modly ставит мод без BepInEx — прописывает DLL
  в списки Unity, и игра загружает её сама.

> **Бета.** Modly ещё в бете — ошибки почти наверняка найдутся. Нашли — напишите в
> [Issues](../../issues), в [Telegram](https://t.me/hackerstonecorp) или [Discord](https://discord.gg/SNM4KemTv).

## Скачать

Последняя версия — на странице [Releases](../../releases) (Windows 10/11, x64):

- **`ModlySetup-<версия>.exe`** — установщик. Ставит Modly для текущего пользователя (без прав
  администратора) в `%LOCALAPPDATA%\Programs\Modly`, создаёт ярлыки, подключает ссылки `modly://`
  и запись в «Приложения и возможности». По желанию ставит инструменты мододела (.NET SDK, C++ Build Tools).
- **`Modly-<версия>-windows-x64.zip`** — портативная версия: распакуйте и запустите `modly.exe`.

Дальше Modly **обновляется сам**: проверяет GitHub при запуске и раз в 6 часов и ставит новую версию
по кнопке «Обновить». Каждое обновление подписано ключом выпусков — Modly сверяет подпись и SHA-256,
а если новая версия не запустится, вернёт прежнюю. Бета-версии и автопроверка — в «Настройки → Обновления».
Отдельные библиотеки не нужны: среда выполнения C++ вшита в exe. Настройки и библиотека игр хранятся
в `%APPDATA%\Modly`. Что нового — в [CHANGELOG.md](CHANGELOG.md).

## Возможности

- Библиотека игр: поиск на рабочем столе, в загрузках, папках `Games` и `Program Files`, в корне дисков, библиотеках Steam и itch (копии не из Steam — тоже);
  иконка и цвет обложки — из exe игры; несколько копий одной игры различаются.
- Моды: установка, обновление, удаление; состояние определяется по файлам игры, поэтому видны
  и моды, поставленные своим установщиком. Мод относится к копии игры по её папке (exe и файлы,
  у Steam — номер приложения), как бы автор ни назвал игру.
- Сборки игры: мод перечисляет в `modly.json` сборки, на которых проверен, а Modly пишет на карточке
  «Сборка b108 — проверена автором» или предупреждает о непроверенной; автору — кнопка
  «Добавить сборку в modly.json».
- Панель управления: страницы, секции, переключатели, ползунки, числа с быстрыми вариантами,
  текстовые поля, выбор (один или несколько вариантов), показатели, прогресс, кнопки
  с подтверждением; уведомления и строка состояния от мода; сообщение, если игра на паузе без фокуса.
- Для мододелов: консоль протокола, сборка модов из исходников кнопкой «Собрать», примеры
  и документация.
- Каталог: моды с сайта, установка и обновление в один клик, скачивание с прогрессом, лайки;
  необязательный вход через аккаунт hackerstone.xyz (в браузере, пароль в Modly не попадает, токен
  зашифрован ключом Windows).
- Библиотека Steam: все установленные игры с обложками из кэша Steam, запуск через Steam,
  «Сделать свой мод» для любой игры (готовый проект на C# для Unity Mono или на C++).
- Оверлей в игре (`Ctrl+Shift+O`): панель мода поверх игры, игра не теряет фокус; вкладка с картой.
- Пульт с телефона: панель мода в браузере телефона по QR-коду (только своя сеть, вход по ключу).
- Карты игр от модов: мод присылает картинку, края в координатах своей игры, метки, живые точки
  (игрок, питомцы) и действия («Телепорт сюда»); карта из пакета видна и без игры. Приближение,
  категории, поиск — на странице игры и в оверлее.
- Связка с WeMod: кнопка «Открыть в WeMod» на странице игры (трейнеры WeMod Modly не скачивает).
- Русский и английский, тёмная тема, доступность (Windows UI Automation / экранные дикторы).

## Сборка и запуск

Нужен Rust 1.85+ (Windows).

```
cargo run -p modly-app --release
```

Готовый exe — `target/release/modly.exe`. Данные Modly — в `%APPDATA%\Modly`
(переменная `MODLY_HOME` подменяет папку — удобно для тестов).

Установщик, архив и подписанный манифест обновления собирает `cargo xtask dist`
(выложить на GitHub — `cargo xtask dist --publish`), подробности — в [docs/RELEASING.md](docs/RELEASING.md).

Флаги запуска: `--page home|mods|catalog|dev|settings`, `--game <название или папка>`,
`modly://install/<адрес мода>` (так Modly открывает сайт; если Modly уже запущен, ссылка уходит ему).
`MODLY_MODS_URL` подменяет адрес сайта модов (например, `http://127.0.0.1:8082` для локальной копии).

Проверить панель без игры — демо-мод со всеми видами элементов:

```
cargo run -p modly-sdk --example demo
```

## Устройство

```
crates/
  modly-protocol/   протокол Modly ↔ мод: сообщения и описание интерфейса
  modly-core/       библиотека игр, пакеты модов, установщики, процессы, сервер канала
  modly-app/        приложение: Rust + Slint (ui/*.slint)
  modly-setup/      установщик (ModlySetup.exe), обновлятор и удаление (modly-updater.exe)
xtask/              выпуск: ключ подписи, установщик, пакет обновления, публикация на GitHub
sdk/
  unity/Modly.cs    SDK для C# (Unity, .NET)
  cpp/modly.hpp     SDK для C++ (Unreal, натив)
  rust/             SDK для Rust (крейт modly-sdk) + пример demo
examples/
  unity-mod/        шаблон Unity-мода: панель в Modly, сборка против DLL игры, modly.json
docs/
  PROTOCOL.md       протокол — для модов на любом языке
  MANIFEST.md       пакет мода (modly.json): поиск игры и установка
  RELEASING.md      как выпустить новую версию Modly
  MOD-UPDATES.md    как мод сам обновляется с сайта (подпись автора)
mods/               пакеты модов для разработки (приватные моды сюда, они в .gitignore)
```

## Для мододелов

Вся документация — с поиском, на русском и английском — на сайте:
[mods.hackerstone.xyz/docs.php](https://mods.hackerstone.xyz/docs.php) (тексты — в
`hs\site\mods.hackerstone.xyz\app\docs`; `docs/PROTOCOL.md` и `docs/MANIFEST.md` здесь — их источник).

1. Подключите SDK и опишите элементы — [sdk/README.md](sdk/README.md).
2. По желанию положите рядом [`modly.json`](docs/MANIFEST.md) — Modly будет находить игру
   и ставить мод одной кнопкой.
3. Включите **Настройки → Режим разработчика**, чтобы видеть каждое сообщение протокола.

## Лицензии

Modly — [MIT](LICENSE) © Meline / Hackerstone: можно использовать, менять и распространять, указав авторов.
Интерфейс сделан на [Slint](https://slint.dev)
(лицензия Royalty-free 2.0 — виджет «Made with Slint» показан в «Настройки → О программе»).

---

## English

**Modly** is a fast, lightweight mod manager and live control panel written in Rust + Slint.
It finds your games, installs and updates mods with one click, and while a game runs it shows the
mod's toggles, sliders and buttons in its own window. Mods describe their UI over a tiny
[NDJSON-over-named-pipe protocol](docs/PROTOCOL.md); SDKs are available for
[C# (Unity/.NET)](sdk/unity/Modly.cs), [C++ (Unreal/native)](sdk/cpp/modly.hpp) and [Rust](sdk/rust).
Mods keep working without Modly. The built-in **Catalog** installs moderated mods from
[mods.hackerstone.xyz](https://mods.hackerstone.xyz) in one click (SHA-256 verified; `modly://install/<slug>` links
from the site open Modly), with an optional sign-in through the browser. Also: your Steam library with covers,
an in-game overlay (`Ctrl+Shift+O`), a phone remote (QR code, local network only), game maps sent by mods
(live player markers, "teleport here" actions) and
"make your own mod" project templates. Docs: [mods.hackerstone.xyz/docs.php](https://mods.hackerstone.xyz/docs.php).
MIT licensed © Meline / Hackerstone.
Build with `cargo run -p modly-app --release`.

**Beta** — expect bugs; please report them in [Issues](../../issues).
Get `ModlySetup-<version>.exe` from [Releases](../../releases) (Windows 10/11 x64): it installs Modly for the
current user without admin rights, adds shortcuts, `modly://` links and an Apps & features entry, and can
install modder tools (.NET SDK, C++ Build Tools). A portable zip is there too. After that Modly **updates
itself**: signed updates from GitHub (Ed25519 + SHA-256), with rollback if a new version fails to start.
No extra runtimes needed. Releases are built with `cargo xtask dist` — see [docs/RELEASING.md](docs/RELEASING.md).
