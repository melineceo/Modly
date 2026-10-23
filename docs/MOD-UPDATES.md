# Обновления мода

Мод может сам узнавать о новых версиях на [mods.hackerstone.xyz](https://mods.hackerstone.xyz) и ставить их —
даже когда Modly не запущен или мод поставлен вручную. Для C++-модов (Unreal, инжектируемые DLL, свои движки)
есть готовый компонент — один заголовок [`sdk/cpp/modly_update.hpp`](../sdk/cpp/modly_update.hpp), только то, что
есть в Windows: WinHTTP, BCrypt и `tar.exe`.

## Чему верит мод

1. **Только одобренные выпуски.** Список берётся из API сайта (`/api/v1/mods/<адрес>`) по https, а сайт
   отдаёт только выпуски, которые проверил модератор.
2. **SHA-256.** Скачанный архив сверяется с суммой из ответа сайта.
3. **Подпись автора.** Рядом с архивом на сайте лежит `<архив>.sig` — подпись RSA-3072 (PKCS#1 v1.5, SHA-256)
   строки `modly-mod-release\n<адрес>\n<версия>\n<sha256>\n`. Открытый ключ вшит в мод, закрытый есть только
   у автора, поэтому подменить обновление нельзя даже получив доступ к сайту. Выпуск без подписи мод не ставит.
4. **Файлы — по `modly.json` из архива** (`install.files`: `from` → `to`, пути от папки игры, без подстановок
   вида `{plugins}`). Пути с `..` и абсолютные отклоняются.
5. **Занятые файлы.** DLL мода держит игра, поэтому старый файл переименовывается в `*.old` (Windows это
   разрешает), а новый включается при следующем запуске игры. Список `*.old` компонент запоминает
   и удаляет при следующем запуске (`RemoveOldFiles`); при любой ошибке старые файлы возвращаются на место.

## Подключить (C++)

```cpp
#include "modly_update.hpp"

namespace mu = modly::update;
static constexpr const char* kAuthorKeyN = "…"; // из cargo xtask mod-keygen
static mu::Updater g_updates({"my-mod", "1.0.0", kAuthorKeyN, "010001"});

// при запуске мода
mu::RemoveOldFiles("my-mod");
// в своём фоновом потоке: при запуске игры (через 20–30 с) и раз в 6 часов
if (settings.updateCheck && g_updates.CheckSync()) notify("Доступна версия " + g_updates.Get().version);
// кнопка «Обновить»: пути из modly.json — от папки игры
g_updates.Install(gameRoot);
// для меню: g_updates.Get() — состояние, версия, что нового (ru/en), прогресс, ошибка
```

Заголовок сам подключает `winhttp.lib` и `bcrypt.lib`. Готовый пример — Neighborly (мод для Hello Neighbor):
уведомление в игре, карточка «Обновления» в меню, значок в шапке, переключатель «Проверять обновления».

## Ключ автора и подпись выпуска

Нужен репозиторий Modly (команды — в `xtask`):

```
cargo xtask mod-keygen                        # один раз: ключ автора и открытый ключ для мода
cargo xtask mod-sign dist\MyMod-1.1.0.zip     # рядом появится MyMod-1.1.0.zip.sig
```

Закрытый ключ — `%USERPROFILE%\.modly\mods-signing-key.pem` (или путь в `MODLY_MODS_KEY`), открытый —
рядом в `mods-signing-key.pub.txt`. Один ключ подходит ко всем модам автора. Сохраните резервную копию
закрытого ключа: без него новые версии не подписать, а моды со старым открытым ключом не примут другой.
Адрес и версию `mod-sign` берёт из `modly.json` в архиве (`id`, `version`), их можно задать `--slug` и `--version`.

## Выпуск на сайте

При новом выпуске в «Студии» прикрепите **два файла**: сначала архив, потом `.sig`. Сайт принимает `.sig`
только как строку hex, не показывает его отдельной кнопкой и не считает его скачивания, а на странице мода
появляется отметка «Выпуск подписан автором».

## Что сказать игрокам

- Проверка включена по умолчанию, её можно выключить в меню мода.
- В описании мода — что он ходит в интернет только за обновлениями и куда (mods.hackerstone.xyz).
- После установки — что новая версия включится при следующем запуске игры.

---

# Mod updates (English)

A mod can find and install its new versions from mods.hackerstone.xyz by itself — even without Modly running.
For C++ mods use the single header [`sdk/cpp/modly_update.hpp`](../sdk/cpp/modly_update.hpp) (WinHTTP, BCrypt
and Windows' `tar.exe` only).

What the mod trusts: only moderator-approved releases from the site API over https; the archive's SHA-256 from
the site; and the author's signature `<archive>.sig` — RSA-3072 PKCS#1 v1.5 / SHA-256 over
`modly-mod-release\n<slug>\n<version>\n<sha256>\n`, checked with the public key built into the mod. Files are
placed by `install.files` from the archive's `modly.json` (paths from the game folder). Files held by the game
are renamed to `*.old` and removed on the next start; on any error the old files come back.

Keys and signing (in the Modly repository): `cargo xtask mod-keygen` once, then
`cargo xtask mod-sign dist\MyMod-1.1.0.zip` for each release. On the site, attach the archive first and the
`.sig` second. Back up the private key (`%USERPROFILE%\.modly\mods-signing-key.pem`).
