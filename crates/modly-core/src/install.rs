//! Установка и удаление модов в папку игры.
//!
//! Состояние определяется по самим файлам игры, а не по записям Modly: мод, поставленный
//! своим установщиком (например, `Установить (Install).bat` у Creatly), Modly тоже видит.

use crate::library::Game;
use crate::manifest::{FileSpec, InstallSpec, LegacyAssembly, ModManifest, expand, safe_relative};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModState {
    NotInstalled,
    /// Установлен. `current` — совпадает с файлами пакета.
    Installed { current: bool },
    /// Установлен частично (например, DLL есть, а в списках Unity её нет).
    Broken(String),
}

/// Тип пользовательской сборки в `ScriptingAssemblies.json`.
const UNITY_USER_ASSEMBLY: u64 = 16;

pub fn status(game: &Game, m: &ModManifest) -> ModState {
    match &m.install {
        InstallSpec::UnityMono { assembly, files, .. } => {
            let u = Unity::new(game);
            let name = file_name(assembly);
            let dll = u.managed.join(&name);
            let registered = u.is_registered(&name).unwrap_or(false);
            match (dll.is_file(), registered) {
                (false, false) => ModState::NotInstalled,
                (true, true) => {
                    let current = same_file(&m.dir.join(assembly), &dll)
                        && files.iter().all(|f| same_file(&m.dir.join(&f.from), &game.dir.join(expand(&f.to, &game.exe))));
                    ModState::Installed { current }
                }
                (true, false) => ModState::Broken("DLL на месте, но не прописана в ScriptingAssemblies.json".into()),
                (false, true) => ModState::Broken("сборка прописана, но DLL нет".into()),
            }
        }
        InstallSpec::Files { files, .. } => {
            let present: Vec<bool> = files.iter().map(|f| game.dir.join(expand(&f.to, &game.exe)).is_file()).collect();
            if present.iter().all(|p| !p) {
                ModState::NotInstalled
            } else if present.iter().all(|p| *p) {
                let current = files.iter().all(|f| same_file(&m.dir.join(&f.from), &game.dir.join(expand(&f.to, &game.exe))));
                ModState::Installed { current }
            } else {
                ModState::Broken("часть файлов отсутствует".into())
            }
        }
    }
}

pub fn install(game: &Game, m: &ModManifest) -> Result<()> {
    let missing = m.missing_files();
    if !missing.is_empty() {
        bail!("в пакете нет файлов: {} — мод не собран?", missing.join(", "));
    }
    match &m.install {
        InstallSpec::UnityMono { assembly, entry, load, files, legacy, .. } => {
            let u = Unity::new(game);
            if !u.assemblies.is_file() || !u.inits.is_file() {
                bail!("это не Unity-игра на Mono: нет {}", u.assemblies.display());
            }
            u.backup()?;
            for old in legacy {
                u.remove_legacy(old)?;
            }
            let name = file_name(assembly);
            copy(&m.dir.join(assembly), &u.managed.join(&name))?;
            u.register(&name, entry, load.unity_value())?;
            copy_files(game, m, files)?;
        }
        InstallSpec::Files { files, .. } => copy_files(game, m, files)?,
    }
    log::info!("installed {} {} into {}", m.id, m.version, game.dir.display());
    Ok(())
}

