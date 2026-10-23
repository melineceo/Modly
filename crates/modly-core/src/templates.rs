//! «Сделать мод» для любой игры из библиотеки: готовая папка проекта с SDK внутри.
//!
//! Шаблоны встроены в Modly (исходников Modly у игрока нет):
//! - Unity на Mono — C#-мод, который собирается компилятором .NET SDK прямо против DLL этой игры
//!   и ставится без загрузчиков (способ `unity-mono`);
//! - всё остальное (Unreal, Unity IL2CPP, свои движки) — C++ DLL с `modly.hpp`; как её загрузить
//!   в игру (ASI Loader, UE4SS) — в README проекта.
//!
//! В обоих — `modly.json` с этой игрой: Modly сразу видит мод на странице игры, собирает его кнопкой
//! «Собрать» и ставит кнопкой «Установить».

use crate::library::{Engine, Game};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::path::{Path, PathBuf};

const MODLY_CS: &str = include_str!("../../../sdk/unity/Modly.cs");
const MODLY_HPP: &str = include_str!("../../../sdk/cpp/modly.hpp");
const UNITY_MOD_CS: &str = include_str!("../../../examples/unity-mod/src/ExampleMod.cs");
const UNITY_BUILD_PS1: &str = include_str!("../../../examples/unity-mod/build.ps1");

/// Что получилось.
#[derive(Debug)]
pub struct Created {
    pub dir: PathBuf,
    /// Шаблон на C# (Unity Mono) или на C++.
    pub csharp: bool,
}

/// Папка по умолчанию для своих модов: «Документы\Modly Mods» (в портативном и тестовом режиме,
/// `MODLY_HOME`, — в папке данных Modly).
pub fn default_root() -> PathBuf {
    if std::env::var_os("MODLY_HOME").is_some() {
        return crate::paths::data_dir().join("my-mods");
    }
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Documents").join("Modly Mods"))
        .unwrap_or_else(|| crate::paths::data_dir().join("my-mods"))
}

/// Латиница и цифры для имени класса и папки: «Red Dead Redemption 2» → `RedDeadRedemption2`.
fn ident(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(if upper { c.to_ascii_uppercase() } else { c });
            upper = false;
        } else {
            upper = true;
        }
    }
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert_str(0, "Game");
    }
    out.truncate(40);
    out
}

