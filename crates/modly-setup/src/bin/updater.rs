//! modly-updater.exe — лежит рядом с modly.exe: обновления и удаление Modly.
//!
//!   modly-updater.exe                              — проверить обновления (окно)
//!   modly-updater.exe --apply --target <папка> --from <версия> [--pid <n>]
//!                                                  — поставить пакет, рядом с которым лежит этот exe
//!   modly-updater.exe --uninstall [--quiet]         — удалить Modly («Приложения и возможности»)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use modly_setup::{Apply, Mode, arg_value, has_flag, ops, system};
use std::path::{Path, PathBuf};
use std::process::exit;

fn main() {
    modly_setup::init_logging("updater");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if modly_setup::escape_package_sandbox(&args) {
        return;
    }
    let own_dir = modly_core::paths::exe_dir();
    let target = arg_value(&args, "--target").map(PathBuf::from).unwrap_or_else(|| own_dir.clone());

    if has_flag(&args, &["--uninstall"]) {
        let quiet = has_flag(&args, &["--quiet", "/S"]);
        let from_temp = has_flag(&args, &["--from-temp"]);
        // Свой exe из папки Modly не удалить, пока он работает: удаляет копия во временной папке.
        if !from_temp && system::same_dir(&own_dir, &target) {
            match relaunch_from_temp(&target, quiet) {
                Ok(code) => exit(code),
                Err(e) => log::warn!("temp copy: {e:#} — uninstalling in place"),
            }
        }
        let cleanup = from_temp.then(|| own_dir.clone());
        // копия из папки Modly держит свой exe, пока не выйдет — дождаться её
        if let Some(pid) = arg_value(&args, "--wait-pid").and_then(|p| p.parse().ok()) {
            system::wait_exit(pid, 15_000);
        }
        if quiet {
            let code = quiet_uninstall(&target);
            if let Some(dir) = cleanup {
                system::delete_after_exit(&[dir]);
            }
            exit(code);
        }
        exit(modly_setup::run(Mode::Uninstall { dir: target, cleanup }));
    }

    if has_flag(&args, &["--apply"]) {
        let from = arg_value(&args, "--from").unwrap_or_default();
        let pid = arg_value(&args, "--pid").and_then(|p| p.parse().ok());
        exit(modly_setup::run(Mode::Update { dir: target, apply: Some(Apply { from, pid }) }));
    }

    exit(modly_setup::run(Mode::Update { dir: target, apply: None }));
}

fn relaunch_from_temp(target: &Path, quiet: bool) -> Result<i32> {
    let tmp = std::env::temp_dir().join(format!("modly-uninstall-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let exe = tmp.join(modly_core::update::UPDATER_EXE);
    std::fs::copy(std::env::current_exe()?, &exe).context("copy to temp")?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["--uninstall", "--from-temp", "--target"]).arg(target);
    cmd.args(["--wait-pid", &std::process::id().to_string()]);
    if quiet {
        cmd.arg("--quiet");
    }
    // не ждём копию: пока этот exe работает, его файл в папке Modly не удалить
    cmd.spawn()?;
    Ok(0)
}

fn quiet_uninstall(dir: &Path) -> i32 {
    for pid in system::modly_pids(dir) {
        system::close_process(pid);
    }
    match ops::uninstall(dir, false, &|_| {}) {
        Ok(()) => 0,
        Err(e) => {
            log::error!("quiet uninstall: {e:#}");
            1
        }
    }
}
