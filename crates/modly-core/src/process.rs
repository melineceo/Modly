//! Запущенные игры и запуск.

use crate::library::Game;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// Снимок запущенных процессов: путь к exe (в нижнем регистре) → pid.
#[derive(Default, Clone)]
pub struct Running {
    by_exe: HashMap<String, u32>,
}

impl Running {
    /// Снимок по играм: полный путь узнаётся только у процессов с такими же именами exe.
    pub fn snapshot(games: &[Game]) -> Self {
        let names: std::collections::HashSet<String> = games.iter().map(|g| g.exe.to_lowercase()).collect();
        let mut by_exe = HashMap::new();
        #[cfg(windows)]
        if !names.is_empty() {
            for (pid, path) in crate::win::processes(Some(&names)) {
                by_exe.insert(path.to_string_lossy().to_lowercase(), pid);
            }
        }
        Running { by_exe }
    }

    pub fn pid_of(&self, game: &Game) -> Option<u32> {
        self.by_exe.get(&game.exe_path().to_string_lossy().to_lowercase()).copied()
    }

    pub fn is_running(&self, game: &Game) -> bool {
        self.pid_of(game).is_some()
    }

    /// Те же ли игры запущены, что и в другом снимке (чтобы не перерисовывать интерфейс зря).
    pub fn same_games(&self, other: &Running, games: &[Game]) -> bool {
        games.iter().all(|g| self.pid_of(g) == other.pid_of(g))
    }
}

/// Запустить игру. Возвращает pid.
pub fn launch(game: &Game, args: &[String]) -> Result<u32> {
    let child = std::process::Command::new(game.exe_path())
        .args(args)
        .current_dir(&game.dir)
        .spawn()
        .with_context(|| format!("запуск {}", game.exe_path().display()))?;
    Ok(child.id())
}

pub fn exe_of(pid: u32) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        crate::win::process_path(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}
