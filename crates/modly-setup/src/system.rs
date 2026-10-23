//! Windows: папки, ярлыки, запись в «Приложения и возможности», ссылки modly://, процессы и права.
//!
//! Установка — для текущего пользователя (`%LOCALAPPDATA%\Programs\Modly`, ключи в HKCU), поэтому
//! права администратора нужны только если выбрать папку вроде Program Files.
//! `MODLY_SETUP_SANDBOX=<папка>` — песочница для проверки установщика: ярлыки ложатся в неё, записи
//! реестра — в `HKCU\Software\ModlySandbox`, ссылки modly:// не трогаются.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegGetValueW, RegSetValueExW, RRF_RT_REG_SZ,
};
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, PWSTR};

pub fn sandbox() -> Option<PathBuf> {
    std::env::var_os("MODLY_SETUP_SANDBOX").map(PathBuf::from)
}

pub struct Places {
    pub start_menu: PathBuf,
    pub desktop: PathBuf,
    /// `%LOCALAPPDATA%\Programs` — программы текущего пользователя.
    pub programs: PathBuf,
}

pub fn places() -> Places {
    if let Some(sb) = sandbox() {
        return Places { start_menu: sb.join("StartMenu"), desktop: sb.join("Desktop"), programs: sb.join("Programs") };
    }
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_UserProgramFiles};
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    Places {
        start_menu: known_folder(&FOLDERID_Programs)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Start Menu\Programs")),
        desktop: known_folder(&FOLDERID_Desktop)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Desktop")),
        programs: known_folder(&FOLDERID_UserProgramFiles).unwrap_or_else(|| local.join("Programs")),
    }
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_CREATE, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_CREATE, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

pub fn default_install_dir() -> PathBuf {
    places().programs.join("Modly")
}

/// Ярлыки Modly: в меню «Пуск» и на рабочем столе.
pub fn shortcut_paths() -> (PathBuf, PathBuf) {
    let p = places();
    (p.start_menu.join("Modly.lnk"), p.desktop.join("Modly.lnk"))
}

// ------------------------------------------------------------------ ярлыки ---

pub fn create_shortcut(lnk: &Path, target: &Path, args: &str, description: &str) -> Result<()> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    if let Some(dir) = lnk.parent() {
        std::fs::create_dir_all(dir)?;
    }
    unsafe {
        // уже инициализирован в другом режиме — не страшно, ярлыку это не мешает
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).context("ShellLink")?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetArguments(&HSTRING::from(args))?;
        link.SetDescription(&HSTRING::from(description))?;
        if let Some(dir) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true).with_context(|| format!("{}", lnk.display()))?;
    }
    Ok(())
}

// ------------------------------------------------------------------ реестр ---

fn uninstall_key() -> &'static str {
    if sandbox().is_some() { r"Software\ModlySandbox\Uninstall\Modly" } else { r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Modly" }
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(sub: &str) -> Result<Key> {
    let mut key = HKEY::default();
    let err = unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(sub), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE | KEY_READ, None, &mut key, None)
    };
    if err.is_err() {
        bail!("реестр {sub}: {}", err.0);
    }
    Ok(Key(key))
}

fn set_sz(key: &Key, name: &str, value: &str) -> Result<()> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
    let err = unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, REG_SZ, Some(bytes)) };
    if err.is_err() {
        bail!("реестр {name}: {}", err.0);
    }
    Ok(())
}

fn set_dword(key: &Key, name: &str, value: u32) -> Result<()> {
    let err = unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())) };
    if err.is_err() {
        bail!("реестр {name}: {}", err.0);
    }
    Ok(())
}

pub fn get_sz(sub: &str, name: &str) -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(sub),
            &HSTRING::from(name),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

fn delete_tree(sub: &str) {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(sub));
    }
}

