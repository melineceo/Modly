use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");

    // Все SVG из ui/icons вшиваются в exe: мод может назвать любую иконку по имени.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui").join("icons");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("ui/icons")
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".svg")).map(str::to_owned))
        .filter(|n| n != "logo")
        .collect();
    names.sort();
    let mut code = String::from("pub static ICONS: &[(&str, &[u8])] = &[\n");
    for n in &names {
        let path = dir.join(format!("{n}.svg"));
        writeln!(code, "    ({n:?}, include_bytes!({:?})),", path.display().to_string()).unwrap();
    }
    code.push_str("];\n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("icons.rs");
    std::fs::write(out, code).unwrap();
    println!("cargo:rerun-if-changed=ui/icons");

    // Иконка и сведения о программе в exe (Проводник, панель задач). Версия — из Cargo.toml.
    #[cfg(windows)]
    {
        let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("modly.ico");
        let rc = Path::new(&std::env::var("OUT_DIR").unwrap()).join("modly.rc");
        std::fs::write(&rc, resource(&icon, &std::env::var("CARGO_PKG_VERSION").unwrap())).unwrap();
        embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().unwrap();
    }
    println!("cargo:rerun-if-changed=assets/modly.ico");
}

/// Сведения о программе: `0.2.0-beta` → FILEVERSION 0,2,0,0 и «0.2.0 Beta».
#[cfg(windows)]
fn resource(icon: &Path, version: &str) -> String {
    let (num, pre) = version.split_once('-').unwrap_or((version, ""));
    let mut parts: Vec<u32> = num.split('.').filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let commas = parts.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let mut c = pre.chars();
    let pre = c.next().map(|f| f.to_uppercase().to_string() + c.as_str()).unwrap_or_default();
    let label = if pre.is_empty() { num.to_owned() } else { format!("{num} {pre}") };
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
            VALUE "FileDescription", "Modly — mod manager and live control panel"
            VALUE "FileVersion", "{label}"
            VALUE "InternalName", "modly"
            VALUE "OriginalFilename", "modly.exe"
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
