//! Обновления Modly: выпуски на GitHub, подписанный манифест и пакет обновления.
//!
//! У каждого выпуска на GitHub четыре файла: установщик `ModlySetup-<версия>.exe`, архив
//! `Modly-<версия>-windows-x64.zip` (он же пакет обновления и портативная версия), манифест
//! `modly-update.json` и его подпись `modly-update.json.sig` (Ed25519). Modly верит только манифесту,
//! подписанному ключом выпусков, открытая часть которого вшита в программу: из него берутся версия,
//! размер и SHA-256 пакета. Поэтому подменить обновление не выйдет даже тому, кто получит доступ
//! к странице выпусков, — без закрытого ключа подпись не сойдётся. Закрытый ключ лежит только
//! у автора (`cargo xtask keygen`, см. docs/RELEASING.md).

use crate::http::{self, Request};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Репозиторий выпусков на GitHub.
pub const REPO: &str = "melineceo/Modly";
/// Куда можно перейти по перенаправлению при скачивании файлов выпуска.
pub const FOLLOW: &[&str] = &["github.com", ".githubusercontent.com"];
pub const MANIFEST_NAME: &str = "modly-update.json";
pub const SIG_NAME: &str = "modly-update.json.sig";
/// Папка внутри архива выпуска: `Modly/modly.exe`, `Modly/modly-updater.exe`…
pub const PACKAGE_ROOT: &str = "Modly/";
pub const APP_EXE: &str = "modly.exe";
pub const UPDATER_EXE: &str = "modly-updater.exe";

/// Открытые ключи выпусков (Ed25519, hex). Несколько — чтобы ключ можно было сменить, не бросив тех,
/// у кого стоит старая версия: новая подписывается старым ключом, а в ней уже вшит новый.
const PUBLIC_KEYS: &[&str] = &["a997fdc7c06720ec44c22894074472d4e9fb567d730a3c8360b76b2aa812fef7"];

// ------------------------------------------------------------------ версии ---

/// Версия вида `1.2.3` или `1.2.3-beta.2` (порядок — как в SemVer: `0.2.0-beta` < `0.2.0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    nums: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut nums = [0u64; 3];
        let parts: Vec<&str> = core.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            nums[i] = p.parse().ok()?;
        }
        let pre = match pre {
            Some(p) => {
                let ids: Vec<String> = p.split('.').map(str::to_owned).collect();
                if ids.iter().any(|i| i.is_empty() || !i.bytes().all(|b| b.is_ascii_alphanumeric())) {
                    return None;
                }
                ids
            }
            None => Vec::new(),
        };
        Some(Version { nums, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.nums.cmp(&other.nums).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Версия для людей: `0.2.0-beta` → «0.2.0 Beta», `1.0.0-rc.2` → «1.0.0 Rc 2».
pub fn label(version: &str) -> String {
    match version.split_once('-') {
        Some((num, pre)) => {
            let pre: Vec<String> = pre
                .split('.')
                .map(|p| {
                    let mut c = p.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                })
                .collect();
            format!("{num} {}", pre.join(" "))
        }
        None => version.to_owned(),
    }
}

/// Канал обновлений: только выпуски или ещё и бета-версии.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    /// Канал по умолчанию: кто поставил бету, тот получает и следующие беты.
    pub fn default_for(current: &str) -> Self {
        if Version::parse(current).is_some_and(|v| v.is_prerelease()) { Channel::Beta } else { Channel::Stable }
    }
}

// -------------------------------------------------------- манифест и подпись ---

/// Файл выпуска: имя на странице выпуска, размер и SHA-256 (hex).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// `modly-update.json` — что за выпуск и чем его проверить.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Всегда "modly": подпись от другого манифеста сюда не подойдёт.
    pub app: String,
    pub version: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub notes_ru: String,
    #[serde(default)]
    pub notes_en: String,
    /// Архив: из него ставится обновление (и он же портативная версия).
    pub package: Asset,
    /// Установщик — для ссылки «скачать вручную».
    #[serde(default)]
    pub setup: Option<Asset>,
}

impl Manifest {
    /// «Что нового» для окна — простым текстом (см. [`plain_notes`]).
    pub fn notes(&self, lang: &str) -> String {
        plain_notes(if lang == "ru" || self.notes_en.trim().is_empty() { &self.notes_ru } else { &self.notes_en })
    }
}

