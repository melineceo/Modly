//! Сайт модов (mods.hackerstone.xyz): каталог, установка в один клик и необязательный вход.
//!
//! Всё, что приходит с сайта, Modly перепроверяет сам:
//! - архив скачивается только с этого сайта и только по номеру файла — адрес из ответа не
//!   используется; размер и SHA-256 сверяются с тем, что одобрил модератор;
//! - распаковка не выпускает файлы за папку пакета (`..`, диски, ссылки), а `modly.json` проходит
//!   те же проверки путей, что и у локальных пакетов ([`crate::manifest::safe_relative`]);
//! - логотипы и обложки — тоже только с этого сайта, не больше 2 МБ, и только если первые байты
//!   файла — PNG, JPEG, WebP или SVG (адрес и заголовки ответа не в счёт);
//! - вход — через браузер на hackerstone.xyz (пароль в Modly не попадает), одноразовый код
//!   приходит на 127.0.0.1 и без проверочной строки PKCE бесполезен; токен хранится
//!   зашифрованным ключом пользователя Windows (DPAPI).

use crate::http::{self, Request};
use crate::manifest::{ModManifest, safe_relative};
use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_SITE: &str = "https://mods.hackerstone.xyz";
const JSON_LIMIT: usize = 4 * 1024 * 1024;
const PACKAGE_LIMIT: u64 = 110 * 1024 * 1024;
const UNPACK_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const THUMB_LIMIT: usize = 2 * 1024 * 1024;
/// Сколько картинка каталога живёт в кэше, прежде чем Modly снова спросит её у сайта.
const THUMB_FRESH: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Кэш картинок не больше стольких файлов и байт — лишние, самые старые, удаляются.
const THUMB_CACHE_FILES: usize = 400;
const THUMB_CACHE_BYTES: u64 = 64 * 1024 * 1024;
/// Метка в папке пакета: мод поставлен из каталога (откуда, какая версия).
pub const MARKER: &str = ".modly-site.json";

/// Ошибки, на которые интерфейс реагирует по-особому.
#[derive(Debug)]
pub enum SiteError {
    /// Токен недействителен (вышли на сайте, отозвали) — войти заново.
    NeedLogin,
    /// Человек отменил вход.
    Cancelled,
}

impl std::fmt::Display for SiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SiteError::NeedLogin => "нужно войти заново",
            SiteError::Cancelled => "вход отменён",
        })
    }
}

impl std::error::Error for SiteError {}

/// Адрес сайта. `MODLY_MODS_URL` подменяет его (проверка локальной копии сайта: http://127.0.0.1:…).
pub fn site_url() -> String {
    if let Ok(u) = std::env::var("MODLY_MODS_URL") {
        let u = u.trim().trim_end_matches('/').to_owned();
        if http::parse_url(&u).is_ok_and(|p| p.3 == "/") {
            return u;
        }
    }
    DEFAULT_SITE.to_owned()
}

/// Имя сайта для подписей: `mods.hackerstone.xyz`.
pub fn site_host() -> String {
    http::parse_url(&site_url()).map(|p| p.1).unwrap_or_default()
}

pub fn slug_ok(slug: &str) -> bool {
    !slug.is_empty() && slug.len() <= 48 && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// ============================================================================================
// Данные сайта

/// Текст на двух языках.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Tr {
    #[serde(default)]
    pub ru: String,
    #[serde(default)]
    pub en: String,
}

impl Tr {
    pub fn get(&self, lang: &str) -> &str {
        if (lang == "en" && !self.en.is_empty()) || self.ru.is_empty() { &self.en } else { &self.ru }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Latest {
    pub version: String,
    #[serde(default)]
    pub modly: bool,
    /// У выпуска есть файлы — их можно скачать прямо в Modly, даже без пакета.
    #[serde(default)]
    pub download: bool,
}

/// Мод в каталоге.
#[derive(Clone, Debug, Deserialize)]
pub struct SiteMod {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub tagline: Tr,
    #[serde(default)]
    pub accent: String,
    /// Логотип и обложка — уменьшенные копии на сайте (полные адреса). Пусто — картинки нет.
    #[serde(default)]
    pub logo: String,
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub downloads: i64,
    #[serde(default)]
    pub likes: i64,
    #[serde(default)]
    pub comments: i64,
    #[serde(default)]
    pub featured: bool,
    #[serde(default)]
    pub liked: bool,
    #[serde(default)]
    pub latest: Option<Latest>,
    /// Какая игра: из пакета Modly последнего выпуска и со страницы мода (номер в Steam).
    #[serde(default)]
    pub game_info: GameInfo,
}

/// Игра мода по данным сайта — чтобы каталог знал, есть ли она у игрока.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GameInfo {
    /// `game.id` из `modly.json` (пусто — пакета нет).
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub exe: String,
    #[serde(default)]
    pub steam_appid: Option<u32>,
    /// Сборки игры, на которых автор проверил мод.
    #[serde(default)]
    pub builds: Vec<String>,
}

/// Название игры для сравнения: только буквы и цифры, без регистра (`Family Time Ep.3` → `familytimeep3`).
pub fn game_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Debug, Deserialize)]
pub struct SiteFile {
    pub id: i64,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub modly: bool,
    #[serde(default)]
    pub modly_root: String,
    /// Адрес файла. Используется только у модов со статической страницей (у них нет номера), см. `file_url`.
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub id: i64,
    pub version: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub notes: Tr,
    #[serde(default)]
    pub files: Vec<SiteFile>,
}

