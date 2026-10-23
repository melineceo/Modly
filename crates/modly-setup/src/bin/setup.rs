//! ModlySetup.exe — установщик Modly. Внутри — архив выпуска.
//!
//!   ModlySetup.exe [--dir <папка>]   — окно установки
//!   ModlySetup.exe --silent [--dir <папка>] — без окна: ярлыки и ссылки modly:// по умолчанию

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use modly_setup::{Mode, ops, system};
use std::path::PathBuf;

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

fn main() {
    modly_setup::init_logging("setup");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = modly_setup::arg_value(&args, "--dir").map(PathBuf::from);
    if modly_setup::has_flag(&args, &["--silent", "/S", "/SILENT", "/VERYSILENT"]) {
        std::process::exit(silent(dir));
    }
    let elevated = modly_setup::arg_value(&args, "--elevated");
    std::process::exit(modly_setup::run(Mode::Install { payload: PAYLOAD, dir, elevated }));
}

fn silent(dir: Option<PathBuf>) -> i32 {
    let dir = dir.or_else(|| system::installed().map(|(d, _)| d)).unwrap_or_else(system::default_install_dir);
    for pid in system::modly_pids(&dir) {
        system::close_process(pid);
    }
    let opts = ops::Options { dir, desktop: true, start_menu: true, protocol: true };
    match ops::install(PAYLOAD, &opts, modly_setup::VERSION, &|_| {}) {
        Ok(()) => 0,
        Err(e) => {
            log::error!("silent install: {e:#}");
            1
        }
    }
}