/// Запись в «Приложения и возможности»: версия, значок, размер и команда удаления.
pub fn write_uninstall_entry(dir: &Path, version: &str, size_kb: u32) -> Result<()> {
    let key = create_key(uninstall_key())?;
    let exe = dir.join(modly_core::update::APP_EXE);
    let updater = dir.join(modly_core::update::UPDATER_EXE);
    set_sz(&key, "DisplayName", "Modly")?;
    set_sz(&key, "DisplayVersion", &modly_core::update::label(version))?;
    set_sz(&key, "ModlyVersion", version)?;
    set_sz(&key, "Publisher", "Meline / Hackerstone")?;
    set_sz(&key, "DisplayIcon", &format!("{},0", exe.display()))?;
    set_sz(&key, "InstallLocation", &dir.display().to_string())?;
    set_sz(&key, "UninstallString", &format!("\"{}\" --uninstall", updater.display()))?;
    set_sz(&key, "QuietUninstallString", &format!("\"{}\" --uninstall --quiet", updater.display()))?;
    set_sz(&key, "URLInfoAbout", "https://github.com/melineceo/Modly")?;
    set_sz(&key, "URLUpdateInfo", "https://github.com/melineceo/Modly/releases")?;
    set_sz(&key, "HelpLink", "https://mods.hackerstone.xyz/docs.php")?;
    set_sz(&key, "InstallDate", &today_compact())?;
    set_dword(&key, "EstimatedSize", size_kb)?;
    set_dword(&key, "NoModify", 1)?;
    set_dword(&key, "NoRepair", 1)?;
    Ok(())
}

/// Обновить версию в записи, если Modly стоит именно в `dir`.
pub fn update_uninstall_version(dir: &Path, version: &str, size_kb: u32) {
    if installed().is_some_and(|(d, _)| same_dir(&d, dir)) {
        if let Ok(key) = create_key(uninstall_key()) {
            let _ = set_sz(&key, "DisplayVersion", &modly_core::update::label(version));
            let _ = set_sz(&key, "ModlyVersion", version);
            let _ = set_dword(&key, "EstimatedSize", size_kb);
        }
    }
}

pub fn remove_uninstall_entry() {
    delete_tree(uninstall_key());
}

/// Куда и какая версия Modly установлена (по записи в реестре).
pub fn installed() -> Option<(PathBuf, String)> {
    let dir = PathBuf::from(get_sz(uninstall_key(), "InstallLocation")?);
    let version = get_sz(uninstall_key(), "ModlyVersion").unwrap_or_default();
    dir.join(modly_core::update::APP_EXE).is_file().then_some((dir, version))
}

pub fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.display().to_string().trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}

fn today_compact() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
}

// ----------------------------------------------------------- ссылки modly:// ---

pub fn register_protocol(exe: &Path) -> Result<()> {
    if sandbox().is_some() {
        return Ok(());
    }
    modly_core::win::register_url_protocol(exe)
}

/// Снять ссылки modly://, если они ведут в эту папку (другую копию Modly не трогаем).
pub fn unregister_protocol_if_in(dir: &Path) {
    if sandbox().is_some() {
        return;
    }
    let cmd = get_sz(r"Software\Classes\modly\shell\open\command", "").unwrap_or_default().to_lowercase();
    let dir = dir.display().to_string().to_lowercase();
    if !cmd.is_empty() && cmd.contains(&dir) {
        delete_tree(r"Software\Classes\modly");
    }
}

// ---------------------------------------------------------------- процессы ---

/// Запущенные modly.exe из папки `dir`.
pub fn modly_pids(dir: &Path) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if name.eq_ignore_ascii_case(modly_core::update::APP_EXE) {
                if let Some(path) = process_path(e.th32ProcessID) {
                    if path.parent().is_some_and(|p| same_dir(p, dir)) {
                        out.push(e.th32ProcessID);
                    }
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn process_path(pid: u32) -> Option<PathBuf> {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Дождаться, пока процесс завершится. `true` — завершился (или его уже нет).
pub fn wait_exit(pid: u32, timeout_ms: u32) -> bool {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
        let r = WaitForSingleObject(h, timeout_ms);
        let _ = CloseHandle(h);
        r == WAIT_OBJECT_0
    }
}

/// Закрыть Modly: сначала по-хорошему (как крестиком окна), потом — принудительно.
pub fn close_process(pid: u32) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut owner = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == lparam.0 as u32 {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(pid as isize));
    }
    if wait_exit(pid, 6000) {
        return true;
    }
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
    wait_exit(pid, 4000)
}

