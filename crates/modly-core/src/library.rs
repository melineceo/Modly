//! Библиотека игр: что Modly знает о играх на этом ПК и как их находит.

use crate::manifest::{GameDef, expand};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    /// Идентификатор записи (из пути к папке) — у каждой копии игры свой.
    pub id: String,
    /// Идентификатор игры из пакетов модов (`family-time-ep3`), если игра известна.
    #[serde(default)]
    pub def_id: Option<String>,
    pub name: String,
    pub dir: PathBuf,
    pub exe: String,
    #[serde(default)]
    pub manual: bool,
    /// Игра из Steam: номер приложения (обложки из кэша Steam, запуск через Steam).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_appid: Option<u32>,
    /// Все описания игр из пакетов модов, под которые подходит эта папка (см. [`Library::link_defs`]).
    /// Авторы называют одну игру по-разному (`family-time-ep3`, `familytime`) — мод относится к копии
    /// по её папке, а не по совпадению названий.
    #[serde(skip)]
    pub defs: Vec<String>,
}

impl Game {
    pub fn new(dir: &Path, exe: &str, name: &str, def_id: Option<String>, manual: bool) -> Self {
        Game {
            id: game_id(dir),
            def_id,
            name: name.to_owned(),
            dir: dir.to_path_buf(),
            exe: exe.to_owned(),
            manual,
            steam_appid: None,
            defs: Vec::new(),
        }
    }

    /// Мод с этим описанием игры (`game.id` из пакета) подходит этой копии.
    pub fn has_def(&self, id: &str) -> bool {
        self.def_id.as_deref() == Some(id) || self.defs.iter().any(|d| d == id)
    }

    pub fn exe_path(&self) -> PathBuf {
        self.dir.join(&self.exe)
    }

    pub fn exists(&self) -> bool {
        self.exe_path().is_file()
    }

    pub fn engine(&self) -> Engine {
        detect_engine(&self.dir, &self.exe)
    }

    /// Имя папки — чтобы различать несколько копий одной игры.
    pub fn folder_name(&self) -> String {
        self.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

pub fn game_id(dir: &Path) -> String {
    crate::stable_id(&dir.to_string_lossy().to_lowercase().replace('/', "\\"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    UnityMono,
    UnityIl2Cpp,
    Unreal,
    Godot,
    GameMaker,
    RenPy,
    Unknown,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::UnityMono => "Unity · Mono",
            Engine::UnityIl2Cpp => "Unity · IL2CPP",
            Engine::Unreal => "Unreal Engine",
            Engine::Godot => "Godot",
            Engine::GameMaker => "GameMaker",
            Engine::RenPy => "Ren'Py",
            Engine::Unknown => "",
        }
    }
}

pub fn detect_engine(dir: &Path, exe: &str) -> Engine {
    let data = dir.join(expand("{data}", exe));
    if data.is_dir() {
        if dir.join("GameAssembly.dll").is_file() || data.join("il2cpp_data").is_dir() {
            return Engine::UnityIl2Cpp;
        }
        return Engine::UnityMono;
    }
    if dir.join("Engine").join("Binaries").is_dir()
        || std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| e.path().join("Binaries").join("Win64").is_dir())
    {
        return Engine::Unreal;
    }
    let stem = exe.rsplit_once('.').map(|(s, _)| s).unwrap_or(exe);
    if dir.join(format!("{stem}.pck")).is_file() {
        return Engine::Godot;
    }
    if dir.join("data.win").is_file() {
        return Engine::GameMaker;
    }
    if dir.join("renpy").is_dir() && dir.join("game").is_dir() {
        return Engine::RenPy;
    }
    Engine::Unknown
}

/// Служебный exe рядом с игрой: установщики, отчёты об ошибках, античиты, лаунчеры обновлений.
pub fn is_helper_exe(file: &str) -> bool {
    const HELPERS: &[&str] = &[
        "unins", "setup", "redist", "vcredist", "vc_redist", "dxsetup", "crashhandler", "crashreport", "crashpad",
        "easyanticheat", "eac_", "beservice", "battleye", "uninstall", "prereq", "dotnet", "directx", "physx",
        "oalinst", "cefprocess", "helper", "update", "patcher", "dxwebsetup", "touchup", "cleanup", "installer", "downloader",
    ];
    let lower = file.to_lowercase();
    HELPERS.iter().any(|h| lower.contains(h))
}

/// Игра, найденная по признакам движка, — без пакета мода.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub dir: PathBuf,
    pub exe: String,
    pub name: String,
    pub engine: Engine,
}