pub fn uninstall(game: &Game, m: &ModManifest) -> Result<()> {
    match &m.install {
        InstallSpec::UnityMono { assembly, entry, files, cleanup, legacy, .. } => {
            let u = Unity::new(game);
            let name = file_name(assembly);
            u.unregister(&name, entry)?;
            for old in legacy {
                u.remove_legacy(old)?;
            }
            remove_file(&u.managed.join(&name))?;
            remove_files(game, files.iter().map(|f| f.to.as_str()))?;
            remove_files(game, cleanup.iter().map(String::as_str))?;
        }
        InstallSpec::Files { files, cleanup } => {
            remove_files(game, files.iter().map(|f| f.to.as_str()))?;
            remove_files(game, cleanup.iter().map(String::as_str))?;
        }
    }
    log::info!("uninstalled {} from {}", m.id, game.dir.display());
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Unity без загрузчика

struct Unity {
    managed: PathBuf,
    assemblies: PathBuf,
    inits: PathBuf,
}

impl Unity {
    fn new(game: &Game) -> Self {
        let data = game.dir.join(expand("{data}", &game.exe));
        Unity {
            managed: data.join("Managed"),
            assemblies: data.join("ScriptingAssemblies.json"),
            inits: data.join("RuntimeInitializeOnLoads.json"),
        }
    }

    /// Оригиналы списков Unity сохраняются один раз, как `*.orig` (так же делает установщик Creatly).
    fn backup(&self) -> Result<()> {
        for f in [&self.assemblies, &self.inits] {
            let orig = PathBuf::from(format!("{}.orig", f.display()));
            if !orig.exists() {
                std::fs::copy(f, &orig).with_context(|| format!("backup {}", f.display()))?;
            }
        }
        Ok(())
    }

    fn is_registered(&self, dll_name: &str) -> Result<bool> {
        let doc = read_json(&self.assemblies)?;
        Ok(names(&doc).iter().any(|n| n.as_str() == Some(dll_name)))
    }

    fn register(&self, dll_name: &str, entry: &str, load: u32) -> Result<()> {
        let mut doc = read_json(&self.assemblies)?;
        let obj = doc.as_object_mut().context("ScriptingAssemblies.json: не объект")?;
        let present = obj.get("names").and_then(Value::as_array).is_some_and(|a| a.iter().any(|n| n.as_str() == Some(dll_name)));
        if !present {
            let len_names = array_mut(obj, "names")?.len();
            let len_types = array_mut(obj, "types")?.len();
            if len_names != len_types {
                bail!("ScriptingAssemblies.json: names и types разной длины");
            }
            array_mut(obj, "names")?.push(json!(dll_name));
            array_mut(obj, "types")?.push(json!(UNITY_USER_ASSEMBLY));
            write_json(&self.assemblies, &doc)?;
        }

        let (asm, ns, class, method) = split_entry(dll_name, entry)?;
        let mut doc = read_json(&self.inits)?;
        let obj = doc.as_object_mut().context("RuntimeInitializeOnLoads.json: не объект")?;
        let root = array_mut(obj, "root")?;
        let exists = root.iter().any(|e| e.get("assemblyName").and_then(Value::as_str) == Some(asm.as_str()));
        if !exists {
            root.push(json!({
                "assemblyName": asm,
                "nameSpace": ns,
                "className": class,
                "methodName": method,
                "loadTypes": load,
                "isUnityClass": false
            }));
            write_json(&self.inits, &doc)?;
        }
        Ok(())
    }

    fn unregister(&self, dll_name: &str, entry: &str) -> Result<()> {
        let mut doc = read_json(&self.assemblies)?;
        let obj = doc.as_object_mut().context("ScriptingAssemblies.json: не объект")?;
        let index = obj
            .get("names")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().position(|n| n.as_str() == Some(dll_name)));
        if let Some(i) = index {
            array_mut(obj, "names")?.remove(i);
            let types = array_mut(obj, "types")?;
            if i < types.len() {
                types.remove(i);
            }
            write_json(&self.assemblies, &doc)?;
        }

        let (asm, ..) = split_entry(dll_name, entry)?;
        let mut doc = read_json(&self.inits)?;
        let obj = doc.as_object_mut().context("RuntimeInitializeOnLoads.json: не объект")?;
        let root = array_mut(obj, "root")?;
        let before = root.len();
        root.retain(|e| e.get("assemblyName").and_then(Value::as_str) != Some(asm.as_str()));
        if root.len() != before {
            write_json(&self.inits, &doc)?;
        }
        Ok(())
    }

    fn remove_legacy(&self, old: &LegacyAssembly) -> Result<()> {
        self.unregister(&old.assembly, &old.entry)?;
        remove_file(&self.managed.join(&old.assembly))
    }
}

fn names(doc: &Value) -> Vec<Value> {
    doc.get("names").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn array_mut<'a>(obj: &'a mut Map<String, Value>, key: &str) -> Result<&'a mut Vec<Value>> {
    obj.entry(key.to_owned())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("поле {key} — не массив"))
}

/// `Creatly.ModEntry.Init` → (сборка `Creatly`, пространство `Creatly`, класс `ModEntry`, метод `Init`).
fn split_entry(dll_name: &str, entry: &str) -> Result<(String, String, String, String)> {
    let asm = dll_name.strip_suffix(".dll").unwrap_or(dll_name).to_owned();
    let parts: Vec<&str> = entry.rsplitn(3, '.').collect();
    match parts.as_slice() {
        [method, class, ns] => Ok((asm, (*ns).to_owned(), (*class).to_owned(), (*method).to_owned())),
        [method, class] => Ok((asm, String::new(), (*class).to_owned(), (*method).to_owned())),
        _ => bail!("entry должен быть вида Пространство.Класс.Метод, а не {entry:?}"),
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&data);
    serde_json::from_slice(data).with_context(|| format!("parse {}", path.display()))
}

