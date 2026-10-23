//! Обёртки над WinAPI, которые нужны Modly.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HANDLE, MAX_PATH};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Папка Steam из реестра (`HKCU\Software\Valve\Steam\SteamPath`).
pub fn steam_path() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if res.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]).replace('/', "\\"));
    path.is_dir().then_some(path)
}

/// Запущенные процессы: (pid, полный путь к exe). Процессы, к которым нет доступа, пропускаются.
/// `names` — имена exe в нижнем регистре: полный путь узнаётся только у них (так в разы быстрее).
pub fn processes(names: Option<&std::collections::HashSet<String>>) -> Vec<(u32, PathBuf)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let pid = entry.th32ProcessID;
                let wanted = names.is_none_or(|set| {
                    let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                    set.contains(&String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase())
                });
                if pid != 0 && wanted {
                    if let Some(path) = process_path(pid) {
                        out.push((pid, path));
                    }
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Полный путь к exe процесса.
pub fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize * 4];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..size as usize])))
    }
}

/// Открыть папку в Проводнике или ссылку в браузере.
pub fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Иконка exe в RGBA (`size`×`size`). `None`, если иконки нет.
pub fn exe_icon(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let path_w = wide(path.as_os_str());
    let mut name = [0u16; 260];
    name[..path_w.len().min(259)].copy_from_slice(&path_w[..path_w.len().min(259)]);
    let mut icons = [HICON::default(); 1];
    let mut id = [0u32; 1];
    let got = unsafe { PrivateExtractIconsW(&name, 0, size as i32, size as i32, Some(&mut icons), Some(id.as_mut_ptr()), 0) };
    if got == 0 || got == u32::MAX || icons[0].is_invalid() {
        return None;
    }
    let icon = icons[0];
    let result = unsafe { icon_to_rgba(icon) };
    unsafe {
        let _ = DestroyIcon(icon);
    }
    result
}

unsafe fn icon_to_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let color = info.hbmColor;
        let mask = info.hbmMask;
        let cleanup = || {
            if !color.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(color.0));
            }
            if !mask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(mask.0));
            }
        };
        if color.is_invalid() {
            cleanup();
            return None;
        }
        let dc = GetDC(None);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, ..Default::default() },
            ..Default::default()
        };
        // первый вызов — узнать размер
        if GetDIBits(dc, color, 0, 0, None, &mut bmi, DIB_RGB_COLORS) == 0 {
            ReleaseDC(None, dc);
            cleanup();
            return None;
        }
        let w = bmi.bmiHeader.biWidth.unsigned_abs();
        let h = bmi.bmiHeader.biHeight.unsigned_abs();
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;
        bmi.bmiHeader.biHeight = -(h as i32); // сверху вниз
        bmi.bmiHeader.biSizeImage = 0;
        let mut px = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(dc, color, 0, h, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        cleanup();
        if lines == 0 {
            return None;
        }
        let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2); // BGRA → RGBA
            if !has_alpha {
                p[3] = 255;
            }
        }
        Some((w, h, px))
    }
}

/// Главный цвет картинки: среднее по насыщенным непрозрачным пикселям (для обложки игры).
pub fn accent_color(rgba: &[u8]) -> Option<(u8, u8, u8)> {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in rgba.chunks_exact(4) {
        if p[3] < 200 {
            continue;
        }
        let max = p[0].max(p[1]).max(p[2]) as i32;
        let min = p[0].min(p[1]).min(p[2]) as i32;
        if max < 50 || max - min < 40 {
            continue; // слишком тёмный или серый
        }
        let wgt = (max - min) as u64;
        r += p[0] as u64 * wgt;
        g += p[1] as u64 * wgt;
        b += p[2] as u64 * wgt;
        n += wgt;
    }
    (n > 0).then(|| ((r / n) as u8, (g / n) as u8, (b / n) as u8))
}

/// Тёмный заголовок окна Windows 11 в цвет приложения. `caption` и `text` — RGB.
pub fn style_window(hwnd: isize, caption: (u8, u8, u8), border: (u8, u8, u8), text: (u8, u8, u8)) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };
    let hwnd = HWND(hwnd as *mut _);
    let colorref = |(r, g, b): (u8, u8, u8)| u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
    unsafe {
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&dark as *const i32).cast(), 4);
        for (attr, rgb) in [(DWMWA_CAPTION_COLOR, caption), (DWMWA_BORDER_COLOR, border), (DWMWA_TEXT_COLOR, text)] {
            let c = colorref(rgb);
            let _ = DwmSetWindowAttribute(hwnd, attr, (&c as *const u32).cast(), 4);
        }
    }
}

/// Текущее местное время «ЧЧ:ММ:СС».
pub fn local_time_hms() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

/// Положить текст в буфер обмена.
pub fn set_clipboard(text: &str) -> bool {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = (|| -> Option<()> {
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))).ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

