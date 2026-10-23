//! Пакет мода — папка с `modly.json`. Формат описан в `docs/MANIFEST.md`.

use anyhow::{Context, Result, bail};
use modly_protocol::Text;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "modly.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// Взять версию из исходников: `{"file": "src/ModEntry.cs", "after": "Version = \""}` —
    /// версия — это текст после `after` до ближайшей кавычки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_from: Option<VersionFrom>,
    #[serde(default)]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
    /// Ссылки: `{"site": "...", "telegram": "...", "discord": "..."}`.
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    pub game: GameDef,
    pub install: InstallSpec,
    /// Только для разработчика мода (видно в режиме разработчика).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<DevSpec>,
    /// Карты игры от автора мода (видны и без запущенной игры), см. [`crate::maps`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps: Vec<crate::maps::PackageMap>,

    /// Папка пакета (заполняется при загрузке).
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionFrom {
    pub file: String,
    pub after: String,
}

/// Описание игры, для которой мод: по нему Modly находит игру на диске.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameDef {
    pub id: String,
    pub name: String,
    /// Имя exe, например `FamilyTime.exe`.
    pub exe: String,
    /// Файлы, которые должны быть рядом с exe, чтобы папка считалась этой игрой
    /// (поддерживаются подстановки, см. [`expand`]).
    #[serde(default)]
    pub detect: Vec<String>,
    /// Цвет обложки, например `"#7c5cff"`. Без него берётся из иконки игры.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// Номер игры в Steam: копия игры из Steam с этим номером и этим exe — та же игра,
    /// даже если файлы из `detect` у неё другие.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_appid: Option<u32>,
    /// Сборки игры, на которых автор проверил мод. Сборку Modly узнаёт по SHA-256 файла
    /// `build_file`; незнакомая сборка — предупреждение, а не запрет.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builds: Vec<GameBuild>,
    /// Файл, по которому узнаётся сборка (с подстановками). По умолчанию — код игры:
    /// `{managed}/Assembly-CSharp.dll` (Unity Mono), `GameAssembly.dll` (IL2CPP), иначе exe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_file: Option<String>,
}

/// Сборка игры, на которой проверен мод.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameBuild {
    /// Как называть сборку: `b108`, `1.4.2`, `Steam 2026-10`.
    pub name: String,
    /// SHA-256 файла сборки (`build_file`), 64 шестнадцатеричных знака.
    pub sha256: String,
}

impl GameDef {
    /// Сборка по отпечатку файла.
    pub fn build_named(&self, sha256: &str) -> Option<&GameBuild> {
        self.builds.iter().find(|b| b.sha256.eq_ignore_ascii_case(sha256))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case")]
pub enum InstallSpec {
    /// Unity (Mono) без загрузчиков: DLL кладётся в `<Игра>_Data/Managed`, а сборка и её метод
    /// прописываются в `ScriptingAssemblies.json` и `RuntimeInitializeOnLoads.json` — Unity сама
    /// загружает мод при старте. Метод должен быть `public static void` без параметров.
    UnityMono {
        /// Путь к DLL внутри пакета.
        assembly: String,
        /// `Пространство.Класс.Метод`, например `Creatly.ModEntry.Init`.
        entry: String,
        #[serde(default)]
        load: LoadTime,
        /// Дополнительные файлы.
        #[serde(default)]
        files: Vec<FileSpec>,
        /// Что удалить из папки игры при удалении мода (файлы; папки с данными игрока не трогаются).
        #[serde(default)]
        cleanup: Vec<String>,
        /// Старые версии мода под другими именами — убираются при установке и удалении.
        #[serde(default)]
        legacy: Vec<LegacyAssembly>,
    },
    /// Просто скопировать файлы (BepInEx/MelonLoader-плагины, pak-моды Unreal и т.п.).
    Files {
        files: Vec<FileSpec>,
        #[serde(default)]
        cleanup: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoadTime {
    /// После загрузки первой сцены (RuntimeInitializeLoadType.AfterSceneLoad = 0).
    #[default]
    AfterScene,
    /// До загрузки первой сцены (BeforeSceneLoad = 1).
    BeforeScene,
    /// Сразу после загрузки сборок (AfterAssembliesLoaded = 2).
    AfterAssemblies,
}

impl LoadTime {
    pub fn unity_value(self) -> u32 {
        match self {
            LoadTime::AfterScene => 0,
            LoadTime::BeforeScene => 1,
            LoadTime::AfterAssemblies => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileSpec {
    /// Путь внутри пакета.
    pub from: String,
    /// Путь внутри папки игры (с подстановками).
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyAssembly {
    /// Имя файла, например `FamilyTimeMod.dll`.
    pub assembly: String,
    pub entry: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DevSpec {
    /// Команда сборки (выполняется в папке пакета через `cmd /C`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl ModManifest {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&data);
        let mut m: ModManifest =
            serde_json::from_slice(data).with_context(|| format!("parse {}", path.display()))?;
        m.dir = dir.to_path_buf();
        if m.id.trim().is_empty() || m.name.trim().is_empty() {
            bail!("{}: пустые id или name", path.display());
        }
        m.check_paths().with_context(|| path.display().to_string())?;
        if let Some(vf) = &m.version_from {
            if let Some(v) = read_version(&dir.join(&vf.file), &vf.after) {
                m.version = v;
            }
        }
        Ok(m)
    }

    /// Все пути пакета — внутри пакета и внутри папки игры (см. [`safe_relative`]).
    fn check_paths(&self) -> Result<()> {
        let mut bad = Vec::new();
        let mut check = |p: &str| {
            if !safe_relative(p) {
                bad.push(p.to_owned());
            }
        };
        match &self.install {
            InstallSpec::UnityMono { assembly, files, cleanup, legacy, .. } => {
                check(assembly);
                for f in files {
                    check(&f.from);
                    check(&f.to);
                }
                cleanup.iter().for_each(|c| check(c));
                legacy.iter().for_each(|l| check(&l.assembly));
            }
            InstallSpec::Files { files, cleanup } => {
                for f in files {
                    check(&f.from);
                    check(&f.to);
                }
                cleanup.iter().for_each(|c| check(c));
            }
        }
        if let Some(vf) = &self.version_from {
            check(&vf.file);
        }
        for m in &self.maps {
            m.paths().into_iter().for_each(&mut check);
        }
        self.game.detect.iter().for_each(|d| check(d));
        if let Some(f) = &self.game.build_file {
            check(f);
        }
        if !bad.is_empty() {
            bail!("пути должны вести внутрь пакета и папки игры: {}", bad.join(", "));
        }
        for b in &self.game.builds {
            if b.name.trim().is_empty() || b.name.chars().count() > 40 {
                bail!("game.builds: у сборки нужно название до 40 знаков");
            }
            if b.sha256.len() != 64 || !b.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
                bail!("game.builds «{}»: sha256 — 64 шестнадцатеричных знака", b.name);
            }
        }
        Ok(())
    }

    /// Главный файл пакета (DLL), если он есть: по нему сверяется, свежая ли установка.
    pub fn primary_file(&self) -> Option<PathBuf> {
        match &self.install {
            InstallSpec::UnityMono { assembly, .. } => Some(self.dir.join(assembly)),
            InstallSpec::Files { files, .. } => files.first().map(|f| self.dir.join(&f.from)),
        }
    }

    /// Собран ли пакет (есть ли все исходные файлы).
    pub fn missing_files(&self) -> Vec<String> {
        let mut list = Vec::new();
        let check = |p: &str, list: &mut Vec<String>| {
            if !self.dir.join(p).is_file() {
                list.push(p.to_owned());
            }
        };
        match &self.install {
            InstallSpec::UnityMono { assembly, files, .. } => {
                check(assembly, &mut list);
                for f in files {
                    check(&f.from, &mut list);
                }
            }
            InstallSpec::Files { files, .. } => {
                for f in files {
                    check(&f.from, &mut list);
                }
            }
        }
        list
    }
}

/// Путь из `modly.json` относительный и не выходит за свою папку: без `..`, дисков, `\\сервер`
/// и потоков NTFS (`файл:поток`). Пакеты приходят и из интернета (каталог Modly) — без этой проверки
/// `"to": "../../…"` записал бы файл куда угодно на диске.
pub fn safe_relative(p: &str) -> bool {
    !p.trim().is_empty()
        && !p.contains(':')
        && !p.starts_with(['/', '\\'])
        // Windows отбрасывает точки и пробелы в конце имени: «...» и «.. » — тоже выход наверх.
        && !p.split(['/', '\\']).any(|part| part.chars().all(|c| c == '.' || c == ' ') && part.matches('.').count() >= 2)
}

fn read_version(file: &Path, after: &str) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let start = text.find(after)? + after.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let v = rest[..end].trim();
    (!v.is_empty() && v.len() < 64).then(|| v.to_owned())
}

/// Подстановки в путях внутри игры:
/// `{data}` — папка данных Unity (`<ИмяExe>_Data`), `{managed}` — `{data}/Managed`,
/// `{plugins}` — `BepInEx/plugins`.
pub fn expand(path: &str, exe: &str) -> String {
    let stem = exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(exe);
    let data = format!("{stem}_Data");
    path.replace("{managed}", &format!("{data}/Managed"))
        .replace("{data}", &data)
        .replace("{plugins}", "BepInEx/plugins")
}

/// Найти все пакеты в папках: каждая папка — либо сам пакет (с `modly.json`),
/// либо папка с пакетами в подпапках.
pub fn discover(dirs: &[PathBuf]) -> (Vec<ModManifest>, Vec<String>) {
    let mut mods: Vec<ModManifest> = Vec::new();
    let mut errors = Vec::new();
    let mut try_dir = |dir: &Path, mods: &mut Vec<ModManifest>| {
        if !dir.join(MANIFEST_FILE).is_file() {
            return;
        }
        match ModManifest::load(dir) {
            Ok(m) => {
                if !mods.iter().any(|x| x.id == m.id) {
                    mods.push(m);
                }
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    };
    for dir in dirs {
        if dir.join(MANIFEST_FILE).is_file() {
            try_dir(dir, &mut mods);
            continue;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        // Служебные папки (.downloads, .incoming-… при установке из каталога) — не пакеты.
        let mut subdirs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();
        for sub in subdirs {
            try_dir(&sub, &mut mods);
        }
    }
    (mods, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_unity_placeholders() {
        assert_eq!(expand("{managed}/Creatly.dll", "FamilyTime.exe"), "FamilyTime_Data/Managed/Creatly.dll");
        assert_eq!(expand("{data}/Creatly.cfg", "FamilyTime.exe"), "FamilyTime_Data/Creatly.cfg");
    }

    #[test]
    fn rejects_paths_outside_package_or_game() {
        assert!(safe_relative("bin/M.dll"));
        assert!(safe_relative("{managed}/M.dll"));
        assert!(safe_relative("BepInEx\\plugins\\M.dll"));
        assert!(safe_relative("./bin/M.dll"));
        for bad in ["../x.dll", "a/../../x", "a\\..\\x", "a/.../x", "a/.. /x", "/etc/x", "\\\\srv\\share\\x", "C:\\Windows\\x.dll", "x.dll:ads", "", "  "] {
            assert!(!safe_relative(bad), "{bad}");
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(MANIFEST_FILE),
            r#"{"id":"m","name":"M","game":{"id":"g","name":"G","exe":"G.exe"},
                "install":{"method":"files","files":[{"from":"m.dll","to":"../../Startup/m.exe"}]}}"#,
        )
        .unwrap();
        let err = ModManifest::load(root.path()).unwrap_err();
        assert!(format!("{err:#}").contains("../../Startup/m.exe"), "{err:#}");
    }

    #[test]
    fn game_builds_and_steam_appid() {
        let root = tempfile::tempdir().unwrap();
        let sha = "a".repeat(64);
        let write = |game: &str| {
            std::fs::write(
                root.path().join(MANIFEST_FILE),
                format!(r#"{{"id":"m","name":"M","game":{game},"install":{{"method":"files","files":[]}}}}"#),
            )
            .unwrap();
        };
        write(&format!(
            r#"{{"id":"g","name":"G","exe":"G.exe","steam_appid":42,"build_file":"{{data}}/x.dll","builds":[{{"name":"b108","sha256":"{}"}}]}}"#,
            sha.to_uppercase()
        ));
        let m = ModManifest::load(root.path()).unwrap();
        assert_eq!(m.game.steam_appid, Some(42));
        assert_eq!(m.game.build_named(&sha).map(|b| b.name.as_str()), Some("b108"));
        assert!(m.game.build_named(&"b".repeat(64)).is_none());

        for bad in [
            r#"{"id":"g","name":"G","exe":"G.exe","builds":[{"name":"b1","sha256":"xyz"}]}"#,
            r#"{"id":"g","name":"G","exe":"G.exe","builds":[{"name":" ","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}]}"#,
            r#"{"id":"g","name":"G","exe":"G.exe","build_file":"../other/x.dll"}"#,
            r#"{"id":"g","name":"G","exe":"G.exe","detect":["..\\..\\Windows\\x"]}"#,
        ] {
            write(bad);
            assert!(ModManifest::load(root.path()).is_err(), "{bad}");
        }
    }

    #[test]
    fn loads_manifest_with_version_from_source() {
        let root = tempfile::tempdir().unwrap();
        let pkg = root.path().join("m");
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::write(pkg.join("src/ModEntry.cs"), "const string Version = \"1.2.3\";").unwrap();
        std::fs::write(
            pkg.join(MANIFEST_FILE),
            r#"{"id":"m","name":"M","version_from":{"file":"src/ModEntry.cs","after":"Version = \""},
                "game":{"id":"g","name":"G","exe":"G.exe"},
                "install":{"method":"unity-mono","assembly":"bin/M.dll","entry":"M.ModEntry.Init"}}"#,
        )
        .unwrap();
        let m = ModManifest::load(&pkg).unwrap();
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.missing_files(), vec!["bin/M.dll".to_owned()]);
        // и как папка с пакетами, и как сам пакет — один и тот же мод не дублируется
        let (found, errors) = discover(&[root.path().to_path_buf(), pkg.clone()]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "m");
    }
}