/// Компактно и без BOM — ровно как пишет Unity; так установщики модов на PowerShell
/// (они ищут подстроки вроде `,"Creatly.dll"],"types":[`) продолжают работать с этими файлами.
fn write_json(path: &Path, doc: &Value) -> Result<()> {
    let text = serde_json::to_string(doc)?;
    crate::write_atomic(path, text.as_bytes()).with_context(|| format!("write {}", path.display()))
}

// ---------------------------------------------------------------------------------------------

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_owned())
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Сначала во временный файл рядом: если игра держит старый файл, не останемся с половиной DLL.
    let tmp = to.with_extension("modly-new");
    std::fs::copy(from, &tmp).with_context(|| format!("copy {} → {}", from.display(), tmp.display()))?;
    std::fs::rename(&tmp, to).with_context(|| format!("replace {}", to.display()))?;
    Ok(())
}

fn copy_files(game: &Game, m: &ModManifest, files: &[FileSpec]) -> Result<()> {
    for f in files {
        // ModManifest::load это уже проверил; здесь — вторая преграда на случай пакета, собранного в обход.
        if !safe_relative(&f.from) || !safe_relative(&f.to) {
            bail!("путь ведёт за пределы пакета или игры: {} → {}", f.from, f.to);
        }
        copy(&m.dir.join(&f.from), &game.dir.join(expand(&f.to, &game.exe)))?;
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("delete {}", path.display())),
    }
}

/// Удалить файлы внутри папки игры. В имени файла (не в папках) можно `*`: `{data}/Sessions/*.ftmod`.
fn remove_files<'a>(game: &Game, paths: impl Iterator<Item = &'a str>) -> Result<()> {
    for p in paths {
        let full = game.dir.join(expand(p, &game.exe));
        if !full.starts_with(&game.dir) || p.contains("..") {
            continue;
        }
        let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.contains('*') {
            let Some(dir) = full.parent() else { continue };
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for entry in rd.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_file() && wildcard(&name.to_lowercase(), &file.to_lowercase()) {
                    remove_file(&entry.path())?;
                }
            }
        } else if full.is_file() {
            remove_file(&full)?;
        }
    }
    Ok(())
}

