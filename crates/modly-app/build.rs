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

    // Иконка и сведения о программе в exe (Проводник, панель задач).
    #[cfg(windows)]
    if Path::new("assets/modly.ico").exists() {
        embed_resource::compile("assets/modly.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
    println!("cargo:rerun-if-changed=assets/modly.rc");
    println!("cargo:rerun-if-changed=assets/modly.ico");
}