/// Папка похожа на установленную игру: Unity (`<exe>_Data` с данными игры), Unreal
/// (`<Игра>/Binaries/Win64/*-Shipping.exe` и папка `Engine`), Godot (`<exe>.pck`),
/// GameMaker (`data.win`) или Ren'Py (`renpy` и `game`). `entries` — содержимое папки (имя, папка ли).
pub fn detect_game_in(dir: &Path, entries: &[(String, bool)]) -> Option<Detected> {
    let has_dir = |n: &str| entries.iter().any(|(e, d)| *d && e.eq_ignore_ascii_case(n));
    let has_file = |n: &str| entries.iter().any(|(e, d)| !*d && e.eq_ignore_ascii_case(n));
    let exes: Vec<&str> = entries
        .iter()
        .filter(|(e, d)| !*d && e.to_lowercase().ends_with(".exe") && !is_helper_exe(e))
        .map(|(e, _)| e.as_str())
        .collect();
    let stem = |e: &str| e[..e.len() - 4].to_owned();
    let found = |exe: &str, engine: Engine| {
        let name = match engine {
            Engine::UnityMono | Engine::UnityIl2Cpp => guess_name(dir, exe),
            _ => pretty_name(dir, exe),
        };
        Some(Detected { dir: dir.to_path_buf(), exe: exe.to_owned(), name, engine })
    };

    // Unity: рядом с exe — папка <exe>_Data с данными игры
    for exe in &exes {
        let data_name = format!("{}_Data", stem(exe));
        if has_dir(&data_name) {
            let data = dir.join(&data_name);
            if ["globalgamemanagers", "data.unity3d", "mainData"].iter().any(|f| data.join(f).is_file())
                || data.join("Managed").is_dir()
                || data.join("il2cpp_data").is_dir()
            {
                return found(exe, detect_engine(dir, exe));
            }
        }
    }
    // Unreal (собранная игра): папка Engine и <Игра>/Binaries/Win64/<...>-Shipping.exe
    if has_dir("Engine") {
        for (sub, is_dir) in entries {
            if !*is_dir || sub.eq_ignore_ascii_case("Engine") {
                continue;
            }
            let bin = dir.join(sub).join("Binaries").join("Win64");
            let shipping = std::fs::read_dir(&bin).into_iter().flatten().flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.ends_with(".exe") && n.contains("-shipping")
            });
            if shipping {
                // запускают игру через exe в корне (<Игра>.exe), без него — сам Shipping (не лаунчер)
                let root_exe = exes.iter().find(|e| stem(e).eq_ignore_ascii_case(sub)).map(|e| e.to_string());
                let exe = root_exe.or_else(|| {
                    std::fs::read_dir(&bin).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| {
                        let l = n.to_lowercase();
                        l.ends_with(".exe") && l.contains("-shipping")
                    }).map(|n| format!("{sub}/Binaries/Win64/{n}"))
                })?;
                return found(&exe, Engine::Unreal);
            }
        }
    }
    // Godot: <exe>.pck рядом
    for exe in &exes {
        if has_file(&format!("{}.pck", stem(exe))) {
            return found(exe, Engine::Godot);
        }
    }
    // GameMaker и Ren'Py: главный exe — самый подходящий из папки
    let main = || main_exe(dir, &dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .filter(|(d, _)| d == dir)
        .map(|(_, e)| e);
    if has_file("data.win") && !exes.is_empty() {
        return found(&main()?, Engine::GameMaker);
    }
    if has_dir("renpy") && has_dir("game") && !exes.is_empty() {
        return found(&main()?, Engine::RenPy);
    }
    None
}