/// `RedDeadRedemption2` → `red-dead-redemption2` (id мода и игры).
fn kebab(id: &str) -> String {
    let mut out = String::new();
    for (i, c) in id.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Создать проект мода для игры в `root` (папка `<Игра>Mod`, `<Игра>Mod-2`… — существующие не трогаем).
pub fn create(game: &Game, root: &Path) -> Result<Created> {
    let class = format!("{}Mod", ident(&game.name));
    let mod_id = kebab(&class);
    let game_id = game.def_id.clone().unwrap_or_else(|| kebab(&ident(&game.name)));
    std::fs::create_dir_all(root).with_context(|| root.display().to_string())?;
    let mut dir = root.join(&class);
    for n in 2..100 {
        if !dir.exists() {
            break;
        }
        dir = root.join(format!("{class}-{n}"));
    }
    if dir.exists() {
        bail!("слишком много проектов с таким именем в {}", root.display());
    }
    let engine = game.engine();
    let csharp = engine == Engine::UnityMono;
    let write = |rel: &str, text: &str| -> Result<()> {
        let p = dir.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, text).with_context(|| p.display().to_string())
    };
    let game_def = json!({
        "id": game_id,
        "name": game.name,
        "exe": game.exe,
        "detect": if csharp { json!(["{managed}/Assembly-CSharp.dll"]) } else { json!([]) },
    });

    if csharp {
        let cs = UNITY_MOD_CS
            .replace("namespace ExampleMod", &format!("namespace {class}"))
            .replace("\"example-mod\"", &format!("\"{mod_id}\""))
            .replace("\"Example Mod\"", &format!("\"{} Mod\"", game.name.replace('"', "")))
            .replace("\"your-game\"", &format!("\"{game_id}\""))
            .replace("\"Your Game\"", &format!("\"{}\"", game.name.replace('"', "")))
            .replace("[ExampleMod]", &format!("[{class}]"))
            .replace("new GameObject(\"ExampleMod\")", &format!("new GameObject(\"{class}\")"));
        write(&format!("src/{class}.cs"), &cs)?;
        write("sdk/Modly.cs", MODLY_CS)?;
        let ps1 = UNITY_BUILD_PS1
            .replace("[string]$Name = 'ExampleMod'", &format!("[string]$Name = '{class}'"))
            .replace("'..\\..\\sdk\\unity\\Modly.cs'", "'sdk\\Modly.cs'");
        // BOM — чтобы Windows PowerShell 5 прочитал русские комментарии.
        write("build.ps1", &format!("\u{feff}{}", ps1.trim_start_matches('\u{feff}')))?;
        let manifest = json!({
            "id": mod_id,
            "name": format!("{} Mod", game.name),
            "version": "0.1.0",
            "author": "",
            "description": { "ru": format!("Мой мод для {}", game.name), "en": format!("My mod for {}", game.name) },
            "game": game_def,
            "install": { "method": "unity-mono", "assembly": format!("bin/{class}.dll"), "entry": format!("{class}.ModEntry.Init") },
            "dev": { "build": format!("powershell -NoProfile -ExecutionPolicy Bypass -File build.ps1 -GamePath \"{}\"", game.dir.display()) },
        });
        write("modly.json", &serde_json::to_string_pretty(&manifest)?)?;
        write("README.md", &readme_unity(&game.name, &class))?;
    } else {
        write("include/modly.hpp", MODLY_HPP)?;
        write("src/main.cpp", &cpp_main(&mod_id, &game.name, &game_id))?;
        write("build.bat", &cpp_build(&class))?;
        let manifest = json!({
            "id": mod_id,
            "name": format!("{} Mod", game.name),
            "version": "0.1.0",
            "author": "",
            "description": { "ru": format!("Мой мод для {}", game.name), "en": format!("My mod for {}", game.name) },
            "game": game_def,
            "install": { "method": "files", "files": [ { "from": format!("bin/{class}.asi"), "to": format!("{class}.asi") } ] },
            "dev": { "build": "build.bat" },
        });
        write("modly.json", &serde_json::to_string_pretty(&manifest)?)?;
        write("README.md", &readme_cpp(&game.name, &class, engine))?;
    }
    log::info!("mod template for {} → {}", game.name, dir.display());
    Ok(Created { dir, csharp })
}

fn readme_unity(game: &str, class: &str) -> String {
    format!(
        "# {game} — мой мод\n\n\
         Шаблон мода для Unity-игры на Mono. Мод ставится без загрузчиков: Modly прописывает DLL в списки Unity,\n\
         и игра сама вызывает `{class}.ModEntry.Init()` при старте.\n\n\
         ## Как собрать и попробовать\n\n\
         1. Установите .NET SDK 6 или новее: https://dotnet.microsoft.com/download\n\
         2. В Modly включите **Настройки → Режим разработчика**.\n\
         3. На странице игры у мода нажмите **Собрать**, потом **Установить** и запустите игру.\n\
         4. Пока игра запущена, переключатели и ползунки мода появляются в Modly — меняйте `src/{class}.cs`.\n\n\
         Сборка идёт против DLL самой игры: если чего-то из Unity API в ней нет, компилятор сразу это покажет.\n\n\
         ## Выложить мод\n\n\
         Упакуйте папку (с `modly.json` и `bin/{class}.dll`) в ZIP и загрузите в «Студию» на https://mods.hackerstone.xyz —\n\
         после проверки модератором его можно будет поставить из каталога Modly одной кнопкой.\n\n\
         Документация: https://mods.hackerstone.xyz/docs.php\n"
    )
}

