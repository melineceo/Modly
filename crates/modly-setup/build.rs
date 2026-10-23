use std::path::{Path, PathBuf};

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/setup.slint", config).expect("Slint build failed");
    println!("cargo:rerun-if-changed=../modly-app/ui");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // Архив выпуска внутри установщика: MODLY_PAYLOAD — путь к Modly-<версия>-windows-x64.zip
    // (его собирает cargo xtask dist). Без него установщик собирается пустым — для проверки окна.
    println!("cargo:rerun-if-env-changed=MODLY_PAYLOAD");
    let payload = out.join("payload.zip");
    match std::env::var_os("MODLY_PAYLOAD").map(PathBuf::from) {
        Some(src) => {
            println!("cargo:rerun-if-changed={}", src.display());
            std::fs::copy(&src, &payload).unwrap_or_else(|e| panic!("MODLY_PAYLOAD {}: {e}", src.display()));
        }
        None => std::fs::write(&payload, b"").unwrap(),
    }

    // Иконка и сведения о программе: у установщика и обновлятора — свои описания.
    #[cfg(windows)]
    {
        let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("../modly-app/assets/modly.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        for (bin, desc) in [("modly-setup", "Modly Setup"), ("modly-updater", "Modly Updater")] {
            let rc = out.join(format!("{bin}.rc"));
            std::fs::write(&rc, resource(&icon, bin, desc, &version)).unwrap();
            embed_resource::compile_for(&rc, [bin], embed_resource::NONE).manifest_optional().unwrap();
        }
    }
}

#[cfg(windows)]
fn resource(icon: &Path, bin: &str, desc: &str, version: &str) -> String {
    let (num, _) = version.split_once('-').unwrap_or((version, ""));
    let mut parts: Vec<u32> = num.split('.').filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let commas = parts.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let label = match version.split_once('-') {
        Some((n, pre)) => {
            let mut c = pre.chars();
            format!("{n} {}{}", c.next().map(|f| f.to_uppercase().to_string()).unwrap_or_default(), c.as_str())
        }
        None => version.to_owned(),
    };
    let icon = icon.display().to_string().replace('\\', "\\\\");
    format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "Meline / Hackerstone"
            VALUE "FileDescription", "{desc}"
            VALUE "FileVersion", "{label}"
            VALUE "InternalName", "{bin}"
            VALUE "OriginalFilename", "{bin}.exe"
            VALUE "ProductName", "Modly"
            VALUE "ProductVersion", "{label}"
            VALUE "LegalCopyright", "MIT, Meline / Hackerstone"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}