/// Markdown из CHANGELOG — в простой текст: пункты «•», без `**` и обратных кавычек, перенос
/// строки внутри пункта (строка с отступом) склеивается с предыдущей.
pub fn plain_notes(md: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let clean = t.replace("**", "").replace('`', "");
        if let Some(item) = clean.strip_prefix("- ").or_else(|| clean.strip_prefix("* ")) {
            out.push(format!("• {item}"));
        } else if line.starts_with(' ') && out.last().is_some_and(|l| !l.is_empty()) {
            let last = out.last_mut().unwrap();
            last.push(' ');
            last.push_str(&clean);
        } else {
            out.push(clean);
        }
    }
    out.join("\n").trim().to_owned()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Подпись `sig` (hex, 64 байта) у `data` от одного из ключей `keys` (hex, 32 байта).
fn verify_with(keys: &[&str], data: &[u8], sig: &str) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};
    let Some(sig) = unhex(sig).and_then(|b| <[u8; 64]>::try_from(b).ok()) else {
        return false;
    };
    let sig = Signature::from_bytes(&sig);
    keys.iter().any(|k| {
        unhex(k)
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok())
            .is_some_and(|vk| vk.verify_strict(data, &sig).is_ok())
    })
}

/// Проверить подписанный манифест и разобрать его.
pub fn verify_manifest(data: &[u8], sig: &str) -> Result<Manifest> {
    verify_manifest_with(PUBLIC_KEYS, data, sig)
}

fn verify_manifest_with(keys: &[&str], data: &[u8], sig: &str) -> Result<Manifest> {
    if !verify_with(keys, data, sig) {
        bail!("подпись обновления не сошлась — его не ставим");
    }
    let m: Manifest = serde_json::from_slice(data).context("манифест обновления")?;
    if m.app != "modly" || Version::parse(&m.version).is_none() {
        bail!("манифест обновления не от Modly");
    }
    if !safe_name(&m.package.name) || m.package.size == 0 || unhex(&m.package.sha256).map(|b| b.len()) != Some(32) {
        bail!("в манифесте неверный пакет");
    }
    Ok(m)
}

/// Имя файла без папок и странных символов.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !name.starts_with('.')
}

// ------------------------------------------------------------------ GitHub ---