/// Простая маска: `*` — любые символы.
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(pos) => rest = &rest[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) if ma.len() == mb.len() => std::fs::read(a).ok() == std::fs::read(b).ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{GameDef, LoadTime};

    fn fake_game(dir: &Path) -> Game {
        let data = dir.join("FamilyTime_Data");
        std::fs::create_dir_all(data.join("Managed")).unwrap();
        std::fs::write(dir.join("FamilyTime.exe"), b"MZ").unwrap();
        std::fs::write(data.join("Managed/Assembly-CSharp.dll"), b"x").unwrap();
        std::fs::write(
            data.join("ScriptingAssemblies.json"),
            r#"{"names":["UnityEngine.dll","Assembly-CSharp.dll","FamilyTimeMod.dll"],"types":[2,16,16]}"#,
        )
        .unwrap();
        std::fs::write(
            data.join("RuntimeInitializeOnLoads.json"),
            r#"{"root":[{"assemblyName":"Assembly-CSharp","nameSpace":"","className":"A","methodName":"B","loadTypes":4,"isUnityClass":false},{"assemblyName":"FamilyTimeMod","nameSpace":"FamilyTimeMod","className":"ModEntry","methodName":"Init","loadTypes":0,"isUnityClass":false}]}"#,
        )
        .unwrap();
        std::fs::write(data.join("Managed/FamilyTimeMod.dll"), b"old").unwrap();
        Game::new(dir, "FamilyTime.exe", "Family Time", None, false)
    }

    fn manifest(dir: &Path) -> ModManifest {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/Creatly.dll"), b"new mod").unwrap();
        ModManifest {
            id: "creatly".into(),
            name: "Creatly".into(),
            version: "1.0.1".into(),
            version_from: None,
            author: String::new(),
            adult: false,
            description: None,
            links: Default::default(),
            game: GameDef { id: "ft".into(), name: "FT".into(), exe: "FamilyTime.exe".into(), ..Default::default() },
            install: InstallSpec::UnityMono {
                assembly: "bin/Creatly.dll".into(),
                entry: "Creatly.ModEntry.Init".into(),
                load: LoadTime::AfterScene,
                files: vec![],
                cleanup: vec!["{data}/Creatly.cfg".into()],
                legacy: vec![LegacyAssembly { assembly: "FamilyTimeMod.dll".into(), entry: "FamilyTimeMod.ModEntry.Init".into() }],
            },
            dev: None,
            maps: Vec::new(),
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn unity_install_matches_powershell_installer_and_uninstalls_cleanly() {
        let g = tempfile::tempdir().unwrap();
        let p = tempfile::tempdir().unwrap();
        let game = fake_game(g.path());
        let m = manifest(p.path());
        let data = g.path().join("FamilyTime_Data");

        assert_eq!(status(&game, &m), ModState::NotInstalled);
        install(&game, &m).unwrap();
        assert_eq!(status(&game, &m), ModState::Installed { current: true });

        let asm = std::fs::read_to_string(data.join("ScriptingAssemblies.json")).unwrap();
        // старая версия убрана, новая — в конце, в том же компактном виде, что ищут установщики на PowerShell
        assert_eq!(asm, r#"{"names":["UnityEngine.dll","Assembly-CSharp.dll","Creatly.dll"],"types":[2,16,16]}"#);
        assert!(asm.contains(r#","Creatly.dll"],"types":["#));
        let inits = std::fs::read_to_string(data.join("RuntimeInitializeOnLoads.json")).unwrap();
        assert!(inits.ends_with(r#"{"assemblyName":"Creatly","nameSpace":"Creatly","className":"ModEntry","methodName":"Init","loadTypes":0,"isUnityClass":false}]}"#));
        assert!(!inits.contains("FamilyTimeMod"));
        assert!(!data.join("Managed/FamilyTimeMod.dll").exists());
        assert!(data.join("ScriptingAssemblies.json.orig").exists());

        // повторная установка ничего не дублирует
        install(&game, &m).unwrap();
        assert_eq!(std::fs::read_to_string(data.join("ScriptingAssemblies.json")).unwrap(), asm);

        // пакет пересобран — установка устарела
        std::fs::write(p.path().join("bin/Creatly.dll"), b"newer mod").unwrap();
        assert_eq!(status(&game, &m), ModState::Installed { current: false });

        std::fs::write(data.join("Creatly.cfg"), "x=1").unwrap();
        uninstall(&game, &m).unwrap();
        assert_eq!(status(&game, &m), ModState::NotInstalled);
        assert_eq!(
            std::fs::read_to_string(data.join("ScriptingAssemblies.json")).unwrap(),
            r#"{"names":["UnityEngine.dll","Assembly-CSharp.dll"],"types":[2,16]}"#
        );
        assert!(!data.join("Creatly.cfg").exists());
        assert!(!std::fs::read_to_string(data.join("RuntimeInitializeOnLoads.json")).unwrap().contains("Creatly"));
    }

    #[test]
    fn refuses_to_install_unbuilt_package() {
        let g = tempfile::tempdir().unwrap();
        let p = tempfile::tempdir().unwrap();
        let game = fake_game(g.path());
        let m = manifest(p.path());
        std::fs::remove_file(p.path().join("bin/Creatly.dll")).unwrap();
        let err = install(&game, &m).unwrap_err().to_string();
        assert!(err.contains("bin/Creatly.dll"), "{err}");
    }

    #[test]
    fn wildcard_matches() {
        assert!(wildcard("*.ftmod", "my world.family.ftmod"));
        assert!(!wildcard("*.ftmod", "my world.family"));
        assert!(wildcard("creatly*.dll", "creatly.dll"));
        assert!(wildcard("a*b*c", "a-x-b-y-c"));
        assert!(!wildcard("a*b*c", "a-x-c"));
    }

    #[test]
    fn entry_split() {
        assert_eq!(
            split_entry("Creatly.dll", "Creatly.ModEntry.Init").unwrap(),
            ("Creatly".into(), "Creatly".into(), "ModEntry".into(), "Init".into())
        );
        assert_eq!(
            split_entry("A.dll", "My.Deep.Ns.Entry.Run").unwrap(),
            ("A".into(), "My.Deep.Ns".into(), "Entry".into(), "Run".into())
        );
        assert_eq!(split_entry("A.dll", "Entry.Run").unwrap().1, "");
    }
}
