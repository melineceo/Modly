# Протокол Modly (версия 1)

Modly рисует интерфейс мода — переключатели, ползунки, поля, кнопки — по описанию, которое мод присылает
сам. Мод не зависит от Modly: если Modly не запущен, мод работает как обычно (своё меню, горячие клавиши
и т.п.), а связь появляется, как только пользователь откроет Modly.

Готовые SDK делают всё описанное ниже за вас:

| Язык | Файл | Для чего |
|---|---|---|
| C# | [`sdk/unity/Modly.cs`](../sdk/unity/Modly.cs) | Unity (Mono/IL2CPP), BepInEx, MelonLoader, любые .NET-игры |
| C++ | [`sdk/cpp/modly.hpp`](../sdk/cpp/modly.hpp) | Unreal Engine (UE4SS), инжектируемые DLL, свои движки |
| Rust | [`sdk/rust`](../sdk/rust) | моды и инструменты на Rust |

Этот документ — для тех, кто пишет на другом языке (Lua, Python, Go…) или хочет понимать, что внутри.

## Транспорт

- **Именованный канал Windows** `\\.\pipe\modly`. Modly — сервер, мод — клиент (`CreateFile` с
  `GENERIC_READ | GENERIC_WRITE`, `OPEN_EXISTING`).
- Канал доступен только локально и только тому же пользователю Windows.
- Нет канала (Modly не запущен) — пробуйте снова раз в 1–3 секунды. `CreateFile` в этом случае
  мгновенно возвращает ошибку, так что ожидание ничего не стоит.
- **Сообщения — строки JSON в UTF-8, по одной на строку, с `\n` в конце** (NDJSON). Внутри строк JSON
  переводов строки не бывает (они экранируются как `\n`).
- Предел одного сообщения — 4 МиБ.
- Порядок сообщений в каждую сторону сохраняется.

> **Совет для синхронных каналов.** В Windows чтение и запись по одному синхронному хэндлу
> выполняются по очереди: пока висит `ReadFile`, `WriteFile` из другого потока ждёт. SDK Modly
> поэтому читают только то, что уже пришло (`PeekNamedPipe` → `ReadFile`), и пишут из того же потока.

## Жизненный цикл

```
мод                                   Modly
 │ ── hello ─────────────────────────▶ │  первым сообщением, иначе Modly закроет канал
 │ ◀──────────────────────── welcome ── │  язык Modly и версия
 │ ── layout ────────────────────────▶ │  весь интерфейс (можно присылать заново в любой момент)
 │ ── state / patch / notify … ──────▶ │  значения меняются в игре
 │ ◀──────────────────── set / invoke ── │  пользователь что-то изменил или нажал
 │ ── result ────────────────────────▶ │  (необязательно) итог: ок / ошибка + текст
 │ ── bye ───────────────────────────▶ │  (необязательно) мод выгружается
```

`hello` нужно прислать в течение 10 секунд после подключения.

## Сообщения мода → Modly

### `hello`

```json
{"type":"hello","protocol":1,
 "mod":{"id":"my-mod","name":"My Mod","version":"1.0.0","author":"me","description":"…","url":"https://…"},
 "game":{"id":"my-game","name":"My Game","exe":"MyGame.exe"},
 "pid":12345,"sdk":"my-sdk/1.0"}
```

