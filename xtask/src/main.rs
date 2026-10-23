//! Выпуск Modly: `cargo xtask <команда>`.
//!
//! - `keygen` — создать ключ подписи обновлений (один раз; хранить в секрете и в резервной копии);
//! - `dist` — собрать выпуск в `target/dist`: установщик, архив (он же пакет обновления),
//!   подписанный манифест, контрольные суммы и текст выпуска;
//! - `dist --publish` — то же и выложить на GitHub (`gh release create`; бета — как pre-release);
//! - `sign <файл>` — подписать файл ключом выпусков (`<файл>.sig`).
//!
//! Ключ — `%USERPROFILE%\.modly\release-signing-key.txt` (или путь в `MODLY_SIGNING_KEY`). В репозиторий
//! он не попадает. Без него собрать выпуск, который примут уже установленные копии Modly, нельзя.

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signer, SigningKey};
use modly_core::update::{self, Asset, Manifest};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(flag("--force")),
        Some("dist") => dist(flag("--publish")),
        Some("sign") => sign_file(Path::new(args.get(1).context("cargo xtask sign <файл>")?)),
        _ => {
            eprintln!("cargo xtask keygen [--force] | dist [--publish] | sign <file>");
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn key_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MODLY_SIGNING_KEY") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    home.join(".modly").join("release-signing-key.txt")
}

fn load_key() -> Result<SigningKey> {
    let p = key_path();
    let text = std::fs::read_to_string(&p).with_context(|| format!("ключ подписи {} — создайте его: cargo xtask keygen", p.display()))?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or_default();
    let seed: [u8; 32] = update::unhex(line).and_then(|b| b.try_into().ok()).context("ключ подписи повреждён")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn keygen(force: bool) -> Result<()> {
    let p = key_path();
    if p.exists() && !force {
        bail!(
            "ключ уже есть: {}\nНовый ключ — только с --force, и его открытую часть нужно заранее вшить в выпуск, подписанный старым: иначе установленные копии перестанут принимать обновления.",
            p.display()
        );
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    let public = update::hex(key.verifying_key().as_bytes());
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(
        &p,
        format!(
            "# Modly: закрытый ключ подписи обновлений. Никому не передавать, не класть в репозиторий,\n# сохранить резервную копию (без него не выпустить обновление).\n# Открытый ключ: {public}\n{}\n",
            update::hex(&seed)
        ),
    )?;
    let src = root().join("crates/modly-core/src/update.rs");
    let code = std::fs::read_to_string(&src)?;
    let placeholder = format!("\"{}\"", "0".repeat(64));
    if code.contains(&placeholder) {
        std::fs::write(&src, code.replace(&placeholder, &format!("\"{public}\"")))?;
        println!("Открытый ключ вшит в crates/modly-core/src/update.rs");
    } else {
        println!("Добавьте открытый ключ в PUBLIC_KEYS (crates/modly-core/src/update.rs).");
    }
    println!("Закрытый ключ: {}\nОткрытый ключ: {public}", p.display());
    Ok(())
}

fn sign_file(path: &Path) -> Result<()> {
    let key = load_key()?;
    let data = std::fs::read(path)?;
    let sig = update::hex(&key.sign(&data).to_bytes());
    let out = PathBuf::from(format!("{}.sig", path.display()));
    std::fs::write(&out, format!("{sig}\n"))?;
    println!("{}", out.display());
    Ok(())
}

fn workspace_version(root: &Path) -> Result<String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let section = toml.split("[workspace.package]").nth(1).context("[workspace.package]")?;
    let line = section.lines().find(|l| l.trim_start().starts_with("version")).context("version")?;
    let v = line.split('"').nth(1).context("version")?.to_owned();
    update::Version::parse(&v).with_context(|| format!("версия {v}"))?;
    Ok(v)
}

fn cargo(root: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Result<()> {
    println!("> cargo {}", args.join(" "));
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args(args).current_dir(root);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if !cmd.status()?.success() {
        bail!("cargo {} не собралось", args.join(" "));
    }
    Ok(())
}

fn write_zip(dest: &Path, files: &[(PathBuf, &str)]) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(std::fs::File::create(dest)?);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(9));
    for (src, name) in files {
        w.start_file(*name, opts)?;
        w.write_all(&std::fs::read(src).with_context(|| format!("{}", src.display()))?)?;
    }
    w.finish()?;
    Ok(())
}

fn asset(path: &Path) -> Result<Asset> {
    Ok(Asset {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        size: std::fs::metadata(path)?.len(),
        sha256: update::file_sha256(path)?,
    })
}

/// Сегодняшняя дата ГГГГ-ММ-ДД (UTC).
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86400).unwrap_or(0) as i64;
    // civil_from_days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Что нового из CHANGELOG.md: раздел `## <версия>`, в нём `### Русский` и `### English`.
fn changelog_notes(root: &Path, version: &str) -> Result<(String, String)> {
    let text = std::fs::read_to_string(root.join("CHANGELOG.md")).context("CHANGELOG.md")?;
    let mut section = String::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            if inside {
                break;
            }
            inside = h.split_whitespace().next() == Some(version);
            continue;
        }
        if inside {
            section.push_str(line);
            section.push('\n');
        }
    }
    if section.trim().is_empty() {
        bail!("в CHANGELOG.md нет раздела «## {version}»");
    }
    let part = |name: &str| -> Option<String> {
        let start = section.find(&format!("### {name}"))?;
        let body = &section[start..];
        let body = &body[body.find('\n').map(|i| i + 1).unwrap_or(body.len())..];
        let end = body.find("\n### ").unwrap_or(body.len());
        Some(body[..end].trim().to_owned())
    };
    let ru = part("Русский").unwrap_or_else(|| section.trim().to_owned());
    let en = part("English").unwrap_or_default();
    Ok((ru, en))
}