/// Мод со страницы: то же, что в каталоге, плюс выпуски.
#[derive(Clone, Debug, Deserialize)]
pub struct ModDetails {
    #[serde(flatten)]
    pub brief: SiteMod,
    #[serde(default)]
    pub releases: Vec<Release>,
}

impl ModDetails {
    /// Свежий одобренный выпуск с пакетом для Modly (ZIP с `modly.json`).
    pub fn package(&self) -> Option<(&Release, &SiteFile)> {
        self.releases
            .iter()
            .filter(|r| r.status == "approved")
            .find_map(|r| r.files.iter().find(|f| f.modly && f.kind == "zip").map(|f| (r, f)))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AccountUser {
    #[serde(default)]
    pub id: i64,
    pub nick: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub checkmark: bool,
}

#[derive(Clone, Debug)]
pub struct Account {
    pub token: String,
    pub user: AccountUser,
}

// ============================================================================================
// API

pub(crate) fn api<T: DeserializeOwned>(path: &str, token: Option<&str>, body: Option<&Value>) -> Result<T> {
    let url = format!("{}/api/v1/{}", site_url(), path);
    let payload = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
    let req = match &payload {
        Some(p) => Request::post_json(&url, p),
        None => Request::get(&url),
    }
    .bearer(token);
    let (status, data) = http::fetch(&req, JSON_LIMIT)?;
    let v: Value = serde_json::from_slice(&data).map_err(|_| anyhow!("сайт ответил непонятно (код {status})"))?;
    if status == 401 {
        return Err(SiteError::NeedLogin.into());
    }
    if !(200..300).contains(&status) || v.get("ok") == Some(&Value::Bool(false)) {
        let msg = v.get("error").and_then(Value::as_str).unwrap_or("ошибка сайта");
        bail!("{msg}");
    }
    serde_json::from_value(v).context("ответ сайта")
}

#[derive(Deserialize)]
struct CatalogResp {
    mods: Vec<SiteMod>,
}

/// Весь каталог (одобренные моды). С токеном — ещё и «мои лайки».
pub fn catalog(token: Option<&str>) -> Result<Vec<SiteMod>> {
    let mut all = Vec::new();
    for page in 0..10 {
        let r: CatalogResp = api(&format!("mods?page={page}"), token, None)?;
        let n = r.mods.len();
        all.extend(r.mods.into_iter().filter(|m| slug_ok(&m.slug)));
        if n < 40 {
            break;
        }
    }
    Ok(all)
}

#[derive(Deserialize)]
struct DetailsResp {
    #[serde(rename = "mod")]
    m: ModDetails,
}

pub fn details(slug: &str, token: Option<&str>) -> Result<ModDetails> {
    if !slug_ok(slug) {
        bail!("неверный адрес мода");
    }
    Ok(api::<DetailsResp>(&format!("mods/{slug}"), token, None)?.m)
}

#[derive(Deserialize)]
struct LikeResp {
    liked: bool,
    likes: i64,
}

/// Поставить или снять лайк. Возвращает (лайк стоит?, всего лайков).
pub fn like(slug: &str, token: &str) -> Result<(bool, i64)> {
    if !slug_ok(slug) {
        bail!("неверный адрес мода");
    }
    let r: LikeResp = api(&format!("mods/{slug}/like"), Some(token), Some(&json!({})))?;
    Ok((r.liked, r.likes))
}

#[derive(Deserialize)]
struct MeResp {
    user: AccountUser,
}

/// Проверить вход: кто это (ошибка [`SiteError::NeedLogin`] — токен больше не действует).
pub fn me(token: &str) -> Result<AccountUser> {
    Ok(api::<MeResp>("me", Some(token), None)?.user)
}

// ============================================================================================
// Картинки каталога: логотипы и обложки

/// Формат картинки — по её первым байтам.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Svg,
}

/// Что за картинка в `data`. SVG — только обычным текстом: сжатый svgz не принимается.
pub fn image_format(data: &[u8]) -> Option<ImageFormat> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(ImageFormat::Jpeg);
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    let text = std::str::from_utf8(data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)).ok()?;
    svg_root(text).then_some(ImageFormat::Svg)
}

/// Корневой элемент документа — `<svg>` (после `<?xml …?>`, комментариев и `<!DOCTYPE>`).
fn svg_root(mut s: &str) -> bool {
    loop {
        s = s.trim_start();
        let end = if s.starts_with("<?") {
            "?>"
        } else if s.starts_with("<!--") {
            "-->"
        } else if s.starts_with("<!") {
            ">"
        } else {
            break;
        };
        match s.find(end) {
            Some(i) => s = &s[i + end.len()..],
            None => return false,
        }
    }
    s.starts_with("<svg") && s[4..].starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
}