/// Название игры не из Unity: имя папки, если оно похоже на название (`Hello Neighbor`), иначе — exe,
/// разбитый по словам (`HelloNeighbor.exe` → «Hello Neighbor»).
pub fn pretty_name(dir: &Path, exe: &str) -> String {
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if folder.contains(' ') && !folder.contains('_') && folder.chars().any(char::is_alphabetic) {
        return folder;
    }
    let stem = Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = stem.split("-Win64").next().unwrap_or(&stem).to_owned();
    let mut out = String::new();
    let chars: Vec<char> = stem.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            out.push(' ');
            continue;
        }
        let prev = if i > 0 { chars[i - 1] } else { ' ' };
        if i > 0 && c.is_uppercase() && (prev.is_lowercase() || prev.is_ascii_digit()) {
            out.push(' ');
        }
        out.push(c);
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { folder } else { out }
}

/// Название игры для папки без пакета модов: из `app.info` Unity (вторая строка — продукт)
/// или из имени exe.
pub fn guess_name(dir: &Path, exe: &str) -> String {
    let info = dir.join(expand("{data}", exe)).join("app.info");
    if let Ok(text) = std::fs::read_to_string(info) {
        if let Some(product) = text.lines().nth(1).map(str::trim).filter(|s| !s.is_empty()) {
            return product.to_owned();
        }
    }
    exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_owned()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub games: Vec<Game>,
}

impl Library {
    pub fn load() -> Self {
        std::fs::read(crate::paths::library_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let data = serde_json::to_vec_pretty(self).expect("library serialize");
        if let Err(e) = crate::write_atomic(&crate::paths::library_file(), &data) {
            log::warn!("library save failed: {e}");
        }
    }

    pub fn get(&self, id: &str) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }

    /// Добавить, если такой папки ещё нет. Возвращает true, если добавлено.
    pub fn add(&mut self, game: Game) -> bool {
        if self.games.iter().any(|g| g.id == game.id) {
            return false;
        }
        self.games.push(game);
        self.sort();
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.games.retain(|g| g.id != id);
    }

    pub fn sort(&mut self) {
        self.games.sort_by(|a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.dir.cmp(&b.dir))
        });
    }

    /// Привязать записи к описаниям игр из пакетов модов: запомнить все подходящие
    /// (и назвать игру по первому, если пакет появился после добавления игры).
    pub fn link_defs(&mut self, defs: &[GameDef]) {
        for g in &mut self.games {
            let mut ids: Vec<String> = Vec::new();
            for d in defs.iter().filter(|d| game_matches_def(g, d)) {
                if !ids.contains(&d.id) {
                    ids.push(d.id.clone());
                }
            }
            g.defs = ids;
            if g.def_id.is_none() {
                if let Some(d) = defs.iter().find(|d| g.defs.contains(&d.id)) {
                    g.def_id = Some(d.id.clone());
                    g.name = d.name.clone();
                }
            }
        }
    }

    /// Запись по пути к exe (например, у подключившегося мода).
    pub fn find_by_exe(&self, exe: &Path) -> Option<&Game> {
        let want = exe.to_string_lossy().to_lowercase();
        self.games.iter().find(|g| g.exe_path().to_string_lossy().to_lowercase() == want)
    }
}

/// Папка подходит под описание игры: есть exe и все файлы из `detect`.
pub fn matches_def(dir: &Path, def: &GameDef) -> bool {
    dir.join(&def.exe).is_file() && def.detect.iter().all(|f| dir.join(expand(f, &def.exe)).exists())
}

/// Копия игры подходит под описание: по папке ([`matches_def`]) или — для Steam — по номеру
/// приложения, если в папке есть нужный exe.
pub fn game_matches_def(game: &Game, def: &GameDef) -> bool {
    matches_def(&game.dir, def)
        || (def.steam_appid.is_some() && def.steam_appid == game.steam_appid && game.dir.join(&def.exe).is_file())
}

/// Файл, по которому узнаётся сборка игры: `build_file` из описания или код игры по движку.
pub fn build_file(game: &Game, def: &GameDef) -> PathBuf {
    if let Some(f) = &def.build_file {
        return game.dir.join(expand(f, &game.exe));
    }
    match game.engine() {
        Engine::UnityMono => game.dir.join(expand("{managed}/Assembly-CSharp.dll", &game.exe)),
        Engine::UnityIl2Cpp => game.dir.join("GameAssembly.dll"),
        _ => game.exe_path(),
    }
}

