# Что нового в Modly

Формат: раздел `## <версия> — <дата>`, в нём `### Русский` и `### English`. Отсюда `cargo xtask dist`
берёт текст выпуска на GitHub и в обновлении внутри Modly.

## 0.2.3-beta — 2026-10-23

### Русский
- **Игры находятся и без модов.** Раньше Modly искал игры только по пакетам модов и библиотекам Steam, а теперь
  узнаёт и движок игры: Unity, Unreal, Godot, GameMaker и Ren'Py. После обновления Modly один раз поищет игры заново.
- Игру не из Steam, убранную из библиотеки, новый поиск обратно не добавит. Если добавить её вручную, Modly снова
  будет её находить.
- На странице игры есть блок **«В каталоге для этой игры»**: моды с сайта, которых в этой игре ещё нет, ставятся
  прямо отсюда.
- Копия Modly с отдельной папкой данных (`MODLY_HOME`) запускается рядом с основной.

### English
- **Games are found without mods, too.** Modly used to find games only by mod packages and Steam libraries. Now
  it also recognises the game engine: Unity, Unreal, Godot, GameMaker and Ren'Py. After the update Modly searches
  for games once more.
- A non-Steam game you removed from the library stays removed after a new search. If you add it by hand, Modly
  finds it again.
- A game's page has an **"In the catalog for this game"** block: mods from the site that this game doesn't have
  yet install right from there.
- A Modly copy with its own data folder (`MODLY_HOME`) starts next to the main one.

## 0.2.2-beta — 2026-10-23

### Русский
- Установщик и обновлятор, запущенные изнутри другого приложения (например, из терминала приложения из
  Microsoft Store), перезапускаются через Проводник: Windows прятала записи такого запуска в песочницу
  приложения, и Modly не появлялся в «Приложения и возможности».
- Тихое удаление (`--uninstall --quiet` — так удаляют winget и другие менеджеры программ) больше не оставляет
  файлы в папке Modly.

### English
- The installer and updater started from inside another app (for example, a terminal of a Microsoft Store app)
  relaunch through Explorer: Windows sandboxed the registry writes of such a run, and Modly didn't show up in
  Apps & features.
- Quiet uninstall (`--uninstall --quiet`, used by winget and other package managers) no longer leaves files in the
  Modly folder.

## 0.2.1-beta — 2026-10-23

### Русский
- «Что нового» в обновлении — аккуратным текстом, без разметки.
- Кольцо прогресса в установщике и обновляторе идёт от верха.

### English
- Update notes are shown as clean text, without markup.
- The progress ring in the installer and updater starts at the top.

## 0.2.0-beta — 2026-10-23

### Русский
- **Установщик** `ModlySetup-0.2.0-beta.exe` в стиле Modly: ставит для текущего пользователя без прав
  администратора, можно выбрать папку, ярлыки на рабочем столе и в «Пуске», ссылки `modly://`.
  Modly появляется в «Приложения и возможности». По желанию — инструменты мододела: .NET SDK и
  C++ Build Tools (через winget).
- **Modly обновляется сам.** Проверяет GitHub при запуске и раз в 6 часов; новая версия — кнопка
  «Обновить» в настройках и в боковой панели. Обновление подписано ключом выпусков (Ed25519), Modly
  сверяет подпись и SHA-256. Если новая версия не запустится, вернётся прежняя.
- **Обновлятор** `modly-updater.exe` рядом с Modly: ставит обновления и удаляет Modly (можно оставить
  или удалить настройки и библиотеку).
- Каналы обновлений: бета-версии или только выпуски; автопроверку можно выключить.
- Среда выполнения C++ теперь внутри exe — Visual C++ Redistributable не нужен.

### English
- **Installer** `ModlySetup-0.2.0-beta.exe` in Modly's style: installs for the current user without
  administrator rights, lets you pick the folder, desktop and Start menu shortcuts and `modly://` links.
  Modly shows up in Apps & features. Optional modder tools: .NET SDK and C++ Build Tools (via winget).
- **Modly updates itself.** It checks GitHub at start and every 6 hours; a new version gets an
  "Update" button in Settings and in the sidebar. Updates are signed with the release key (Ed25519);
  Modly checks the signature and SHA-256. If the new version fails to start, the previous one comes back.
- **Updater** `modly-updater.exe` next to Modly installs updates and uninstalls Modly (keep or remove
  your settings and library).
- Update channels: betas or releases only; automatic checks can be turned off.
- The C++ runtime is now built into the exe — no Visual C++ Redistributable needed.

## 0.1.0-beta — 2026-10-23

### Русский
- Первый публичный выпуск: библиотека игр, установка модов в один клик, панель мода во время игры,
  оверлей, пульт с телефона, каталог mods.hackerstone.xyz, SDK для C#, C++ и Rust.

### English
- First public release: game library, one-click mod install, live mod panel, overlay, phone remote,
  the mods.hackerstone.xyz catalog, SDKs for C#, C++ and Rust.