fn thumb_dir() -> PathBuf {
    crate::paths::data_dir().join("cache").join("catalog")
}

/// Логотип или обложка мода (`SiteMod::logo` / `cover`): из кэша на диске, а если там нет или
/// копия старше недели — с сайта. Только с адреса этого сайта, без перенаправлений, не больше 2 МБ.
/// Блокирует — вызывать не в потоке интерфейса.
pub fn thumb(url: &str) -> Result<Vec<u8>> {
    thumb_cached(&thumb_dir(), &site_url(), url)
}

fn thumb_cached(dir: &Path, site: &str, url: &str) -> Result<Vec<u8>> {
    if url.len() > 1024 || !url.starts_with(&format!("{site}/")) || http::parse_url(url).is_err() {
        bail!("картинка не с сайта модов");
    }
    let file = dir.join(format!("{}.img", &hex(&Sha256::digest(url.as_bytes()))[..32]));
    let cached = match std::fs::read(&file) {
        Ok(data) if data.len() <= THUMB_LIMIT && image_format(&data).is_some() => {
            let age = file.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            if age.is_some_and(|a| a < THUMB_FRESH) {
                return Ok(data);
            }
            Some(data)
        }
        _ => None,
    };
    match fetch_thumb(url) {
        Ok(data) => {
            if let Err(e) = crate::write_atomic(&file, &data) {
                log::warn!("thumb cache {}: {e}", file.display());
            }
            prune_thumbs(dir);
            Ok(data)
        }
        // Сайт недоступен — старая копия лучше, чем ничего.
        Err(e) => cached.ok_or(e),
    }
}

fn fetch_thumb(url: &str) -> Result<Vec<u8>> {
    let (status, data) = http::fetch(&Request::get(url), THUMB_LIMIT)?;
    if status != 200 {
        bail!("сайт не отдал картинку (код {status})");
    }
    if image_format(&data).is_none() {
        bail!("сайт прислал не картинку");
    }
    Ok(data)
}

/// Удалить самые старые картинки, если кэш разросся.
fn prune_thumbs(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "img"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if files.len() <= THUMB_CACHE_FILES && total <= THUMB_CACHE_BYTES {
        return;
    }
    files.sort();
    let mut left = files.len();
    for (_, len, path) in files {
        if left <= THUMB_CACHE_FILES && total <= THUMB_CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            left -= 1;
            total -= len;
        }
    }
}

// ============================================================================================
// Установка пакета из каталога

/// Пакет, поставленный из каталога.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SitePackage {
    pub slug: String,
    pub version: String,
    #[serde(default)]
    pub file_id: i64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub installed_at: u64,
    /// id мода из `modly.json` (папка пакета).
    #[serde(skip_deserializing, default)]
    pub id: String,
}

/// Пакеты из каталога в папке модов: адрес на сайте → пакет.
pub fn site_packages(mods_dir: &Path) -> HashMap<String, SitePackage> {
    let mut out = HashMap::new();
    let Ok(rd) = std::fs::read_dir(mods_dir) else { return out };
    for e in rd.flatten() {
        let dir = e.path();
        let Ok(data) = std::fs::read(dir.join(MARKER)) else { continue };
        let Ok(mut p) = serde_json::from_slice::<SitePackage>(&data) else { continue };
        if slug_ok(&p.slug) {
            p.id = e.file_name().to_string_lossy().into_owned();
            out.insert(p.slug.clone(), p);
        }
    }
    out
}

/// Папка, которая удаляется, если установка не дошла до конца.
struct TempDir(Option<PathBuf>);

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// Скачать и распаковать пакет мода `slug` в `mods_dir`. `progress(0…1)`.
/// Возвращает id мода (имя папки пакета) и версию.
pub fn install_package(slug: &str, mods_dir: &Path, progress: &mut dyn FnMut(f32)) -> Result<SitePackage> {
    let d = details(slug, None)?;
    let (release, file) = d.package().ok_or_else(|| anyhow!("у этого мода нет пакета для Modly — скачайте его на сайте"))?;
    if file.size == 0 || file.size > PACKAGE_LIMIT {
        bail!("пакет слишком большой");
    }
    if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("у пакета нет контрольной суммы");
    }
    std::fs::create_dir_all(mods_dir)?;
    let downloads = mods_dir.join(".downloads");
    std::fs::create_dir_all(&downloads)?;
    let zip_path = downloads.join(format!("{slug}-{}.zip", file.id));
    let result = download(file, &zip_path, progress, 0.9).and_then(|()| {
        progress(0.92);
        unpack(&zip_path, &file.modly_root, mods_dir)
    });
    let _ = std::fs::remove_file(&zip_path);
    let (id, dir) = result?;
    let mut pkg = SitePackage {
        slug: slug.to_owned(),
        version: release.version.clone(),
        file_id: file.id,
        sha256: file.sha256.to_lowercase(),
        installed_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        id: String::new(),
    };
    crate::write_atomic(&dir.join(MARKER), &serde_json::to_vec_pretty(&pkg)?)?;
    pkg.id = id;
    progress(1.0);
    log::info!("site package {} {} → {}", slug, pkg.version, dir.display());
    Ok(pkg)
}