/// Отпечаток сборки — SHA-256 файла (строчные шестнадцатеричные знаки).
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ---------------------------------------------------------------------------------------------
// Поиск игр

/// Где искать игры: рабочий стол, загрузки, документы, папки Games и Program Files на дисках, папки в корне
/// дисков, библиотеки Steam и itch. Копия игры не из Steam находится так же — по exe и файлам `detect`.
pub fn default_roots() -> Vec<(PathBuf, usize)> {
    let mut roots: Vec<(PathBuf, usize)> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        for (sub, depth) in [("Desktop", 3), ("Downloads", 3), ("Documents", 2), ("Games", 3)] {
            roots.push((home.join(sub), depth));
        }
        // OneDrive переносит рабочий стол
        roots.push((home.join("OneDrive").join("Desktop"), 3));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        roots.push((appdata.join("itch").join("apps"), 2));
    }
    for drive in b'C'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", drive as char));
        if !root.exists() {
            continue;
        }
        // Program Files — туда ставят копии не из Steam и репаки (`Program Files (x86)\R.G. Mechanics\Игра`)
        for sub in [
            "Games",
            "Игры",
            "Program Files",
            "Program Files (x86)",
            "Program Files (x86)\\Steam\\steamapps\\common",
            "SteamLibrary\\steamapps\\common",
        ] {
            roots.push((root.join(sub), 2));
        }
        // и папки прямо в корне диска: `D:\Hello Neighbor`
        roots.push((root, 1));
    }
    for lib in steam_libraries() {
        roots.push((lib.join("steamapps").join("common"), 2));
    }
    roots.retain(|(p, _)| p.is_dir());
    // пути в Windows без учёта регистра: Steam из реестра пишет `c:\program files (x86)\steam`
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    roots.sort_by(|a, b| key(&a.0).cmp(&key(&b.0)).then(b.1.cmp(&a.1)));
    roots.dedup_by(|a, b| key(&a.0) == key(&b.0));
    roots
}

/// Папки библиотек Steam из реестра и `libraryfolders.vdf`.
fn steam_libraries() -> Vec<PathBuf> {
    #[cfg(windows)]
    let steam = crate::win::steam_path();
    #[cfg(not(windows))]
    let steam: Option<PathBuf> = None;
    let Some(steam) = steam else { return Vec::new() };
    let mut libs = vec![steam.clone()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        for line in vdf.lines() {
            let parts: Vec<&str> = line.split('"').collect();
            // строка вида:   "path"		"D:\\SteamLibrary"
            if parts.len() >= 4 && parts[1] == "path" {
                libs.push(PathBuf::from(parts[3].replace("\\\\", "\\")));
            }
        }
    }
    libs
}

// ---------------------------------------------------------------------------------------------
// Steam

/// Установленное приложение Steam (из `steamapps/appmanifest_<id>.acf`).
#[derive(Clone, Debug, PartialEq)]
pub struct SteamApp {
    pub appid: u32,
    pub name: String,
    /// Папка игры (`steamapps/common/<installdir>`).
    pub dir: PathBuf,
}

/// Не игры: библиотеки времени выполнения, Proton, SteamVR, распространяемые пакеты.
const STEAM_NOT_GAMES: &[u32] = &[228980, 1070560, 1391110, 1628350, 1493710, 2180100, 250820, 1826330, 2805730, 1580130];