| Поле | | |
|---|---|---|
| `protocol` | обязательно | версия протокола, сейчас `1` |
| `mod.id` | обязательно | постоянный идентификатор мода (латиница, `-`) |
| `mod.name`, `mod.version` | обязательно | для показа пользователю |
| `mod.author`, `mod.description`, `mod.url`, `mod.icon` | | `description` — [текст](#текст) |
| `game` | | к какой игре относится мод; по `game.id` Modly связывает мод с пакетами модов |
| `pid` | желательно | id процесса игры: по нему Modly находит, из какой копии игры подключился мод |
| `sdk` | | название и версия SDK — видно в консоли разработчика |

### `layout`

Весь интерфейс мода: страницы → секции → элементы. Повторный `layout` полностью заменяет прежний.

```json
{"type":"layout","pages":[
  {"id":"player","title":{"en":"Player","ru":"Игрок"},"icon":"user","sections":[
    {"id":"cheats","title":"Cheats","description":"Turn them on and off","controls":[
      {"id":"god","kind":"toggle","label":"God mode","icon":"shield","value":false},
      {"id":"speed","kind":"slider","label":"Speed","min":0.5,"max":5,"step":0.1,"unit":"×","value":1},
      {"id":"heal","kind":"button","label":"Heal to full","tone":"accent"}
    ]}
  ]}
]}
```

Подробно об элементах — в разделе [«Элементы»](#элементы).

### `state` — новые значения

```json
{"type":"state","values":{"god":true,"speed":2.5,"health":87}}
```

Присылайте, когда значение изменилось в игре (не чаще 5–10 раз в секунду на элемент — SDK Modly
сравнивают значения раз в 200 мс и шлют только изменившиеся).

### `patch` — изменить свойства элементов

```json
{"type":"patch","controls":{"summon":{"enabled":false},"item":{"options":[{"value":"log","label":"Log"}]}}}
```

Можно менять: `label`, `description`, `icon`, `enabled`, `visible`, `options`, `min`, `max`, `step`,
`presets`, `unit`, `tone`, `confirm`. Не указанные поля не трогаются.

### `notify` — всплывающее уведомление

```json
{"type":"notify","tone":"success","text":"Summoned 5 pups"}
```

### `status` — строка состояния мода

```json
{"type":"status","tone":"warning","text":"Load a save to use the mod"}
```

Показывается над панелью мода, пока не придёт `status` с пустым `text`.

### `result` — ответ на `set` / `invoke`

```json
{"type":"result","req":42,"ok":false,"text":"Not enough room"}
```

Необязателен. `ok: false` Modly покажет ошибкой, `ok: true` с `text` — уведомлением об успехе.

### `log` — строка в консоль разработчика Modly

```json
{"type":"log","tone":"warning","text":"Template not found, using fallback"}
```

### `paused` — игра стоит

```json
{"type":"paused","paused":true}
```

Многие игры (например, Unity с выключенным *Run In Background*) замирают, когда их окно не активно.
Команды из Modly тогда выполнятся, только когда игрок вернётся в игру. Сообщите об этом — Modly
покажет пояснение. SDK Modly делают это сами: если главный поток не вызывал `update()` дольше
1,5 с — шлют `paused: true`, когда снова пошёл — `paused: false`.

### `pong`, `bye`

`pong` — ответ на `ping`. `bye` — мод отключается сам (необязательно: закрытия канала достаточно).

## Сообщения Modly → мод

| Сообщение | Пример | Смысл |
|---|---|---|
| `welcome` | `{"type":"welcome","protocol":1,"app":"Modly","version":"0.1.0","lang":"ru"}` | ответ на `hello` |
| `set` | `{"type":"set","req":7,"id":"speed","value":2.5}` | пользователь изменил значение |
| `invoke` | `{"type":"invoke","req":8,"id":"heal"}` | пользователь нажал кнопку |
| `map_action` | `{"type":"map_action","req":9,"map":"island","action":"tp","x":250,"y":-120}` | действие на карте, см. [«Карты»](#карты) |
| `lang` | `{"type":"lang","lang":"en"}` | пользователь сменил язык Modly |
| `ping` | `{"type":"ping"}` | проверка связи — ответьте `pong` |
| `error` | `{"type":"error","text":"…"}` | ошибка протокола, после неё Modly закрывает канал |

После `set` пришлите `state` с настоящим значением (например, если вы его ограничили): Modly сразу
показывает новое значение, не дожидаясь ответа, и поправит его по вашему `state`.

**Выполняйте `set` и `invoke` на главном потоке игры.** Удобнее всего — складывать входящие сообщения
в очередь из потока канала и разбирать её в игровом цикле (так делают SDK).

## Карты

Мод может показать в Modly карты своей игры: картинку, категории и метки — и двигать живые метки
(игрок, питомцы), пока игра идёт. Карта видна на странице игры и во вкладке «Карта» оверлея. Карту
можно положить и в пакет мода (`maps` в [modly.json](MANIFEST.md)) — тогда она видна и без игры.

### `map` — карта целиком

```json
{"type":"map","id":"island","title":{"en":"Island","ru":"Остров"},
 "image":"C:\\Games\\MyGame\\Mods\\island.png",
 "bounds":{"left":-800,"top":533,"right":800,"bottom":-533},
 "categories":[{"id":"places","label":"Places","icon":"map-pin","color":"#8b5cf6"}],
 "markers":[
   {"id":"mill","x":-112,"y":32,"cat":"places","title":"Old mill","actions":[{"id":"tp","label":"Teleport","tone":"accent"}]},
   {"id":"player","x":10,"y":-5,"style":"dot","color":"#22d3ee","title":"You"}],
 "click_actions":[{"id":"tp","label":"Teleport here"}],
 "focus":"player"}
```

| Поле | | |
|---|---|---|
| `id` | обязательно | карта с тем же `id` заменяет прежнюю |
| `title` | | [текст](#текст) |
| `image` | обязательно | полный путь к PNG, JPEG или WebP на этом компьютере или `data:image/png;base64,…`; до 32 МБ. Сетевые пути (`\\сервер\…`) не открываются |
| `bounds` | | края картинки в координатах игры: `left`, `top`, `right`, `bottom`. Оси — в любую сторону: если y растёт вверх, `top` больше `bottom`. Нет — координаты в пикселях картинки |
| `categories` | | `[{id, label, icon, color, hidden}]`; `color` — `#rrggbb`, `hidden` — скрыта, пока игрок её не включит |
| `markers` | | `[{id, x, y, cat, title, text, icon, color, style, actions}]`; `style` — `pin` (булавка, по умолчанию) или `dot` (точка — для того, что движется) |
| `click_actions` | | действия по клику в пустое место карты |
| `focus` | | id метки для кнопки «Где я» (обычно игрок) |

Действие — `{id, label, icon, tone, confirm}`; с `confirm` (или `tone: "danger"`) срабатывает со второго нажатия.

### `map_patch` — изменить метки

```json
{"type":"map_patch","map":"island","set":[{"id":"player","x":12,"y":-4,"style":"dot","title":"You"}],"remove":["chest-7"]}
```

`set` — добавить или заменить метки (целиком, по `id`), `remove` — убрать. Так двигаются живые метки:
SDK Modly присылают только изменившиеся и не чаще раза в 200 мс.

### `map_remove` — убрать карту

```json
{"type":"map_remove","map":"island"}
```

### `map_action` — действие на карте (Modly → мод)

```json
{"type":"map_action","req":12,"map":"island","action":"tp","marker":"mill","x":-112,"y":32}
{"type":"map_action","req":13,"map":"island","action":"tp","x":250.5,"y":-120}
```

Кнопка у метки — с `marker` и её местом; клик в пустое место — без `marker`, а `x`, `y` — точка в
координатах игры (по `bounds`). Ответ — `result`, как на `invoke`.

Пределы: 16 карт у мода, 5000 меток на карте, 64 категории, по 8 действий. Лишнее Modly отбрасывает,
метки с ошибкой в полях пропускает.

## Текст

Любой текст для пользователя — либо строка, либо переводы:

```json
"label": "Speed"
"label": {"en": "Speed", "ru": "Скорость", "de": "Geschwindigkeit"}
```

Modly покажет перевод на своём языке, иначе английский, иначе первый попавшийся. Язык Modly приходит
в `welcome` и `lang` — если хотите, присылайте тексты сразу на нём.

## Оттенки (`tone`)

`default`, `muted`, `accent`, `info`, `success`, `warning`, `danger` (`error` — то же, что `danger`).
Неизвестное значение читается как `default`.

## Элементы

Общие поля любого элемента:

| Поле | | |
|---|---|---|
| `id` | обязательно | уникален в пределах мода, не меняется |
| `kind` | обязательно | вид элемента, см. таблицу |
| `label` | | подпись ([текст](#текст)) |
| `description` | | пояснение мелким шрифтом |
| `icon` | | имя [иконки](#иконки) |
| `enabled` | `true` | можно ли сейчас пользоваться |
| `visible` | `true` | показывать ли |
| `value` | | текущее значение (тип — по виду элемента) |
| `hotkey` | | предлагаемая горячая клавиша для `toggle` и `button`, например `"F1"`, `"Ctrl+Num1"` ([подробнее](#горячие-клавиши)) |

| `kind` | Значение | Свои поля | Как выглядит |
|---|---|---|---|
| `toggle` | `bool` | — | переключатель |
| `button` | строка — надпись на кнопке (необязательно) | `tone`, `confirm` | кнопка; с `confirm` срабатывает со второго нажатия |
| `slider` | число | `min`, `max` (обязательны), `step`, `unit` | ползунок со значением |
| `number` | число | `min`, `max`, `step` (1), `presets: [числа]`, `unit` | поле с −/+ и быстрыми вариантами |
| `text` | строка | `placeholder`, `max_length` | поле ввода, значение уходит по Enter |
| `select` | строка; с `multiple` — массив строк | `options: [{value, label, icon, description}]`, `multiple` | до 4 вариантов — сегменты, больше или `multiple` — сетка |
| `stat` | строка или число | `unit`, `tone` | показатель только для чтения |
| `progress` | число 0…1 | — | полоса прогресса |
| `note` | — | `tone` | пояснение; текст — в `label` |
| `separator` | — | — | разделитель |

Элементы неизвестного вида (или с ошибкой в полях) Modly пропускает, а не отбрасывает весь интерфейс, —
так новые виды элементов не ломают старые версии Modly. Неизвестные поля игнорируются.

## Горячие клавиши

Переключателю (`toggle`) и кнопке (`button`) пользователь может назначить глобальную горячую клавишу
в панели Modly — она срабатывает и тогда, когда активно окно игры. Нажатие переключателя Modly
отправляет моду обычным `set` с обратным значением, нажатие кнопки — `invoke`; моду ничего
дополнительно делать не нужно. Кнопка с `confirm` (или `tone: "danger"`) срабатывает от второго нажатия
в течение 3 секунд.

Поле `hotkey` — предложение мода: оно действует, пока пользователь сам ничего не назначил (или не снял
клавишу). Формат: модификаторы `Ctrl`, `Alt`, `Shift`, `Win` через `+` и клавиша — `A`…`Z`, `0`…`9`,
`F1`…`F24`, `Num0`…`Num9`, `Num+`, `Num-`, `Num*`, `Num/`, `Num.`, `Space`, `Enter`, `Tab`, `Insert`,
`Delete`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `Pause`, `ScrollLock`
и знаки `` ` - = [ ] \ ; ' , . / ``. Регистр не важен: `"ctrl+num1"` = `"Ctrl+Num1"`. Неразборчивая строка
игнорируется. Клавиши держатся, только пока мод подключён.

## Иконки

В Modly встроен набор иконок, их можно указывать по имени:

`alert axe bolt book box check chevron-down chevron-left chevron-right chicken clock cloud code coins copy crosshair
dot download droplet egg external eye fish flame folder gamepad gauge gem globe grid hammer heart home
house info keyboard layers leaf link lock magnet map-pin me message minus moon paw pause phone pig play plug
plus power puzzle rain refresh rocket search send shield shovel skull sliders snowflake spawn sparkles
star stop sun sword target terminal trash tree user wand wolf wrench x`

И синонимы: `settings`→`sliders`, `health`→`heart`, `god`→`shield`, `money`/`gold`→`coins`,
`speed`→`gauge`, `teleport`→`map-pin`, `build`→`hammer`, `magic`→`wand`, `item`→`box`,
`animal`/`pet`→`paw`, `water`→`droplet`, `fire`→`flame`, `kill`→`skull`, `freeze`→`snowflake`,
`harvest`→`axe`, `dig`→`shovel`, `player`→`user`, `time`→`clock`, `weather`→`cloud` и другие
(полный список — `crates/modly-app/src/icons.rs`). Неизвестное имя — без иконки.

## Минимальный мод на Python

Канал можно открыть как обычный файл. Здесь чтение и запись идут по очереди из одного потока, поэтому
синхронный канал не мешает:

```python
import json, time

def send(pipe, msg):
    pipe.write((json.dumps(msg, ensure_ascii=False) + "\n").encode("utf-8"))

god = False
while True:
    try:
        pipe = open(r"\\.\pipe\modly", "r+b", buffering=0)
    except OSError:
        time.sleep(2)            # Modly не запущен — ждём
        continue
    send(pipe, {"type": "hello", "protocol": 1, "mod": {"id": "py-demo", "name": "Python Demo", "version": "1.0"}})
    send(pipe, {"type": "layout", "pages": [{"id": "main", "title": "Main", "sections": [{"id": "s", "controls": [
        {"id": "god", "kind": "toggle", "label": "God mode", "value": god},
        {"id": "hello", "kind": "button", "label": "Say hello", "tone": "accent"},
    ]}]}]})
    buf = b""
    try:
        while True:
            chunk = pipe.read(4096)
            if not chunk:
                break
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                msg = json.loads(line)
                if msg["type"] == "set" and msg["id"] == "god":
                    god = bool(msg["value"])
                    send(pipe, {"type": "state", "values": {"god": god}})
                elif msg["type"] == "invoke" and msg["id"] == "hello":
                    send(pipe, {"type": "notify", "tone": "success", "text": "Hello from Python!"})
                elif msg["type"] == "ping":
                    send(pipe, {"type": "pong"})
    except OSError:
        pass                     # Modly закрылся — подключимся снова
```

## Совместимость

- Новые необязательные поля и виды элементов добавляются без смены `protocol`.
- Несовместимые изменения повышают `protocol`; Modly отвечает `error`, если мод новее, чем он понимает.
- Отладка: **Настройки → Режим разработчика** включает консоль протокола на странице «Для мододелов» —
  там видно каждое сообщение в обе стороны.