/// Откуда качать файл выпуска. Обычный выпуск — только `download.php` этого сайта по номеру
/// (адрес из ответа не используется); у модов со своей статической страницей (Creatly) номера нет —
/// тогда допускается только их папка `releases/` на этом же сайте.
fn file_url(file: &SiteFile) -> Result<String> {
    let base = site_url();
    if file.id > 0 {
        return Ok(format!("{base}/download.php?f={}", file.id));
    }
    let path = file.url.strip_prefix(&base).unwrap_or("");
    let mut parts = path.split('/');
    let ok = parts.next() == Some("")
        && parts.next().is_some_and(slug_ok)
        && parts.next() == Some("releases")
        && parts.next().is_some_and(|v| !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b)))
        && parts.next().is_some_and(file_name_ok)
        && parts.next().is_none();
    if !ok {
        bail!("файл лежит не на сайте модов — скачивать его Modly не будет");
    }
    Ok(format!("{base}{path}"))
}

fn file_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._- ()".contains(&b))
}

/// Скачать файл выпуска и сверить размер и SHA-256. Прогресс — от 0 до `share`.
fn download(file: &SiteFile, dest: &Path, progress: &mut dyn FnMut(f32), share: f32) -> Result<()> {
    let url = file_url(file)?;
    let mut out = std::fs::File::create(dest).with_context(|| dest.display().to_string())?;
    let mut hasher = Sha256::new();
    let mut got = 0u64;
    let req = Request { timeout_ms: 60_000, ..Request::get(&url) };
    http::send(
        &req,
        &mut |status, len| {
            if status != 200 {
                bail!("сайт не отдал файл (код {status})");
            }
            if len.is_some_and(|l| l != file.size) {
                bail!("размер файла не совпадает с тем, что одобрил модератор");
            }
            Ok(())
        },
        &mut |chunk| {
            got += chunk.len() as u64;
            if got > file.size {
                bail!("файл больше, чем одобрил модератор");
            }
            hasher.update(chunk);
            out.write_all(chunk)?;
            progress(got as f32 / file.size as f32 * share);
            Ok(())
        },
    )?;
    if got != file.size {
        bail!("файл скачался не полностью");
    }
    let sum = hex(&hasher.finalize());
    if !sum.eq_ignore_ascii_case(&file.sha256) {
        bail!("контрольная сумма SHA-256 не совпала — файл повреждён или подменён, ставить его нельзя");
    }
    out.sync_all()?;
    Ok(())
}

/// Скачанный файл мода без пакета Modly (обычный архив, DLL и т.п.).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SiteDownload {
    pub slug: String,
    pub version: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Куда складывать скачанные файлы: «Загрузки\Modly». В портативном и тестовом режиме
/// (`MODLY_HOME`) — в папку данных Modly, чтобы ничего не попадало в профиль пользователя.
pub fn downloads_dir() -> PathBuf {
    if std::env::var_os("MODLY_HOME").is_some() {
        return crate::paths::data_dir().join("downloads");
    }
    #[cfg(windows)]
    let base = crate::win::known_downloads();
    #[cfg(not(windows))]
    let base: Option<PathBuf> = None;
    base.or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")))
        .unwrap_or_else(|| crate::paths::data_dir().join("downloads"))
        .join("Modly")
}

fn downloads_file() -> PathBuf {
    crate::paths::data_dir().join("downloads.json")
}

/// Что уже скачано (и до сих пор лежит на месте): адрес мода → файл.
pub fn site_downloads() -> HashMap<String, SiteDownload> {
    let list: Vec<SiteDownload> = std::fs::read(downloads_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    list.into_iter().filter(|d| slug_ok(&d.slug) && d.path.is_file()).map(|d| (d.slug.clone(), d)).collect()
}

fn remember_download(d: &SiteDownload) {
    let mut all = site_downloads();
    all.insert(d.slug.clone(), d.clone());
    let list: Vec<&SiteDownload> = all.values().collect();
    if let Ok(data) = serde_json::to_vec_pretty(&list) {
        let _ = crate::write_atomic(&downloads_file(), &data);
    }
}

/// Скачать файл мода, у которого нет пакета Modly: свежий одобренный выпуск, его первый файл
/// (ZIP — в первую очередь). Сначала во временный `.part`, после проверки размера и SHA-256 —
/// под настоящим именем. `progress(0…1)`.
pub fn download_file(slug: &str, progress: &mut dyn FnMut(f32)) -> Result<SiteDownload> {
    let d = details(slug, None)?;
    let (release, file) = d
        .releases
        .iter()
        .filter(|r| r.status == "approved")
        .find_map(|r| r.files.iter().find(|f| f.kind == "zip").or(r.files.first()).map(|f| (r, f)))
        .ok_or_else(|| anyhow!("у этого мода пока нет файлов"))?;
    if file.size == 0 || file.size > PACKAGE_LIMIT {
        bail!("файл слишком большой");
    }
    if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("у файла нет контрольной суммы");
    }
    let name = if file_name_ok(&file.name) { file.name.clone() } else { format!("{slug}-{}.{}", release.version, file.kind) };
    if !file_name_ok(&name) {
        bail!("неверное имя файла");
    }
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    let dest = dir.join(&name);
    let part = dir.join(format!("{name}.part"));
    let result = download(file, &part, progress, 1.0);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest).with_context(|| dest.display().to_string())?;
    progress(1.0);
    let out = SiteDownload { slug: slug.to_owned(), version: release.version.clone(), path: dest, size: file.size };
    remember_download(&out);
    log::info!("site download {} {} → {}", slug, out.version, out.path.display());
    Ok(out)
}