/// Найденное обновление: подписанный манифест и откуда качать пакет.
#[derive(Clone, Debug)]
pub struct Update {
    pub manifest: Manifest,
    pub package_url: String,
    /// Страница выпуска на GitHub (что нового, ручное скачивание).
    pub page_url: String,
    pub setup_url: Option<String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// Адрес API: `MODLY_UPDATE_API` подменяет GitHub (локальная проверка обновлений). Подпись всё равно
/// проверяется вшитым ключом, так что подмена адреса не даёт поставить чужой пакет.
fn api_base() -> String {
    std::env::var("MODLY_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".to_owned())
}

/// GET с переходом на CDN GitHub. `api` — запрос к API (свой Accept и версия API).
fn get(url: &str, limit: usize, api: bool) -> Result<Vec<u8>> {
    let mut req = Request::get(url);
    req.follow = FOLLOW;
    if api {
        req.headers.push(("Accept", "application/vnd.github+json".to_owned()));
        req.headers.push(("X-GitHub-Api-Version", "2022-11-28".to_owned()));
    } else {
        req.headers.push(("Accept", "application/octet-stream".to_owned()));
    }
    let (status, body) = http::fetch(&req, limit)?;
    match status {
        200 => Ok(body),
        403 | 429 => bail!("GitHub просит подождать с проверками — попробуйте позже"),
        404 => bail!("выпуски Modly не найдены"),
        s => bail!("GitHub ответил {s}"),
    }
}

/// Есть ли версия новее `current` в канале `channel`. `Ok(None)` — у вас последняя.
pub fn check(current: &str, channel: Channel) -> Result<Option<Update>> {
    let current = Version::parse(current).context("своя версия")?;
    let body = get(&format!("{}/repos/{REPO}/releases?per_page=20", api_base()), 2 << 20, true)?;
    let releases: Vec<GhRelease> = serde_json::from_slice(&body).context("ответ GitHub")?;
    let mut candidates: Vec<(Version, GhRelease)> = releases
        .into_iter()
        .filter(|r| !r.draft && (channel == Channel::Beta || !r.prerelease))
        .filter_map(|r| Version::parse(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| *v > current)
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let mut first_err = None;
    for (version, rel) in candidates {
        match release_update(&version, &rel) {
            Ok(u) => return Ok(Some(u)),
            Err(e) => {
                log::warn!("update {}: {e:#}", rel.tag_name);
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

fn release_update(version: &Version, rel: &GhRelease) -> Result<Update> {
    let asset = |name: &str| rel.assets.iter().find(|a| a.name == name);
    let (Some(m), Some(s)) = (asset(MANIFEST_NAME), asset(SIG_NAME)) else {
        bail!("у выпуска нет подписанного манифеста");
    };
    let data = get(&m.browser_download_url, 64 << 10, false)?;
    let sig = get(&s.browser_download_url, 1 << 10, false)?;
    let manifest = verify_manifest(&data, &String::from_utf8_lossy(&sig))?;
    if Version::parse(&manifest.version).as_ref() != Some(version) {
        bail!("версия в манифесте не совпадает с выпуском");
    }
    let Some(pkg) = asset(&manifest.package.name) else {
        bail!("у выпуска нет пакета {}", manifest.package.name);
    };
    if pkg.size != manifest.package.size {
        bail!("размер пакета не совпадает с манифестом");
    }
    let setup_url = manifest.setup.as_ref().and_then(|s| asset(&s.name)).map(|a| a.browser_download_url.clone());
    Ok(Update {
        package_url: pkg.browser_download_url.clone(),
        page_url: rel.html_url.clone(),
        setup_url,
        manifest,
    })
}

// --------------------------------------------------------------- загрузка ---

/// Папка загрузок обновлений: `%LOCALAPPDATA%\Modly\updates` (`MODLY_HOME\updates` в тестах).
pub fn updates_dir() -> PathBuf {
    let dir = match std::env::var_os("MODLY_HOME") {
        Some(home) => PathBuf::from(home).join("updates"),
        None => std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("Modly").join("updates"))
            .unwrap_or_else(|| crate::paths::data_dir().join("updates")),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Скачать пакет в `dir` и проверить размер и SHA-256. `progress(сколько, из скольких)`;
/// ошибка из него — отмена. Возвращает путь к проверенному архиву.
pub fn download(update: &Update, dir: &Path, progress: &mut dyn FnMut(u64, u64) -> Result<()>) -> Result<PathBuf> {
    let pkg = &update.manifest.package;
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(&pkg.name);
    if dest.is_file() && file_sha256(&dest).is_ok_and(|h| h.eq_ignore_ascii_case(&pkg.sha256)) {
        progress(pkg.size, pkg.size)?;
        return Ok(dest);
    }
    let part = dir.join(format!("{}.part", pkg.name));
    let mut file = std::fs::File::create(&part).with_context(|| format!("{}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut got = 0u64;
    let mut req = Request::get(&update.package_url);
    req.follow = FOLLOW;
    req.timeout_ms = 60_000;
    req.headers.push(("Accept", "application/octet-stream".to_owned()));
    let status = http::send(
        &req,
        &mut |status, _| {
            if status != 200 {
                bail!("GitHub ответил {status}");
            }
            Ok(())
        },
        &mut |chunk| {
            got += chunk.len() as u64;
            if got > pkg.size {
                bail!("пакет больше, чем обещает манифест");
            }
            hasher.update(chunk);
            file.write_all(chunk)?;
            progress(got, pkg.size)
        },
    );
    drop(file);
    let ok = status.and_then(|_| {
        if got != pkg.size {
            bail!("пакет скачался не полностью");
        }
        if !hex(&hasher.finalize()).eq_ignore_ascii_case(&pkg.sha256) {
            bail!("контрольная сумма пакета не сошлась");
        }
        Ok(())
    });
    if let Err(e) = ok {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

/// Распаковать архив выпуска в `dest` без верхней папки `Modly/`. Возвращает распакованные файлы
/// (пути относительно `dest`).
pub fn extract_package(zip_path: &Path, dest: &Path) -> Result<Vec<PathBuf>> {
    let file = std::fs::File::open(zip_path).with_context(|| format!("{}", zip_path.display()))?;
    extract_package_from(file, dest)
}

pub fn extract_package_from<R: std::io::Read + std::io::Seek>(reader: R, dest: &Path) -> Result<Vec<PathBuf>> {
    let mut zip = zip::ZipArchive::new(reader).context("архив выпуска")?;
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            bail!("в архиве опасный путь");
        };
        let Ok(rel) = name.strip_prefix(PACKAGE_ROOT.trim_end_matches('/')) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&out).with_context(|| format!("{}", out.display()))?;
        std::io::copy(&mut entry, &mut f)?;
        files.push(rel.to_path_buf());
    }
    if !files.iter().any(|f| f.as_os_str().eq_ignore_ascii_case(APP_EXE)) {
        bail!("в архиве нет {APP_EXE}");
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn versions_order_like_semver() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.1.0-beta") < v("0.2.0-beta"));
        assert!(v("0.2.0-beta") < v("0.2.0"));
        assert!(v("0.2.0-beta") < v("0.2.0-beta.2"));
        assert!(v("0.2.0-beta.2") < v("0.2.0-beta.10"));
        assert!(v("0.2.0-alpha") < v("0.2.0-beta"));
        assert!(v("v1.0") == v("1.0.0"));
        assert!(v("1.10.0") > v("1.9.9"));
        for bad in ["", "x", "1..2", "1.2.3.4", "1.2.3-", "1.2.3-be ta", "1.2.3-b..1"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(label("0.2.0-beta"), "0.2.0 Beta");
        assert_eq!(label("1.0.0-rc.2"), "1.0.0 Rc 2");
        assert_eq!(label("1.0.0"), "1.0.0");
        assert_eq!(Channel::default_for("0.1.0-beta"), Channel::Beta);
        assert_eq!(Channel::default_for("1.0.0"), Channel::Stable);
    }

    #[test]
    fn notes_become_plain_text() {
        let md = "- **Установщик** `ModlySetup.exe`: ставит\n  без прав.\n- Второй пункт.\n\nПросто текст.";
        assert_eq!(plain_notes(md), "• Установщик ModlySetup.exe: ставит без прав.\n• Второй пункт.\n\nПросто текст.");
    }

    fn signed(key: &SigningKey, m: &Manifest) -> (Vec<u8>, String) {
        let data = serde_json::to_vec_pretty(m).unwrap();
        let sig = hex(&key.sign(&data).to_bytes());
        (data, sig)
    }

    #[test]
    fn manifest_needs_a_valid_signature() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = hex(key.verifying_key().as_bytes());
        let keys = [public.as_str()];
        let m = Manifest {
            app: "modly".into(),
            version: "0.2.0-beta".into(),
            date: "2026-10-23".into(),
            notes_ru: "Новое".into(),
            notes_en: "New".into(),
            package: Asset { name: "Modly-0.2.0-beta-windows-x64.zip".into(), size: 10, sha256: "ab".repeat(32) },
            setup: None,
        };
        let (data, sig) = signed(&key, &m);
        assert_eq!(verify_manifest_with(&keys, &data, &sig).unwrap(), m);

        // байт поменялся — подпись не подходит
        let mut bad = data.clone();
        bad[10] ^= 1;
        assert!(verify_manifest_with(&keys, &bad, &sig).is_err());
        // чужой ключ
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let (_, other_sig) = signed(&other, &m);
        assert!(verify_manifest_with(&keys, &data, &other_sig).is_err());
        assert!(verify_manifest_with(&keys, &data, "zz").is_err());
        // подписано верно, но это не Modly или путь к файлу
        let (d, s) = signed(&key, &Manifest { app: "other".into(), ..m.clone() });
        assert!(verify_manifest_with(&keys, &d, &s).is_err());
        let mut evil = m.clone();
        evil.package.name = "..\\..\\evil.exe".into();
        let (d, s) = signed(&key, &evil);
        assert!(verify_manifest_with(&keys, &d, &s).is_err());
    }

    #[test]
    fn package_extracts_without_root_folder() {
        use zip::write::SimpleFileOptions;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = SimpleFileOptions::default();
            for (name, body) in [("Modly/modly.exe", "exe"), ("Modly/modly-updater.exe", "upd"), ("Modly/LICENSE.txt", "mit"), ("other.txt", "x")] {
                w.start_file(name, o).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        buf.set_position(0);
        let files = extract_package_from(buf, dir.path()).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(std::fs::read_to_string(dir.path().join("modly.exe")).unwrap(), "exe");
        assert!(!dir.path().join("other.txt").exists());
    }
}