/// Все установленные приложения Steam во всех его библиотеках.
pub fn steam_apps() -> Vec<SteamApp> {
    let mut out: Vec<SteamApp> = Vec::new();
    for lib in steam_libraries() {
        let apps = lib.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&apps) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            if let Some(app) = parse_manifest(&text, &apps) {
                if !out.iter().any(|a| a.appid == app.appid) {
                    out.push(app);
                }
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Разобрать `appmanifest_*.acf`: только полностью установленные игры, без служебных пакетов.
fn parse_manifest(text: &str, steamapps: &Path) -> Option<SteamApp> {
    let appid: u32 = acf_value(text, "appid")?.parse().ok()?;
    let name = acf_value(text, "name")?;
    let installdir = acf_value(text, "installdir")?;
    let flags: u32 = acf_value(text, "StateFlags").and_then(|f| f.parse().ok()).unwrap_or(0);
    if flags & 4 == 0 || STEAM_NOT_GAMES.contains(&appid) {
        return None;
    }
    let lower = name.to_lowercase();
    if ["redistributable", "steam linux runtime", "proton ", "steamworks", "steamvr", "dedicated server"].iter().any(|w| lower.contains(w)) {
        return None;
    }
    if installdir.is_empty() || installdir.contains(['/', '\\']) || installdir.contains("..") {
        return None;
    }
    let dir = steamapps.join("common").join(&installdir);
    dir.is_dir().then_some(SteamApp { appid, name, dir })
}

/// Значение ключа в текстовом VDF/ACF: строка вида `"key"   "value"` (первое вхождение).
fn acf_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 && parts[1].eq_ignore_ascii_case(key) {
            return Some(parts[3].replace("\\\\", "\\"));
        }
    }
    None
}