/// Распаковать пакет: сначала во временную папку, потом — на место прежней версии.
fn unpack(zip_path: &Path, root: &str, mods_dir: &Path) -> Result<(String, PathBuf)> {
    let root = root.trim_matches('/');
    if !root.is_empty() && (!safe_relative(root) || root.contains(['/', '\\'])) {
        bail!("неверная папка пакета в архиве");
    }
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?).context("архив повреждён")?;
    if zip.len() > MAX_ENTRIES {
        bail!("в архиве слишком много файлов");
    }
    let staging = mods_dir.join(format!(".incoming-{}", random_hex(8)?));
    std::fs::create_dir_all(&staging)?;
    let mut guard = TempDir(Some(staging.clone()));
    let prefix = if root.is_empty() { String::new() } else { format!("{root}/") };
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().replace('\\', "/");
        let Some(rel) = name.strip_prefix(&prefix) else { continue };
        let rel = rel.trim_end_matches('/');
        if rel.is_empty() {
            continue;
        }
        if !safe_relative(rel) || entry.enclosed_name().is_none() {
            bail!("в архиве опасный путь: {name}");
        }
        if entry.is_symlink() {
            bail!("в архиве ссылка на другой файл: {name}");
        }
        let out_path = staging.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(dir) = out_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = std::fs::File::create(&out_path).with_context(|| out_path.display().to_string())?;
        let left = UNPACK_LIMIT - total;
        let n = std::io::copy(&mut (&mut entry).take(left + 1), &mut out)?;
        total += n;
        if total > UNPACK_LIMIT {
            bail!("после распаковки архив больше 2 ГБ");
        }
    }

    let m = ModManifest::load(&staging).context("modly.json в архиве")?;
    if !id_ok(&m.id) {
        bail!("неверный id мода в modly.json: {}", m.id);
    }
    let target = mods_dir.join(&m.id);
    if target.exists() {
        if !target.join(MARKER).is_file() {
            bail!("в папке модов уже есть «{}», поставленный не из каталога — переименуйте или удалите его", m.id);
        }
        let old = mods_dir.join(format!(".old-{}", random_hex(8)?));
        std::fs::rename(&target, &old).context("мод занят — закройте программы, которые его используют")?;
        if let Err(e) = std::fs::rename(&staging, &target) {
            let _ = std::fs::rename(&old, &target);
            return Err(e).context("не удалось заменить прежнюю версию");
        }
        let _ = std::fs::remove_dir_all(&old);
    } else {
        std::fs::rename(&staging, &target)?;
    }
    guard.0 = None;
    Ok((m.id, target))
}

/// id мода годится в имя папки: латиница, цифры, `.`, `_`, `-`; не служебные имена Windows.
fn id_ok(id: &str) -> bool {
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    let upper = id.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        && !id.ends_with('.')
        && !reserved.contains(&base)
        && !((base.starts_with("COM") || base.starts_with("LPT")) && base.len() == 4)
}

// ============================================================================================
// Вход (необязательный)

fn token_file() -> PathBuf {
    crate::paths::data_dir().join("account.key")
}

fn account_file() -> PathBuf {
    crate::paths::data_dir().join("account.json")
}

