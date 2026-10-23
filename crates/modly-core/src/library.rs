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
    Unknown,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::UnityMono => "Unity · Mono",
            Engine::UnityIl2Cpp => "Unity · IL2CPP",
            Engine::Unreal => "Unreal Engine",
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
    Engine::Unknown
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
    const HELPERS: &[&str] = &[
        "unins", "setup", "redist", "vcredist", "vc_redist", "dxsetup", "crashhandler", "crashreport", "crashpad",
        "easyanticheat", "eac_", "beservice", "battleye", "uninstall", "prereq", "dotnet", "directx", "physx",
        "oalinst", "cefprocess", "helper", "update", "patcher", "dxwebsetup", "touchup", "cleanup", "installer", "downloader",
    ];
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let want_dir = norm(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let want_name = norm(name);
    let pick = |folder: &Path| -> Option<(i64, String)> {
        let mut best: Option<(i64, u64, String)> = None;
        for e in std::fs::read_dir(folder).ok()?.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let lower = file.to_lowercase();
            if !lower.ends_with(".exe") || HELPERS.iter().any(|h| lower.contains(h)) {
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
    let mut found: Vec<(usize, PathBuf)> = Vec::new();
    if defs.is_empty() {
        return found;
    }
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().cloned().collect();
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        visited += 1;
        if visited > 60_000 {
            break;
        }
        for (i, def) in defs.iter().enumerate() {
            if matches_def(&dir, def) && !found.iter().any(|(_, d)| d == &dir) {
                found.push((i, dir.clone()));
            }
        }
        if depth == 0 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if SKIP_DIRS.contains(&name.as_str()) || name.ends_with("_data") || name.starts_with('.') {
                continue;
            }
            queue.push_back((entry.path(), depth - 1));
        }
    }
    found
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

        // сборка: по умолчанию у Unity Mono — Assembly-CSharp.dll
        let file = build_file(&lib.games[0], &def());
        assert_eq!(file, game.join("FamilyTime_Data/Managed/Assembly-CSharp.dll"));
        assert_eq!(file_sha256(&file).unwrap(), "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881");
    }
}

#[cfg(test)]
mod local_check {
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
