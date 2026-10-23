//! Инструменты мододела — по желанию, в установщике: .NET SDK (моды на C# для Unity и .NET) и
//! Visual Studio Build Tools с C++ (моды для Unreal и нативных игр). Самому Modly они не нужны:
//! его среда выполнения вшита в exe. Ставятся через winget — так берётся официальный пакет
//! Microsoft, а Windows сама спросит права администратора.

use crate::ops::t;
use anyhow::{Result, bail};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct Tool {
    pub id: &'static str,
    pub present: bool,
}

impl Tool {
    pub fn title(&self) -> String {
        match self.id {
            "dotnet" => ".NET SDK 8".to_owned(),
            _ => t("C++ Build Tools", "C++ Build Tools"),
        }
    }

    pub fn text(&self) -> String {
        match self.id {
            "dotnet" => t("Моды на C# для Unity и .NET (~250 МБ)", "C# mods for Unity and .NET (~250 MB)"),
            _ => t("Моды на C++ для Unreal и нативных игр (~2 ГБ)", "C++ mods for Unreal and native games (~2 GB)"),
        }
    }
}

/// Что уже есть на компьютере.
pub fn detect() -> Vec<Tool> {
    vec![Tool { id: "dotnet", present: has_dotnet_sdk() }, Tool { id: "cpp", present: has_cpp_tools() }]
}

fn quiet(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn has_dotnet_sdk() -> bool {
    quiet("dotnet", &["--list-sdks"]).is_some_and(|o| o.lines().any(|l| l.trim().chars().next().is_some_and(|c| c.is_ascii_digit())))
}

fn vswhere() -> PathBuf {
    let pf = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    pf.join(r"Microsoft Visual Studio\Installer\vswhere.exe")
}

fn has_cpp_tools() -> bool {
    let w = vswhere();
    w.is_file()
        && quiet(
            &w.display().to_string(),
            &["-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
        )
        .is_some_and(|o| !o.trim().is_empty())
}

pub fn has_winget() -> bool {
    quiet("winget", &["--version"]).is_some()
}

/// Поставить инструмент через winget. Долго (минуты), окно установщика тем временем крутит кольцо.
pub fn install(id: &str) -> Result<()> {
    let mut args: Vec<&str> = vec!["install", "--exact", "--source", "winget", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"];
    match id {
        "dotnet" => args.extend(["--id", "Microsoft.DotNet.SDK.8", "--silent"]),
        "cpp" => args.extend([
            "--id",
            "Microsoft.VisualStudio.2022.BuildTools",
            "--override",
            "--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended",
        ]),
        _ => bail!("неизвестный инструмент {id}"),
    }
    log::info!("winget {}", args.join(" "));
    let out = Command::new("winget").args(&args).creation_flags(CREATE_NO_WINDOW).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    log::info!("winget exit {:?}: {}", out.status.code(), text.lines().rev().take(5).collect::<Vec<_>>().join(" | "));
    // 0x8A15002B — уже стоит, новее нет
    if out.status.success() || out.status.code() == Some(0x8A15_002Bu32 as i32) {
        Ok(())
    } else {
        bail!("winget: код {}", out.status.code().unwrap_or(-1))
    }
}