fn token_ok(t: &str) -> bool {
    t.len() == 47 && t.starts_with("hmt_") && t[4..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Сохранённый вход, если он есть.
pub fn load_account() -> Option<Account> {
    let blob = std::fs::read(token_file()).ok()?;
    let token = String::from_utf8(unprotect(&blob).ok()?).ok()?;
    if !token_ok(&token) {
        return None;
    }
    let user: AccountUser = serde_json::from_slice(&std::fs::read(account_file()).ok()?).ok()?;
    Some(Account { token, user })
}

fn save_account(acc: &Account) -> Result<()> {
    crate::write_atomic(&token_file(), &protect(acc.token.as_bytes())?)?;
    crate::write_atomic(&account_file(), &serde_json::to_vec_pretty(&acc.user)?)?;
    Ok(())
}

/// Забыть вход на этом компьютере.
pub fn forget_account() {
    let _ = std::fs::remove_file(token_file());
    let _ = std::fs::remove_file(account_file());
}

/// Обновить ник и аватар (после проверки `me`).
pub fn update_account_user(acc: &Account) {
    let _ = crate::write_atomic(&account_file(), &serde_json::to_vec_pretty(&acc.user).unwrap_or_default());
}

/// Выйти: отозвать токен на сайте и забыть его здесь.
pub fn logout(acc: &Account) {
    let _ = api::<Value>("app/logout", Some(&acc.token), Some(&json!({})));
    forget_account();
}

#[derive(Deserialize)]
struct TokenResp {
    token: String,
    user: AccountUser,
}

/// Войти через браузер. Блокирует до конца входа (до 10 минут) — вызывать не в потоке интерфейса.
/// `open` открывает адрес в браузере, `cancel` — отменить ожидание.
pub fn login(device: &str, cancel: &AtomicBool, open: &dyn Fn(&str)) -> Result<Account> {
    let verifier = b64url(&random(32)?);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = b64url(&random(24)?);
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("не удалось открыть порт для входа")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    open(&format!(
        "{}/app-auth.php?port={port}&challenge={challenge}&state={state}&device={}",
        site_url(),
        url_encode(device)
    ));

    let deadline = Instant::now() + Duration::from_secs(600);
    let code = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(SiteError::Cancelled.into());
        }
        if Instant::now() > deadline {
            bail!("время на вход вышло — попробуйте ещё раз");
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(code) = callback(stream, &state)? {
                    break code;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.into()),
        }
    };
    let r: TokenResp = api("app/token", None, Some(&json!({ "code": code, "verifier": verifier })))?;
    if !token_ok(&r.token) {
        bail!("сайт выдал неверный токен");
    }
    let acc = Account { token: r.token, user: r.user };
    save_account(&acc)?;
    Ok(acc)
}

/// Один запрос браузера на 127.0.0.1. `Some(code)` — вход подтверждён, `None` — чужой запрос, ждём дальше.
fn callback(mut stream: TcpStream, state: &str) -> Result<Option<String>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 8192 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return Ok(None),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method != "GET" || path != "/callback" {
        respond(&mut stream, 404, "", "");
        return Ok(None);
    }
    let q: HashMap<String, String> = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), url_decode(v)))
        .collect();
    let got_state = q.get("state").map(String::as_str).unwrap_or("");
    if !ct_eq(got_state.as_bytes(), state.as_bytes()) {
        // Не наш вход (старая вкладка или чужая программа) — ждём настоящий.
        respond(&mut stream, 400, "Ссылка устарела", "Начните вход заново в Modly. / Start signing in again in Modly.");
        return Ok(None);
    }
    if q.contains_key("error") {
        respond(&mut stream, 200, "Вход отменён · Sign-in cancelled", "Эту вкладку можно закрыть. / You can close this tab.");
        return Err(SiteError::Cancelled.into());
    }
    let code = q.get("code").cloned().unwrap_or_default();
    if code.len() < 16 || code.len() > 128 || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        respond(&mut stream, 400, "Ссылка устарела", "Начните вход заново в Modly. / Start signing in again in Modly.");
        return Ok(None);
    }
    respond(&mut stream, 200, "Готово! · Done!", "Возвращайтесь в Modly — вход уже там. Эту вкладку можно закрыть. / Go back to Modly. You can close this tab.");
    Ok(Some(code))
}

/// Маленькая страница в браузере после входа.
fn respond(stream: &mut TcpStream, status: u16, title: &str, text: &str) {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = if title.is_empty() {
        String::new()
    } else {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Modly</title><style>\
             body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0a0c11;color:#eef0f7;\
             font:16px 'Segoe UI',sans-serif}}main{{max-width:460px;padding:36px;border-radius:20px;background:#131722;\
             border:1px solid #2b3347;text-align:center}}h1{{margin:0 0 10px;font-size:26px}}p{{margin:0;color:#b2b9cc;line-height:1.55}}\
             b{{display:inline-block;width:56px;height:56px;line-height:56px;border-radius:18px;margin-bottom:16px;\
             background:#8b5cf6;color:#fff;font-size:28px}}</style></head><body><main><b>M</b><h1>{}</h1><p>{}</p></main></body></html>",
            esc(title),
            esc(text)
        )
    };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nReferrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

// ============================================================================================
// Мелочи

#[cfg(windows)]
fn random(n: usize) -> Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    crate::win::random_bytes(&mut v)?;
    Ok(v)
}

#[cfg(not(windows))]
fn random(_n: usize) -> Result<Vec<u8>> {
    bail!("нет источника случайных чисел")
}

#[cfg(windows)]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    crate::win::protect(data)
}

#[cfg(windows)]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    crate::win::unprotect(data)
}

#[cfg(not(windows))]
fn protect(_data: &[u8]) -> Result<Vec<u8>> {
    bail!("нет хранилища ключей")
}