fn readme_cpp(game: &str, class: &str, engine: Engine) -> String {
    let how = match engine {
        Engine::Unreal => "Для Unreal Engine удобнее всего загрузить DLL через UE4SS (раздел C++-модов) или Ultimate ASI Loader.",
        Engine::UnityIl2Cpp => "Это Unity на IL2CPP: C#-мод без загрузчика тут не сработает. DLL можно загрузить через Ultimate ASI Loader; для C#-модов — MelonLoader или BepInEx IL2CPP.",
        _ => "DLL загружается в игру через Ultimate ASI Loader: положите его `dinput8.dll` (или `version.dll`) рядом с exe игры — он подхватит наш `.asi`.",
    };
    format!(
        "# {game} — мой мод\n\n\
         Шаблон мода на C++: DLL, которая при загрузке в игру подключается к Modly и показывает в нём свои\n\
         переключатели, ползунки и кнопки (SDK — `include/modly.hpp`, только C++17 и WinAPI).\n\n\
         ## Как собрать\n\n\
         1. Установите Visual Studio 2022 (или Build Tools) с компонентом «Разработка классических приложений на C++».\n\
         2. В Modly включите **Настройки → Режим разработчика** и на странице игры нажмите **Собрать** —\n\
            или запустите `build.bat` сами. Получится `bin/{class}.asi` (это обычная DLL).\n\n\
         ## Как загрузить в игру\n\n\
         {how}\n\n\
         Кнопка **Установить** в Modly кладёт `{class}.asi` в папку игры (см. `install` в `modly.json`).\n\n\
         ## Выложить мод\n\n\
         Упакуйте папку с `modly.json` и `bin/` в ZIP и загрузите в «Студию» на https://mods.hackerstone.xyz.\n\n\
         Документация: https://mods.hackerstone.xyz/docs.php\n"
    )
}

fn cpp_main(mod_id: &str, game: &str, game_id: &str) -> String {
    let game = game.replace(['"', '\\'], "");
    format!(
        "// Мод на C++: DLL, которая подключается к Modly из игры.\n\
         #include \"modly.hpp\"\n\n\
         #include <atomic>\n\
         #include <thread>\n\
         #include <windows.h>\n\n\
         namespace {{\n\
         std::atomic<bool> g_running{{true}};\n\
         bool g_enabled = true;\n\
         float g_speed = 1.0f;\n\n\
         void mod_thread() {{\n\
         \x20   modly::Client modly({{\"{mod_id}\", \"{game} Mod\", \"0.1.0\", \"\", \"{game_id}\", \"{game}\"}});\n\
         \x20   auto& main = modly.page(\"main\", modly::Text(\"Main\").ru(\"Главное\"), \"star\").section(\"main\");\n\
         \x20   main.toggle(\"enabled\", modly::Text(\"Mod enabled\").ru(\"Мод включён\"), &g_enabled).icon(\"power\");\n\
         \x20   main.slider(\"speed\", modly::Text(\"Speed\").ru(\"Скорость\"), 0.5, 5.0, &g_speed).step(0.1).unit(\"x\").icon(\"gauge\");\n\
         \x20   modly.start();\n\
         \x20   while (g_running) {{\n\
         \x20       // Здесь — ваш код: читайте и меняйте память игры, хукайте функции и т.п.\n\
         \x20       modly.update();\n\
         \x20       std::this_thread::sleep_for(std::chrono::milliseconds(16));\n\
         \x20   }}\n\
         }}\n\
         }}  // namespace\n\n\
         BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {{\n\
         \x20   if (reason == DLL_PROCESS_ATTACH) {{\n\
         \x20       DisableThreadLibraryCalls(module);\n\
         \x20       std::thread(mod_thread).detach();\n\
         \x20   }} else if (reason == DLL_PROCESS_DETACH) {{\n\
         \x20       g_running = false;\n\
         \x20   }}\n\
         \x20   return TRUE;\n\
         }}\n"
    )
}