/// Вывести на передний план окно с таким заголовком (второй запуск Modly показывает первый).
pub fn focus_window(title: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow};
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR::null(), &HSTRING::from(title)) {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Один Modly на пользователя: именованный мьютекс. `false` — Modly уже запущен.
pub fn acquire_single_instance() -> bool {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        match CreateMutexW(None, true, w!("Local\\Modly.SingleInstance")) {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(handle);
                    return false;
                }
                // хэндл не закрываем: мьютекс держится до конца процесса
                true
            }
            Err(_) => true,
        }
    }
}

/// Криптостойкие случайные байты (BCryptGenRandom).
pub fn random_bytes(buf: &mut [u8]) -> anyhow::Result<()> {
    use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let status = unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status.is_err() {
        anyhow::bail!("BCryptGenRandom: {:#x}", status.0);
    }
    Ok(())
}

/// Зашифровать данные ключом текущего пользователя Windows (DPAPI): расшифровать их сможет только
/// он и только на этом компьютере. Так хранится токен входа на сайт модов.
pub fn protect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    dpapi(data, true)
}

pub fn unprotect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    dpapi(data, false)
}

fn dpapi(data: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    // Своя «соль»: чужая программа того же пользователя не расшифрует блоб, не зная её
    // (защита от случайного, а не целенаправленного чтения — DPAPI это и так даёт).
    const ENTROPY: &[u8] = b"Modly.mods.hackerstone.token.v1";
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if encrypt {
            CryptProtectData(&input, w!("Modly"), Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?;
        } else {
            CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?;
        }
        let result = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        Ok(result)
    }
}

/// Зарегистрировать ссылки `modly://` для текущего пользователя (HKCU, без прав администратора):
/// кнопка «Установить через Modly» на сайте открывает этот exe. Запись обновляется, только если
/// путь к exe поменялся.
pub fn register_url_protocol(exe: &Path) -> anyhow::Result<()> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
    };
    let command = format!("\"{}\" \"%1\"", exe.display());
    let current = read_hkcu_string(r"Software\Classes\modly\shell\open\command", "");
    if current.as_deref() == Some(command.as_str()) {
        return Ok(());
    }
    let set = |sub: &str, name: &str, value: &str| -> anyhow::Result<()> {
        let mut key = HKEY::default();
        unsafe {
            let err = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(format!(r"Software\Classes\modly{sub}")),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            );
            if err != ERROR_SUCCESS {
                anyhow::bail!("RegCreateKeyExW: {}", err.0);
            }
            let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
            let err = RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(bytes));
            let _ = RegCloseKey(key);
            if err != ERROR_SUCCESS {
                anyhow::bail!("RegSetValueExW: {}", err.0);
            }
        }
        Ok(())
    };
    set("", "", "URL:Modly")?;
    set("", "URL Protocol", "")?;
    set(r"\DefaultIcon", "", &format!("\"{}\",0", exe.display()))?;
    set(r"\shell\open\command", "", &command)?;
    log::info!("modly:// registered → {}", exe.display());
    Ok(())
}

fn read_hkcu_string(sub: &str, name: &str) -> Option<String> {
    let mut buf = [0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let res = unsafe {
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
    if res.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Папка «Загрузки» пользователя (может быть перенесена на другой диск — спрашиваем Windows).
pub fn known_downloads() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as *const _));
        path.filter(|p| p.is_dir())
    }
}

/// Показать файл в Проводнике (с выделением).
pub fn reveal(path: &Path) {
    use std::os::windows::process::CommandExt;
    // Проводник ждёт именно `/select,"путь"` — обычное экранирование аргумента он не понимает.
    let p = path.display().to_string();
    if p.contains('"') {
        return;
    }
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{p}\"")).spawn();
}

/// Установлен ли WeMod (он регистрирует ссылки `wemod://` для текущего пользователя).
pub fn wemod_installed() -> bool {
    read_hkcu_string(r"Software\Classes\wemod\shell\open\command", "").is_some_and(|c| !c.trim().is_empty())
}

/// Прямоугольник на экране (физические пиксели).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// Окно на переднем плане: (hwnd, pid процесса, прямоугольник окна).
pub fn foreground_window() -> Option<(isize, u32, Rect)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        Some((hwnd.0 as isize, pid, Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }))
    }
}

/// Рабочая область (без панели задач) монитора, на котором окно `hwnd` (0 — главный монитор).
pub fn work_area(hwnd: isize) -> Rect {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow};
    unsafe {
        let flags = if hwnd == 0 { MONITOR_DEFAULTTOPRIMARY } else { MONITOR_DEFAULTTONEAREST };
        let mon = MonitorFromWindow(HWND(hwnd as _), flags);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            let r = info.rcWork;
            return Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
        }
        Rect { left: 0, top: 0, right: 1920, bottom: 1080 }
    }
}

/// Сделать окно оверлеем: поверх всех окон, без кнопки на панели задач и без перехвата фокуса —
/// игра остаётся активной (многие игры ставятся на паузу, когда теряют фокус), а клики по оверлею
/// всё равно доходят до него.
pub fn make_overlay(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW,
        SetWindowPos, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    unsafe {
        let h = HWND(hwnd as _);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}