// -------------------------------------------------------------------- права ---

/// Можно ли писать в папку (создаём её, если нет, и пробуем записать файл).
pub fn dir_writable(dir: &Path) -> bool {
    let existed = dir.exists();
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".modly-write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    if !existed {
        let _ = std::fs::remove_dir(dir);
    }
    ok
}

pub fn is_elevated() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some((&mut e as *mut TOKEN_ELEVATION).cast()), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len)
            .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

/// Перезапустить себя с правами администратора (Windows спросит разрешение).
pub fn relaunch_elevated(args: &str) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let exe = std::env::current_exe()?;
    let r = unsafe { ShellExecuteW(None, &HSTRING::from("runas"), &HSTRING::from(exe.as_os_str()), &HSTRING::from(args), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as isize <= 32 {
        bail!("Windows не дала прав администратора");
    }
    Ok(())
}

/// Запущены из дерева процессов «упакованного» приложения (MSIX из `Program Files\WindowsApps`,
/// например из терминала приложения из Microsoft Store)? Windows прячет записи в реестр таких процессов
/// в песочницу приложения: Modly не появился бы в «Приложениях», а ссылки modly:// не заработали бы.
pub fn in_packaged_tree() -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let mut parent = std::collections::HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            parent.insert(e.th32ProcessID, e.th32ParentProcessID);
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    let mut pid = std::process::id();
    for _ in 0..16 {
        let Some(&p) = parent.get(&pid) else { break };
        if p == 0 || p == pid {
            break;
        }
        if process_path(p).is_some_and(|path| path.display().to_string().to_lowercase().contains("\\windowsapps\\")) {
            return true;
        }
        pid = p;
    }
    false
}

/// Перезапустить себя вне такого приложения: ярлык с нашими аргументами открывает Проводник,
/// а он запускает программу как обычную.
pub fn relaunch_outside(args: &[String]) -> Result<()> {
    let exe = std::env::current_exe()?;
    let line: Vec<String> = args.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("\"{a}\"") } else { a.clone() }).collect();
    let lnk = std::env::temp_dir().join(format!("modly-setup-{}.lnk", std::process::id()));
    create_shortcut(&lnk, &exe, &line.join(" "), "Modly")?;
    std::process::Command::new("explorer.exe").arg(&lnk).spawn().context("explorer")?;
    // Проводнику нужно время открыть ярлык
    std::thread::sleep(std::time::Duration::from_secs(4));
    let _ = std::fs::remove_file(&lnk);
    Ok(())
}

/// Запустить Modly. Из установщика с правами администратора — через Проводник, чтобы Modly
/// работал с обычными правами пользователя.
pub fn launch(exe: &Path, args: &[&str]) -> Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    if is_elevated() && args.is_empty() {
        return std::process::Command::new("explorer.exe").arg(exe).spawn().context("explorer");
    }
    std::process::Command::new(exe)
        .args(args)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .with_context(|| format!("{}", exe.display()))
}

/// Удалить файлы и папки, когда этот процесс завершится (для копии удаления во временной папке).
pub fn delete_after_exit(paths: &[PathBuf]) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = String::from("ping -n 3 127.0.0.1 >nul");
    for p in paths {
        let p = p.display().to_string();
        if p.contains('"') {
            continue;
        }
        cmd.push_str(&format!(" & rmdir /s /q \"{p}\" 2>nul & del /f /q \"{p}\" 2>nul"));
    }
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/d /c \"{cmd}\""))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

// -------------------------------------------------------------------- окно ---

/// Скруглённые углы окна без рамки (Windows 11).
pub fn round_corners(hwnd: isize) {
    use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    unsafe {
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(HWND(hwnd as *mut _), DWMWA_WINDOW_CORNER_PREFERENCE, (&pref as *const windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE).cast(), std::mem::size_of_val(&pref) as u32);
    }
}