fn cpp_build(class: &str) -> String {
    // Без блоков `if (...)`: скобка в %ProgramFiles(x86)% закрыла бы такой блок раньше времени.
    format!(
        "@echo off\r\n\
         rem Сборка DLL компилятором MSVC (Visual Studio 2022 / Build Tools).\r\n\
         chcp 65001 >nul\r\n\
         setlocal\r\n\
         cd /d \"%~dp0\"\r\n\
         where cl >nul 2>nul\r\n\
         if not errorlevel 1 goto build\r\n\
         set \"VSWHERE=%ProgramFiles(x86)%\\Microsoft Visual Studio\\Installer\\vswhere.exe\"\r\n\
         if not exist \"%VSWHERE%\" goto novs\r\n\
         for /f \"usebackq tokens=*\" %%i in (`\"%VSWHERE%\" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set \"VS=%%i\"\r\n\
         if not defined VS goto novs\r\n\
         call \"%VS%\\VC\\Auxiliary\\Build\\vcvars64.bat\" >nul\r\n\
         :build\r\n\
         if not exist bin mkdir bin\r\n\
         cl /nologo /std:c++17 /EHsc /O2 /utf-8 /LD /Iinclude src\\main.cpp /Fo:bin\\ /Fe:bin\\{class}.asi user32.lib\r\n\
         if errorlevel 1 exit /b 1\r\n\
         echo OK: bin\\{class}.asi\r\n\
         exit /b 0\r\n\
         :novs\r\n\
         echo Не найден Visual Studio с компонентом C++. Build Tools: https://visualstudio.microsoft.com/visual-cpp-build-tools/\r\n\
         exit /b 1\r\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ModManifest;

    #[test]
    fn names_are_safe() {
        assert_eq!(ident("Red Dead Redemption 2"), "RedDeadRedemption2");
        assert_eq!(ident("Sand:box"), "SandBox");
        assert_eq!(ident("7 Days to Die"), "Game7DaysToDie");
        assert_eq!(ident("Игра"), "Game");
        assert_eq!(kebab("RedDeadRedemption2Mod"), "red-dead-redemption2-mod");
    }

    #[test]
    fn unity_template_is_a_valid_package() {
        let root = tempfile::tempdir().unwrap();
        let game_dir = root.path().join("Cool Game");
        std::fs::create_dir_all(game_dir.join("CoolGame_Data/Managed")).unwrap();
        std::fs::write(game_dir.join("CoolGame.exe"), b"MZ").unwrap();
        let game = Game::new(&game_dir, "CoolGame.exe", "Cool Game", None, false);
        let out = create(&game, &root.path().join("mods")).unwrap();
        assert!(out.csharp);
        let m = ModManifest::load(&out.dir).unwrap();
        assert_eq!(m.id, "cool-game-mod");
        assert_eq!(m.game.exe, "CoolGame.exe");
        assert!(crate::library::matches_def(&game_dir, &m.game) || !game_dir.join("CoolGame_Data/Managed/Assembly-CSharp.dll").exists());
        let cs = std::fs::read_to_string(out.dir.join("src/CoolGameMod.cs")).unwrap();
        assert!(cs.contains("namespace CoolGameMod") && cs.contains("\"cool-game-mod\"") && !cs.contains("ExampleMod"));
        let ps1 = std::fs::read_to_string(out.dir.join("build.ps1")).unwrap();
        assert!(ps1.contains("'sdk\\Modly.cs'") && ps1.contains("'CoolGameMod'"));
        assert!(out.dir.join("sdk/Modly.cs").is_file());
        // второй раз — новая папка, первая не тронута
        let again = create(&game, &root.path().join("mods")).unwrap();
        assert_ne!(again.dir, out.dir);
    }

    #[test]
    fn other_engines_get_a_cpp_template() {
        let root = tempfile::tempdir().unwrap();
        let game_dir = root.path().join("Native");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(game_dir.join("native.exe"), b"MZ").unwrap();
        let game = Game::new(&game_dir, "native.exe", "Native Game", None, false);
        let out = create(&game, root.path()).unwrap();
        assert!(!out.csharp);
        let m = ModManifest::load(&out.dir).unwrap();
        assert_eq!(m.id, "native-game-mod");
        let cpp = std::fs::read_to_string(out.dir.join("src/main.cpp")).unwrap();
        assert!(cpp.contains("modly::Client modly({\"native-game-mod\", \"Native Game Mod\""));
        assert!(out.dir.join("include/modly.hpp").is_file() && out.dir.join("build.bat").is_file());
    }
}

#[cfg(test)]
mod local_check {
    /// Ручная проверка сборки C++-шаблона на этом ПК:
    /// `MODLY_TEMPLATE_OUT=<папка> cargo test -p modly-core local_cpp_template -- --ignored`.
    #[test]
    #[ignore]
    fn local_cpp_template() {
        let out = std::path::PathBuf::from(std::env::var("MODLY_TEMPLATE_OUT").expect("MODLY_TEMPLATE_OUT"));
        let game_dir = out.join("FakeNative");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(game_dir.join("native.exe"), b"MZ").unwrap();
        let game = crate::library::Game::new(&game_dir, "native.exe", "Hello Neighbor", None, false);
        let created = super::create(&game, &out).unwrap();
        println!("{}", created.dir.display());
    }
}