fn dist(publish: bool) -> Result<()> {
    let root = root();
    let version = workspace_version(&root)?;
    let key = load_key()?;
    let (notes_ru, notes_en) = changelog_notes(&root, &version)?;
    // CARGO_TARGET_DIR — своя папка сборки (например, для проверки обновления второй версией)
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).map(|p| if p.is_absolute() { p } else { root.join(p) }).unwrap_or_else(|| root.join("target"));
    let dist = target.join("dist");
    std::fs::create_dir_all(&dist)?;
    let rel = target.join("release");

    cargo(&root, &["build", "--release", "-p", "modly-app", "--bin", "modly"], &[])?;
    cargo(&root, &["build", "--release", "-p", "modly-setup", "--bin", "modly-updater"], &[])?;

    let pkg = dist.join(format!("Modly-{version}-windows-x64.zip"));
    write_zip(
        &pkg,
        &[
            (rel.join(update::APP_EXE), "Modly/modly.exe"),
            (rel.join(update::UPDATER_EXE), "Modly/modly-updater.exe"),
            (root.join("LICENSE"), "Modly/LICENSE.txt"),
            (root.join("README.md"), "Modly/README.md"),
        ],
    )?;
    println!("архив: {}", pkg.display());

    cargo(&root, &["build", "--release", "-p", "modly-setup", "--bin", "modly-setup"], &[("MODLY_PAYLOAD", &pkg)])?;
    let setup = dist.join(format!("ModlySetup-{version}.exe"));
    std::fs::copy(rel.join("modly-setup.exe"), &setup)?;
    println!("установщик: {}", setup.display());

    let manifest = Manifest {
        app: "modly".into(),
        version: version.clone(),
        date: today(),
        notes_ru: notes_ru.clone(),
        notes_en: notes_en.clone(),
        package: asset(&pkg)?,
        setup: Some(asset(&setup)?),
    };
    let data = serde_json::to_vec_pretty(&manifest)?;
    let sig = update::hex(&key.sign(&data).to_bytes());
    // Своя проверка тем ключом, что вшит в Modly: иначе выпуск никто не примет.
    update::verify_manifest(&data, &sig).context("подпись не подходит к ключу, вшитому в Modly")?;
    std::fs::write(dist.join(update::MANIFEST_NAME), &data)?;
    std::fs::write(dist.join(update::SIG_NAME), format!("{sig}\n"))?;

    let sums = format!(
        "{}  {}\n{}  {}\n",
        manifest.setup.as_ref().unwrap().sha256,
        manifest.setup.as_ref().unwrap().name,
        manifest.package.sha256,
        manifest.package.name
    );
    std::fs::write(dist.join("SHA256SUMS.txt"), &sums)?;
    let label = update::label(&version);
    let notes = format!(
        "{notes_ru}\n\n**Установка:** скачайте `{setup}` и запустите. Уже стоит Modly 0.2 или новее — он обновится сам.\n\
         Портативная версия — архив `{pkg}`.\n\n---\n\n{notes_en}\n\n**Install:** download `{setup}` and run it. Modly 0.2 or newer updates itself.\n\
         Portable version: `{pkg}`.\n\n**SHA-256**\n```\n{sums}```\n",
        setup = manifest.setup.as_ref().unwrap().name,
        pkg = manifest.package.name,
    );
    std::fs::write(dist.join("release-notes.md"), &notes)?;
    println!("манифест подписан: {}", dist.join(update::MANIFEST_NAME).display());

    if publish {
        publish_release(&root, &version, &label, &dist, &[&setup, &pkg, &dist.join(update::MANIFEST_NAME), &dist.join(update::SIG_NAME)])?;
    } else {
        println!("Готово. Выложить: cargo xtask dist --publish");
    }
    Ok(())
}

fn gh() -> PathBuf {
    let installed = PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe");
    if installed.is_file() { installed } else { PathBuf::from("gh") }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(root).output()?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn publish_release(root: &Path, version: &str, label: &str, dist: &Path, files: &[&Path]) -> Result<()> {
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        bail!("в репозитории есть незакоммиченные изменения — сначала коммит и push");
    }
    let head = git(root, &["rev-parse", "HEAD"])?;
    let upstream = git(root, &["rev-parse", "@{u}"]).unwrap_or_default();
    if head != upstream {
        bail!("коммит {head:.8} ещё не на GitHub — сначала git push");
    }
    let tag = format!("v{version}");
    let mut cmd = Command::new(gh());
    cmd.current_dir(root).args(["release", "create", &tag]);
    for f in files {
        cmd.arg(f);
    }
    cmd.args(["--title", &format!("Modly {label}"), "--target", &head, "--notes-file"]).arg(dist.join("release-notes.md"));
    if update::Version::parse(version).is_some_and(|v| v.is_prerelease()) {
        cmd.arg("--prerelease");
    }
    println!("> gh release create {tag}");
    if !cmd.status()?.success() {
        bail!("gh release create не получилось");
    }
    println!("Выпуск {tag} опубликован.");
    Ok(())
}