/// Главный exe игры в папке: по имени папки и названию игры, по папке данных Unity, по размеру.
/// Служебные exe (установщики, отчёты об ошибках, античиты) не подходят. Возвращает (папка, имя exe):
/// если в корне игры подходящего exe нет — ищет на уровень глубже (`bin`, `Game` и т.п.).
pub fn main_exe(dir: &Path, name: &str) -> Option<(PathBuf, String)> {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let want_dir = norm(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let want_name = norm(name);
    let pick = |folder: &Path| -> Option<(i64, String)> {
        let mut best: Option<(i64, u64, String)> = None;
        for e in std::fs::read_dir(folder).ok()?.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let lower = file.to_lowercase();
            if !lower.ends_with(".exe") || is_helper_exe(&file) {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let stem = norm(file.trim_end_matches(".exe").trim_end_matches(".EXE"));
            let mut score = 0i64;
            if !stem.is_empty() && (stem == want_dir || stem == want_name) {
                score += 100;
            } else if !stem.is_empty() && (want_name.contains(&stem) || stem.contains(&want_name) || want_dir.contains(&stem)) {
                score += 40;
            }
            if folder.join(format!("{}_Data", file.trim_end_matches(".exe").trim_end_matches(".EXE"))).is_dir() {
                score += 80;
            }
            if lower.contains("launcher") {
                score -= 50;
            }
            if best.as_ref().is_none_or(|(s, sz, _)| (score, size) > (*s, *sz)) {
                best = Some((score, size, file));
            }
        }
        best.map(|(s, _, f)| (s, f))
    };
    if let Some((_, exe)) = pick(dir) {
        return Some((dir.to_path_buf(), exe));
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort();
    subs.into_iter().filter_map(|s| pick(&s).map(|(score, exe)| (score, s, exe))).max_by_key(|(score, _, _)| *score).map(|(_, s, e)| (s, e))
}

/// Картинки игры из кэша Steam.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SteamArt {
    /// Шапка 460×215 — для плиток библиотеки.
    Header,
    /// Широкий баннер — для страницы игры.
    Hero,
}

/// Файл картинки игры в `appcache/librarycache` (новая раскладка — по папкам, старая — `<id>_header.jpg`).
pub fn steam_art(appid: u32, art: SteamArt) -> Option<PathBuf> {
    #[cfg(windows)]
    let steam = crate::win::steam_path()?;
    #[cfg(not(windows))]
    let steam: PathBuf = return None;
    let cache = steam.join("appcache").join("librarycache");
    let names: &[&str] = match art {
        SteamArt::Header => &["header.jpg", "library_header.jpg"],
        SteamArt::Hero => &["library_hero.jpg"],
    };
    let dir = cache.join(appid.to_string());
    let mut dirs = vec![dir.clone()];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for name in names {
        for d in &dirs {
            let p = d.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let flat = match art {
        SteamArt::Header => format!("{appid}_header.jpg"),
        SteamArt::Hero => format!("{appid}_library_hero.jpg"),
    };
    Some(cache.join(flat)).filter(|p| p.is_file())
}

const SKIP_DIRS: &[&str] = &[
    "windows", "$recycle.bin", "system volume information", "appdata", "node_modules", ".git", "target",
    "programdata", "msocache", "recovery", "perflogs", "__pycache__", ".cache",
];

/// Обойти папки и найти игры из `defs`. Возвращает (индекс описания, папка игры).
pub fn scan(defs: &[GameDef], roots: &[(PathBuf, usize)]) -> Vec<(usize, PathBuf)> {
    scan_all(defs, roots).0
}

/// Обойти папки: игры из `defs` (пакеты модов) и любые другие игры по признакам движка
/// ([`detect_game_in`]). В найденную по движку игру поиск не заходит — внутри только её файлы.
pub fn scan_all(defs: &[GameDef], roots: &[(PathBuf, usize)]) -> (Vec<(usize, PathBuf)>, Vec<Detected>) {
    let mut found: Vec<(usize, PathBuf)> = Vec::new();
    let mut detected: Vec<Detected> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().cloned().collect();
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        visited += 1;
        if visited > 60_000 {
            break;
        }
        let mut by_def = false;
        for (i, def) in defs.iter().enumerate() {
            if matches_def(&dir, def) && !found.iter().any(|(_, d)| d == &dir) {
                found.push((i, dir.clone()));
                by_def = true;
            }
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let entries: Vec<(String, bool)> = rd
            .flatten()
            .take(2000)
            .filter_map(|e| {
                let ft = e.file_type().ok()?;
                (!ft.is_symlink()).then(|| (e.file_name().to_string_lossy().into_owned(), ft.is_dir()))
            })
            .collect();
        if !by_def {
            if let Some(game) = detect_game_in(&dir, &entries) {
                if !detected.iter().any(|d| d.dir == game.dir) {
                    detected.push(game);
                }
                continue;
            }
        }
        if depth == 0 {
            continue;
        }
        for (name, is_dir) in &entries {
            let lower = name.to_lowercase();
            if !*is_dir || SKIP_DIRS.contains(&lower.as_str()) || lower.ends_with("_data") || lower.starts_with('.') {
                continue;
            }
            queue.push_back((dir.join(name), depth - 1));
        }
    }
    (found, detected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> GameDef {
        GameDef {
            id: "ft".into(),
            name: "Family Time".into(),
            exe: "FamilyTime.exe".into(),
            detect: vec!["{managed}/Assembly-CSharp.dll".into()],
            ..Default::default()
        }
    }

    #[test]
    fn reads_steam_manifests_and_picks_main_exe() {
        let root = tempfile::tempdir().unwrap();
        let apps = root.path().join("steamapps");
        let game = apps.join("common").join("Cool Game");
        std::fs::create_dir_all(game.join("CoolGame_Data")).unwrap();
        std::fs::write(game.join("CoolGame.exe"), vec![0u8; 10]).unwrap();
        std::fs::write(game.join("UnityCrashHandler64.exe"), vec![0u8; 999]).unwrap();
        std::fs::write(game.join("unins000.exe"), vec![0u8; 9999]).unwrap();
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"123\"\n\t\"name\"\t\t\"Cool Game\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"Cool Game\"\n}";
        let app = parse_manifest(acf, &apps).unwrap();
        assert_eq!(app, SteamApp { appid: 123, name: "Cool Game".into(), dir: game.clone() });
        assert_eq!(main_exe(&game, "Cool Game"), Some((game.clone(), "CoolGame.exe".into())));
        // недокачанная игра, служебный пакет, чужой путь — не берём
        // установлена и при этом обновляется (4 | 1024) — игра есть
        assert!(parse_manifest(&acf.replace("\t\"StateFlags\"\t\t\"4\"", "\t\"StateFlags\"\t\t\"1028\""), &apps).is_some());
        assert!(parse_manifest(&acf.replace("\t\"StateFlags\"\t\t\"4\"", "\t\"StateFlags\"\t\t\"2\""), &apps).is_none());
        assert!(parse_manifest(&acf.replace("\"123\"", "\"228980\""), &apps).is_none());
        assert!(parse_manifest(&acf.replace("\"Cool Game\"\n}", "\"..\\\\x\"\n}"), &apps).is_none());
    }

    #[test]
    fn main_exe_looks_one_level_deeper() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("Big Game");
        std::fs::create_dir_all(game.join("bin")).unwrap();
        std::fs::write(game.join("bin").join("BigGame.exe"), b"MZ").unwrap();
        std::fs::write(game.join("readme.txt"), b"x").unwrap();
        assert_eq!(main_exe(&game, "Big Game"), Some((game.join("bin"), "BigGame.exe".into())));
    }

    #[test]
    fn scans_and_matches_unity_game() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("Games").join("FT copy");
        std::fs::create_dir_all(game.join("FamilyTime_Data/Managed")).unwrap();
        std::fs::write(game.join("FamilyTime.exe"), b"MZ").unwrap();
        std::fs::write(game.join("FamilyTime_Data/Managed/Assembly-CSharp.dll"), b"x").unwrap();
        std::fs::write(game.join("FamilyTime_Data/app.info"), "sgthale\nFamily Time").unwrap();
        // похожая папка без нужных файлов — не игра
        let fake = root.path().join("Fake");
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(fake.join("FamilyTime.exe"), b"MZ").unwrap();

        let found = scan(&[def()], &[(root.path().to_path_buf(), 3)]);
        assert_eq!(found, vec![(0, game.clone())]);
        assert_eq!(detect_engine(&game, "FamilyTime.exe"), Engine::UnityMono);
        assert_eq!(guess_name(&game, "FamilyTime.exe"), "Family Time");

        let mut lib = Library::default();
        assert!(lib.add(Game::new(&game, "FamilyTime.exe", "Family Time", None, false)));
        assert!(!lib.add(Game::new(&game, "FamilyTime.exe", "Family Time", None, false)));
        lib.link_defs(&[def()]);
        assert_eq!(lib.games[0].def_id.as_deref(), Some("ft"));
        assert!(lib.find_by_exe(&game.join("familytime.exe")).is_some());

        // другой автор назвал ту же игру иначе — его мод тоже относится к этой копии
        let other = GameDef { id: "familytime".into(), ..def() };
        lib.link_defs(&[def(), other]);
        assert!(lib.games[0].has_def("ft") && lib.games[0].has_def("familytime"));
        assert_eq!(lib.games[0].def_id.as_deref(), Some("ft"));

        // Steam: тот же номер приложения и exe — та же игра, даже без файлов из detect
        let steam = GameDef { id: "ft-steam".into(), detect: vec!["missing.file".into()], steam_appid: Some(42), ..def() };
        lib.link_defs(std::slice::from_ref(&steam));
        assert!(!lib.games[0].has_def("ft-steam"));
        lib.games[0].steam_appid = Some(42);
        lib.link_defs(&[steam]);
        assert!(lib.games[0].has_def("ft-steam"));

        // без пакета мода игра находится по движку
        let (by_def, by_engine) = scan_all(&[], &[(root.path().to_path_buf(), 3)]);
        assert!(by_def.is_empty());
        assert_eq!(by_engine.len(), 1);
        assert_eq!((by_engine[0].dir.clone(), by_engine[0].exe.as_str(), by_engine[0].name.as_str()), (game.clone(), "FamilyTime.exe", "Family Time"));
        assert_eq!(by_engine[0].engine, Engine::UnityMono);

        // сборка: по умолчанию у Unity Mono — Assembly-CSharp.dll
        let file = build_file(&lib.games[0], &def());
        assert_eq!(file, game.join("FamilyTime_Data/Managed/Assembly-CSharp.dll"));
        assert_eq!(file_sha256(&file).unwrap(), "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881");
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"MZ").unwrap();
    }

    #[test]
    fn finds_games_by_engine_and_skips_tools() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        // Unreal: Engine + <Игра>/Binaries/Win64/*-Shipping.exe, запуск через exe в корне
        touch(&r.join("Hello Neighbor/HelloNeighbor.exe"));
        touch(&r.join("Hello Neighbor/HelloNeighbor/Binaries/Win64/HelloNeighbor-Win64-Shipping.exe"));
        std::fs::create_dir_all(r.join("Hello Neighbor/Engine/Binaries")).unwrap();
        // Godot, GameMaker, Ren'Py
        touch(&r.join("Brotato/Brotato.exe"));
        touch(&r.join("Brotato/Brotato.pck"));
        touch(&r.join("gm_game/RiskOfRain.exe"));
        touch(&r.join("gm_game/data.win"));
        touch(&r.join("DDLC/DDLC.exe"));
        std::fs::create_dir_all(r.join("DDLC/renpy")).unwrap();
        std::fs::create_dir_all(r.join("DDLC/game")).unwrap();
        // не игры: лаунчер Epic (без *-Shipping), редактор Unreal, Unity без данных игры, просто exe
        touch(&r.join("Epic Games/Launcher/Portal/Binaries/Win64/EpicGamesLauncher.exe"));
        std::fs::create_dir_all(r.join("Epic Games/Launcher/Engine/Binaries")).unwrap();
        touch(&r.join("Epic Games/UE_5.3/Engine/Binaries/Win64/UnrealEditor.exe"));
        touch(&r.join("Tool/Tool.exe"));
        std::fs::create_dir_all(r.join("Tool/Tool_Data")).unwrap();
        touch(&r.join("Apps/notepad2.exe"));

        let (_, mut found) = scan_all(&[], &[(r.to_path_buf(), 3)]);
        found.sort_by(|a, b| a.dir.cmp(&b.dir));
        let got: Vec<(String, String, Engine)> = found
            .iter()
            .map(|d| (d.dir.file_name().unwrap().to_string_lossy().into_owned(), d.exe.clone(), d.engine))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Brotato".into(), "Brotato.exe".into(), Engine::Godot),
                ("DDLC".into(), "DDLC.exe".into(), Engine::RenPy),
                ("Hello Neighbor".into(), "HelloNeighbor.exe".into(), Engine::Unreal),
                ("gm_game".into(), "RiskOfRain.exe".into(), Engine::GameMaker),
            ]
        );
        assert_eq!(found.iter().find(|d| d.engine == Engine::Unreal).unwrap().name, "Hello Neighbor");
        assert_eq!(found.iter().find(|d| d.engine == Engine::GameMaker).unwrap().name, "Risk Of Rain");
    }

    #[test]
    fn pretty_names() {
        assert_eq!(pretty_name(Path::new(r"C:\Games\Garry's Mod"), "hl2.exe"), "Garry's Mod");
        assert_eq!(pretty_name(Path::new(r"C:\Games\Family_Time_Ep_3_b108"), "FamilyTime.exe"), "Family Time");
        assert_eq!(pretty_name(Path::new(r"C:\x\HN"), "HelloNeighbor/Binaries/Win64/HelloNeighbor-Win64-Shipping.exe"), "Hello Neighbor");
        assert_eq!(pretty_name(Path::new(r"C:\x\k93"), "Kuindzhi93.exe"), "Kuindzhi93");
    }
}

#[cfg(test)]
mod local_check {
    /// Ручная проверка на этом ПК: какие игры найдутся по движку —
    /// `cargo test -p modly-core local_engines -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn local_engines() {
        let started = std::time::Instant::now();
        let (_, found) = super::scan_all(&[], &super::default_roots());
        for d in &found {
            println!("{:<16} {:<36} {}  [{}]", d.engine.label(), d.name, d.dir.display(), d.exe);
        }
        println!("{} games in {:?}", found.len(), started.elapsed());
    }

    /// Ручная проверка на этом ПК: `cargo test -p modly-core local_steam -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn local_steam() {
        for app in super::steam_apps() {
            let exe = super::main_exe(&app.dir, &app.name);
            let art = super::steam_art(app.appid, super::SteamArt::Header).is_some();
            println!("{:>8}  {:<40} exe={:?} header={art}", app.appid, app.name, exe.map(|(d, e)| d.join(e)));
        }
    }
}