#[cfg(not(windows))]
fn unprotect(_data: &[u8]) -> Result<Vec<u8>> {
    bail!("нет хранилища ключей")
}

fn random_hex(n: usize) -> Result<String> {
    Ok(hex(&random(n)?))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 3);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        match b[i] {
            b'%' if i + 2 < b.len() => match (digit(b[i + 1]), digit(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Сравнение без утечки по времени.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Имя компьютера — подпись входа в «Аккаунт и устройства» на сайте.
pub fn device_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Modly".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_the_site() {
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        // PKCE: проверочная строка из RFC 7636, приложение B
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(b64url(&Sha256::digest(v.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(url_encode("Игровой ПК"), "%D0%98%D0%B3%D1%80%D0%BE%D0%B2%D0%BE%D0%B9%20%D0%9F%D0%9A");
        assert_eq!(url_decode("a%20b+c%2"), "a b c%2");
    }

    #[test]
    fn game_names_compare_loosely_and_catalog_reads_game_info() {
        assert_eq!(game_key("Family Time Ep.3"), "familytimeep3");
        assert_eq!(game_key("family time  ep 3"), game_key("Family-Time Ep.3"));
        assert_ne!(game_key("Family Time Ep.2"), game_key("Family Time Ep.3"));
        assert_eq!(game_key("Игра: Тест"), "игратест");

        let m: SiteMod = serde_json::from_str(
            r#"{"slug":"x","title":"X","game":"G","game_info":{"id":"g","exe":"G.exe","steam_appid":42,"builds":["b1"]}}"#,
        )
        .unwrap();
        assert_eq!((m.game_info.steam_appid, m.game_info.builds.len()), (Some(42), 1));
        // старый сайт без game_info, и null вместо номера Steam
        let old: SiteMod = serde_json::from_str(r#"{"slug":"x","title":"X","game_info":{"steam_appid":null}}"#).unwrap();
        assert!(old.game_info.steam_appid.is_none() && old.game_info.id.is_empty());
        let older: SiteMod = serde_json::from_str(r#"{"slug":"x","title":"X"}"#).unwrap();
        assert!(older.game_info.builds.is_empty());
    }

    #[test]
    fn mod_ids_are_safe_folder_names() {
        for ok in ["creatly", "wolf-pack", "My.Mod_2"] {
            assert!(id_ok(ok), "{ok}");
        }
        for bad in ["", ".hidden", "a/b", "a\\b", "..", "con", "NUL.txt", "com1", "lpt9", "x.", "имя"] {
            assert!(!id_ok(bad), "{bad}");
        }
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        let mut w = zip::ZipWriter::new(f.reopen().unwrap());
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        f
    }

    const MANIFEST: &[u8] = br#"{"id":"wolf-pack","name":"Wolf Pack","game":{"id":"g","name":"G","exe":"G.exe"},
        "install":{"method":"unity-mono","assembly":"bin/W.dll","entry":"W.E.Init"}}"#;

    #[test]
    fn unpacks_package_and_replaces_previous_version() {
        let mods = tempfile::tempdir().unwrap();
        let z = zip_with(&[("WolfPack/modly.json", MANIFEST), ("WolfPack/bin/W.dll", b"v1")]);
        let (id, dir) = unpack(z.path(), "WolfPack", mods.path()).unwrap();
        assert_eq!(id, "wolf-pack");
        assert_eq!(std::fs::read(dir.join("bin/W.dll")).unwrap(), b"v1");
        // без метки «из каталога» чужую папку не заменяем
        let z2 = zip_with(&[("WolfPack/modly.json", MANIFEST), ("WolfPack/bin/W.dll", b"v2")]);
        assert!(unpack(z2.path(), "WolfPack", mods.path()).is_err());
        std::fs::write(dir.join(MARKER), b"{}").unwrap();
        let (_, dir) = unpack(z2.path(), "WolfPack", mods.path()).unwrap();
        assert_eq!(std::fs::read(dir.join("bin/W.dll")).unwrap(), b"v2");
        // временных папок не осталось
        let left: Vec<_> = std::fs::read_dir(mods.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

    #[test]
    fn recognises_images_by_their_bytes() {
        use ImageFormat::*;
        assert_eq!(image_format(b"\x89PNG\r\n\x1a\n...."), Some(Png));
        assert_eq!(image_format(&[0xff, 0xd8, 0xff, 0xe0, 0]), Some(Jpeg));
        assert_eq!(image_format(b"RIFF\x10\0\0\0WEBPVP8 "), Some(Webp));
        assert_eq!(image_format(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#), Some(Svg));
        assert_eq!(
            image_format(b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!-- logo -->\n<!DOCTYPE svg>\n<svg viewBox=\"0 0 1 1\"></svg>"),
            Some(Svg)
        );
        for not_image in [
            &b""[..],
            b"RIFF\x10\0\0\0WAVEfmt ",
            b"<!doctype html><html><body><svg></svg></body></html>",
            b"<svgx/>",
            b"\x1f\x8b\x08\0\0\0\0\0",
            b"just text",
        ] {
            assert_eq!(image_format(not_image), None, "{:?}", String::from_utf8_lossy(not_image));
        }
    }

    #[test]
    fn thumbs_only_from_the_site() {
        let dir = tempfile::tempdir().unwrap();
        let site = "https://mods.hackerstone.xyz";
        for bad in [
            "",
            "https://mods.hackerstone.xyz",
            "https://mods.hackerstone.xyz.evil.com/a.webp",
            "https://mods.hackerstone.xyz@evil.com/a.webp",
            "http://mods.hackerstone.xyz/a.webp",
            "https://evil.com/https://mods.hackerstone.xyz/a.webp",
            "https://mods.hackerstone.xyz/a b.webp",
        ] {
            assert!(thumb_cached(dir.path(), site, bad).is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Крошечный сайт на 127.0.0.1: путь → готовый HTTP-ответ.
    #[cfg(windows)]
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let site = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_owned();
                let path = line.split(' ').nth(1).unwrap_or("");
                let resp = routes.iter().find(|r| r.0 == path).map(|r| r.1.clone());
                let _ = stream.write_all(&resp.unwrap_or_else(|| b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()));
            }
        });
        site
    }

    #[cfg(windows)]
    fn ok(body: &[u8]) -> Vec<u8> {
        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        r.extend_from_slice(body);
        r
    }

    #[cfg(windows)]
    #[test]
    fn downloads_thumbs_into_the_cache() {
        let png = b"\x89PNG\r\n\x1a\nnot really a picture".to_vec();
        let site = serve(vec![
            ("/uploads/mods/a.t.webp", ok(&png)),
            ("/page.html", ok(b"<!doctype html><title>x</title>")),
            ("/moved.webp", b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x.png\r\nContent-Length: 0\r\n\r\n".to_vec()),
            ("/huge.webp", b"HTTP/1.1 200 OK\r\nContent-Length: 3145728\r\n\r\n".to_vec()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{site}/uploads/mods/a.t.webp");
        assert_eq!(thumb_cached(dir.path(), &site, &url).unwrap(), png);
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read(&files[0]).unwrap(), png);

        for bad in ["/page.html", "/moved.webp", "/huge.webp", "/missing.webp"] {
            assert!(thumb_cached(dir.path(), &site, &format!("{site}{bad}")).is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "в кэш попала только картинка");

        // Свежая копия берётся с диска, а устаревшая выручает, когда сайт не отвечает.
        let gone = "http://127.0.0.1:1";
        let cached = |f: &Path| {
            let name = format!("{}.img", &hex(&Sha256::digest(format!("{gone}/a.webp").as_bytes()))[..32]);
            std::fs::copy(f, dir.path().join(name)).unwrap();
        };
        cached(&files[0]);
        assert_eq!(thumb_cached(dir.path(), gone, &format!("{gone}/a.webp")).unwrap(), png);
        let week_ago = std::time::SystemTime::now() - THUMB_FRESH - Duration::from_secs(60);
        for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
            std::fs::File::options().write(true).open(f.path()).unwrap().set_modified(week_ago).unwrap();
        }
        assert_eq!(thumb_cached(dir.path(), gone, &format!("{gone}/a.webp")).unwrap(), png);
        assert!(thumb_cached(dir.path(), gone, &format!("{gone}/b.webp")).is_err());
    }

    #[test]
    fn thumb_cache_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let start = std::time::SystemTime::now() - Duration::from_secs(10_000);
        for i in 0..THUMB_CACHE_FILES + 5 {
            let f = dir.path().join(format!("{i:04}.img"));
            std::fs::write(&f, b"x").unwrap();
            std::fs::File::options().write(true).open(&f).unwrap().set_modified(start + Duration::from_secs(i as u64)).unwrap();
        }
        std::fs::write(dir.path().join("other.txt"), b"x").unwrap();
        prune_thumbs(dir.path());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), THUMB_CACHE_FILES + 1);
        assert!(!dir.path().join("0004.img").exists());
        assert!(dir.path().join("0005.img").exists());
        assert!(dir.path().join("other.txt").exists());
    }

    #[test]
    fn refuses_dangerous_archives() {
        let mods = tempfile::tempdir().unwrap();
        let slip = zip_with(&[("modly.json", MANIFEST), ("../evil.dll", b"x")]);
        assert!(unpack(slip.path(), "", mods.path()).is_err());
        let bad_root = zip_with(&[("modly.json", MANIFEST)]);
        assert!(unpack(bad_root.path(), "../x", mods.path()).is_err());
        let bad_id = zip_with(&[("modly.json", br#"{"id":"..","name":"X","game":{"id":"g","name":"G","exe":"G.exe"},
            "install":{"method":"files","files":[]}}"#)]);
        assert!(unpack(bad_id.path(), "", mods.path()).is_err());
        // ничего не распаковалось и не осталось
        assert_eq!(std::fs::read_dir(mods.path()).unwrap().count(), 0);
        assert!(!mods.path().join("..").join("evil.dll").exists());
    }
}
