//! Окно установщика, обновлятора и удаления: страницы, кнопки и фоновая работа.

use crate::ops::{self, Options, Report, t};
use crate::{Lang, SetupWindow, StepItem, ToolItem, system, tools};
use anyhow::Result;
use modly_core::update::{self, Channel};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub enum Mode {
    /// Установщик: архив выпуска внутри exe. `dir` — папка из командной строки,
    /// `elevated` — перезапуск с правами администратора, сразу к установке с этими настройками.
    Install { payload: &'static [u8], dir: Option<PathBuf>, elevated: Option<String> },
    /// Удаление Modly из `dir`. `cleanup` — эта копия во временной папке, удалить её после выхода.
    Uninstall { dir: PathBuf, cleanup: Option<PathBuf> },
    /// Обновление Modly в `dir`: `apply` — поставить пакет, рядом с которым лежит этот обновлятор;
    /// без него — проверить, есть ли новая версия.
    Update { dir: PathBuf, apply: Option<Apply> },
}

pub struct Apply {
    pub from: String,
    pub pid: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Install,
    Uninstall,
    Update,
}

struct State {
    kind: Kind,
    payload: &'static [u8],
    steps: Rc<VecModel<StepItem>>,
    step_keys: Vec<(String, String)>,
    tools: Vec<tools::Tool>,
    selected_tools: HashSet<String>,
    busy: bool,
    dir: PathBuf,
    /// установка ждёт, пока закроют Modly
    pending: Option<Options>,
    current: String,
    found: Option<update::Update>,
    cleanup: Option<PathBuf>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

pub fn run(mode: Mode) -> i32 {
    select_renderer();
    let ui = match SetupWindow::new() {
        Ok(ui) => ui,
        Err(e) => return software_fallback(&format!("{e}")),
    };

    let english = modly_core::settings::Settings::load().effective_language() != "ru";
    set_language(&ui, english);

    let (kind, dir, payload, cleanup) = match &mode {
        Mode::Install { payload, dir, .. } => {
            let installed = system::installed();
            let dir = dir.clone().or_else(|| installed.as_ref().map(|(d, _)| d.clone())).unwrap_or_else(system::default_install_dir);
            (Kind::Install, dir, *payload, None)
        }
        Mode::Uninstall { dir, cleanup } => (Kind::Uninstall, dir.clone(), &[][..], cleanup.clone()),
        Mode::Update { dir, .. } => (Kind::Update, dir.clone(), &[][..], None),
    };
    let current = match &mode {
        Mode::Update { apply: Some(a), .. } => a.from.clone(),
        _ => ops::InstallInfo::load(&dir).map(|i| i.version).filter(|v| !v.is_empty()).unwrap_or_else(|| VERSION.to_owned()),
    };

    let steps = Rc::new(VecModel::default());
    ui.set_steps(ModelRc::from(steps.clone()));
    ui.set_mode(match kind {
        Kind::Install => "install",
        Kind::Uninstall => "uninstall",
        Kind::Update => "update",
    }
    .into());
    ui.set_install_dir(dir.display().to_string().into());
    ui.set_data_dir(modly_core::paths::data_dir().display().to_string().into());
    ui.set_license_text(include_str!("../../../LICENSE").into());
    ui.set_version(update::label(VERSION).into());

    let tools = if kind == Kind::Install { tools::detect() } else { Vec::new() };
    ui.set_winget(kind != Kind::Install || tools.iter().all(|t| t.present) || tools::has_winget());

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            kind,
            payload,
            steps,
            step_keys: Vec::new(),
            tools,
            selected_tools: HashSet::new(),
            busy: false,
            dir: dir.clone(),
            pending: None,
            current: current.clone(),
            found: None,
            cleanup,
        })
    });

    if kind == Kind::Install {
        let installed = system::installed();
        if let Some((_, v)) = &installed {
            ui.set_installed(true);
            ui.set_from_version(update::label(v).into());
            ui.set_downgrade(update::Version::parse(v).zip(update::Version::parse(VERSION)).is_some_and(|(a, b)| a > b));
        }
        refresh_tools(&ui);
        check_dir(&ui);
    } else {
        ui.set_from_version(update::label(&current).into());
    }

    wire(&ui);

    // Сразу к делу: перезапуск с правами администратора, установка обновления, проверка.
    match mode {
        Mode::Install { elevated: Some(opts), .. } => {
            apply_option_string(&ui, &opts);
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(200), move || {
                if let Some(ui) = weak.upgrade() {
                    start(&ui);
                }
            });
        }
        Mode::Update { apply: Some(a), .. } => {
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(150), move || {
                if let Some(ui) = weak.upgrade() {
                    start_apply(&ui, a.pid);
                }
            });
        }
        Mode::Update { apply: None, .. } => {
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(400), move || {
                if let Some(ui) = weak.upgrade() {
                    start_check(&ui);
                }
            });
        }
        _ => {}
    }

    if let Err(e) = ui.show() {
        return software_fallback(&format!("{e}"));
    }
    style(&ui);
    let _ = slint::run_event_loop();
    let cleanup = with(|s| s.cleanup.clone()).flatten();
    drop(ui);
    if let Some(dir) = cleanup {
        system::delete_after_exit(&[dir]);
    }
    0
}

/// Рисуем видеокартой (градиенты, свечение); без OpenGL — программно.
fn select_renderer() {
    if std::env::var_os("SLINT_BACKEND").is_some() {
        return;
    }
    let gl = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("femtovg".into()).select();
    if let Err(e) = gl {
        log::warn!("femtovg: {e}");
        let _ = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).select();
    }
}

/// OpenGL не завёлся: перезапуск с программной отрисовкой.
fn software_fallback(err: &str) -> i32 {
    log::warn!("window failed: {err}");
    if std::env::var_os("SLINT_BACKEND").is_some() {
        return 1;
    }
    let Ok(exe) = std::env::current_exe() else { return 1 };
    match std::process::Command::new(exe).args(std::env::args().skip(1)).env("SLINT_BACKEND", "winit-software").status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(_) => 1,
    }
}

fn style(ui: &SetupWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = ui.window().window_handle().window_handle() {
        if let RawWindowHandle::Win32(w) = h.as_raw() {
            system::round_corners(w.hwnd.get());
        }
    }
}

fn set_language(ui: &SetupWindow, english: bool) {
    ops::set_english(english);
    ui.global::<Lang>().set_english(english);
}

fn wire(ui: &SetupWindow) {
    let weak = ui.as_weak();
    ui.on_drag(move || {
        if let Some(ui) = weak.upgrade() {
            use slint::winit_030::WinitWindowAccessor;
            ui.window().with_winit_window(|w| {
                let _ = w.drag_window();
            });
        }
    });
    let weak = ui.as_weak();
    ui.on_minimize(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_close_window(move || {
        if with(|s| s.busy).unwrap_or(false) {
            if let Some(ui) = weak.upgrade() {
                ui.set_status(t("Дождитесь окончания — прерывать опасно", "Please wait for it to finish — stopping now is unsafe").into());
            }
            return;
        }
        let _ = slint::quit_event_loop();
    });
    let weak = ui.as_weak();
    ui.on_set_english(move |e| {
        if let Some(ui) = weak.upgrade() {
            set_language(&ui, e);
            refresh_tools(&ui);
            refresh_step_titles();
            check_dir(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_browse(move || {
        if let Some(ui) = weak.upgrade() {
            browse(&ui);
        }
    });
    ui.on_toggle_tool(|id, on| {
        with(|s| {
            if on {
                s.selected_tools.insert(id.to_string());
            } else {
                s.selected_tools.remove(id.as_str());
            }
        });
    });
    let weak = ui.as_weak();
    ui.on_begin(move || {
        if let Some(ui) = weak.upgrade() {
            start(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_retry(move || {
        if let Some(ui) = weak.upgrade() {
            let kind = with(|s| s.kind).unwrap_or(Kind::Install);
            match kind {
                Kind::Install => ui.set_page("welcome".into()),
                Kind::Uninstall => ui.set_page("confirm".into()),
                Kind::Update => {
                    if with(|s| s.found.is_some()).unwrap_or(false) {
                        ui.set_page("available".into());
                    } else {
                        start_check(&ui);
                    }
                }
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_launch(move || {
        let dir = with(|s| s.dir.clone()).unwrap_or_default();
        if let Err(e) = system::launch(&dir.join(update::APP_EXE), &[]) {
            log::warn!("launch: {e:#}");
        }
        if weak.upgrade().is_some() {
            let _ = slint::quit_event_loop();
        }
    });
    ui.on_open_log(|| modly_core::win::shell_open(&crate::log_path().display().to_string()));
    ui.on_open_link(|l| modly_core::win::shell_open(&l));
    let weak = ui.as_weak();
    ui.on_close_modly(move || {
        let dir = with(|s| s.dir.clone()).unwrap_or_default();
        let weak = weak.clone();
        std::thread::spawn(move || {
            for pid in system::modly_pids(&dir) {
                system::close_process(pid);
            }
            let _ = weak.upgrade_in_event_loop(|ui| {
                ui.set_modly_running(false);
                let pending = with(|s| s.pending.take()).flatten();
                if let Some(opts) = pending {
                    run_install(&ui, opts);
                }
            });
        });
    });
}

// ------------------------------------------------------------------- шаги ---

fn set_steps(keys: Vec<(String, String)>) {
    with(|s| {
        s.step_keys = keys;
        s.steps.set_vec(Vec::new());
    });
    refresh_step_titles();
}

fn refresh_step_titles() {
    with(|s| {
        let en = ops::english();
        for (i, (ru, enk)) in s.step_keys.iter().enumerate() {
            let title: SharedString = if en { enk.into() } else { ru.into() };
            if i < s.steps.row_count() {
                let mut row = s.steps.row_data(i).unwrap();
                row.title = title;
                s.steps.set_row_data(i, row);
            } else {
                s.steps.push(StepItem { title, state: "pending".into() });
            }
        }
    });
}

fn key(ru: &str, en: &str) -> (String, String) {
    (ru.to_owned(), en.to_owned())
}

fn apply_report(ui: &SetupWindow, r: Report) {
    match r {
        Report::Step(i, state) => {
            with(|s| {
                if let Some(mut row) = s.steps.row_data(i) {
                    row.state = state.into();
                    s.steps.set_row_data(i, row);
                }
            });
        }
        Report::Progress(p) => {
            ui.set_indeterminate(false);
            ui.set_progress(p);
        }
        Report::Status(text) => ui.set_status(text.into()),
    }
}

/// Работа в фоне: отчёты идут в окно, по окончании — `done` на потоке окна.
fn background(ui: &SetupWindow, work: impl FnOnce(&(dyn Fn(Report) + Sync)) -> Result<()> + Send + 'static, done: impl FnOnce(&SetupWindow, Result<()>) + Send + 'static) {
    with(|s| s.busy = true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let w = weak.clone();
        let report = move |r: Report| {
            let _ = w.upgrade_in_event_loop(move |ui| apply_report(&ui, r));
        };
        let result = work(&report);
        if let Err(e) = &result {
            log::error!("{e:#}");
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            with(|s| s.busy = false);
            done(&ui, result);
        });
    });
}

fn fail(ui: &SetupWindow, e: &anyhow::Error) {
    with(|s| {
        for i in 0..s.steps.row_count() {
            let mut row = s.steps.row_data(i).unwrap();
            if row.state == "active" {
                row.state = "error".into();
                s.steps.set_row_data(i, row);
            }
        }
    });
    ui.set_error_text(format!("{e:#}").into());
    ui.set_page("error".into());
}

// --------------------------------------------------------------- установка ---

fn refresh_tools(ui: &SetupWindow) {
    let items: Vec<ToolItem> = with(|s| {
        s.tools
            .iter()
            .map(|tool| ToolItem {
                id: tool.id.into(),
                title: tool.title().into(),
                text: tool.text().into(),
                present: tool.present,
                selected: s.selected_tools.contains(tool.id),
            })
            .collect()
    })
    .unwrap_or_default();
    ui.set_tools(ModelRc::new(VecModel::from(items)));
}

fn check_dir(ui: &SetupWindow) {
    let dir = PathBuf::from(ui.get_install_dir().as_str());
    let probe = dir.ancestors().find(|p| p.exists()).map(Path::to_path_buf).unwrap_or_else(|| dir.clone());
    let writable = if dir.exists() { system::dir_writable(&dir) } else { system::dir_writable(&probe.join(".modly-probe-dir")) };
    ui.set_dir_warning(if writable || system::is_elevated() {
        SharedString::default()
    } else {
        t("Для этой папки нужны права администратора — Windows спросит разрешение.", "This folder needs administrator rights — Windows will ask for permission.").into()
    });
    ui.set_size_text(t("Нужно около 35 МБ. Для текущего пользователя, без прав администратора.", "About 35 MB. For the current user, no administrator rights needed.").into());
}

fn browse(ui: &SetupWindow) {
    let current = PathBuf::from(ui.get_install_dir().as_str());
    let start = current.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf).unwrap_or_default();
    let Some(mut picked) = rfd::FileDialog::new().set_title(t("Куда установить Modly", "Where to install Modly")).set_directory(start).pick_folder() else {
        return;
    };
    // выбрали «D:\Games» — ставим в «D:\Games\Modly», чтобы не смешать с чужими файлами
    let named_modly = picked.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Modly"));
    let empty = std::fs::read_dir(&picked).map(|mut d| d.next().is_none()).unwrap_or(true);
    if !named_modly && !empty {
        picked = picked.join("Modly");
    }
    ui.set_install_dir(picked.display().to_string().into());
    check_dir(ui);
}

fn options(ui: &SetupWindow) -> Options {
    Options {
        dir: PathBuf::from(ui.get_install_dir().as_str()),
        desktop: ui.get_desktop_shortcut(),
        start_menu: ui.get_start_menu(),
        protocol: ui.get_url_protocol(),
    }
}

/// Настройки для перезапуска с правами администратора: `desktop,start,protocol,tool:dotnet`.
fn option_string(ui: &SetupWindow) -> String {
    let mut parts = Vec::new();
    if ui.get_desktop_shortcut() {
        parts.push("desktop".to_owned());
    }
    if ui.get_start_menu() {
        parts.push("start".to_owned());
    }
    if ui.get_url_protocol() {
        parts.push("protocol".to_owned());
    }
    if ui.global::<Lang>().get_english() {
        parts.push("en".to_owned());
    }
    with(|s| parts.extend(s.selected_tools.iter().map(|t| format!("tool:{t}"))));
    if parts.is_empty() { "none".to_owned() } else { parts.join(",") }
}

fn apply_option_string(ui: &SetupWindow, opts: &str) {
    let has = |k: &str| opts.split(',').any(|p| p == k);
    ui.set_desktop_shortcut(has("desktop"));
    ui.set_start_menu(has("start"));
    ui.set_url_protocol(has("protocol"));
    if has("en") {
        set_language(ui, true);
    }
    with(|s| {
        s.selected_tools = opts.split(',').filter_map(|p| p.strip_prefix("tool:")).map(str::to_owned).collect();
    });
}

fn start(ui: &SetupWindow) {
    match with(|s| s.kind).unwrap_or(Kind::Install) {
        Kind::Install => start_install(ui),
        Kind::Uninstall => start_uninstall(ui),
        Kind::Update => start_download(ui),
    }
}

fn start_install(ui: &SetupWindow) {
    let opts = options(ui);
    if !system::dir_writable(&opts.dir) && !system::is_elevated() {
        let args = format!("--elevated \"{}\" --dir \"{}\"", option_string(ui), opts.dir.display());
        match system::relaunch_elevated(&args) {
            Ok(()) => {
                let _ = slint::quit_event_loop();
            }
            Err(e) => fail(ui, &e),
        }
        return;
    }
    with(|s| s.dir = opts.dir.clone());
    let mut keys = vec![
        key("Подготовка", "Preparing"),
        key("Файлы Modly", "Modly files"),
        key("Ярлыки", "Shortcuts"),
        key("Регистрация в Windows", "Registering with Windows"),
    ];
    let selected: Vec<String> = with(|s| s.tools.iter().filter(|t| !t.present && s.selected_tools.contains(t.id)).map(|t| t.id.to_owned()).collect())
        .unwrap_or_default();
    for id in &selected {
        keys.push(match id.as_str() {
            "dotnet" => key("Ставим .NET SDK 8", "Installing .NET SDK 8"),
            _ => key("Ставим C++ Build Tools", "Installing C++ Build Tools"),
        });
    }
    set_steps(keys);
    ui.set_progress(0.0);
    ui.set_indeterminate(false);
    ui.set_page("progress".into());
    if !system::modly_pids(&opts.dir).is_empty() {
        ui.set_status(t("Modly сейчас открыт — его нужно закрыть, чтобы продолжить", "Modly is running — it needs to close to continue").into());
        ui.set_modly_running(true);
        with(|s| s.pending = Some(opts));
        return;
    }
    run_install(ui, opts);
}

fn run_install(ui: &SetupWindow, opts: Options) {
    let payload = with(|s| s.payload).unwrap_or(&[]);
    let tools: Vec<String> = with(|s| s.tools.iter().filter(|t| !t.present && s.selected_tools.contains(t.id)).map(|t| t.id.to_owned()).collect())
        .unwrap_or_default();
    let dir = opts.dir.clone();
    let weak = ui.as_weak();
    background(
        ui,
        move |rep| {
            ops::install(payload, &opts, VERSION, rep)?;
            let mut failed = Vec::new();
            for (i, id) in tools.iter().enumerate() {
                let step = 4 + i;
                rep(Report::Step(step, "active"));
                rep(Report::Status(t("Скачиваем и ставим через winget — это может занять несколько минут…", "Downloading and installing with winget — this may take a few minutes…")));
                let _ = weak.upgrade_in_event_loop(|ui| ui.set_indeterminate(true));
                match tools::install(id) {
                    Ok(()) => rep(Report::Step(step, "done")),
                    Err(e) => {
                        log::warn!("tool {id}: {e:#}");
                        rep(Report::Step(step, "error"));
                        failed.push(id.clone());
                    }
                }
            }
            if !failed.is_empty() {
                log::warn!("tools failed: {failed:?}");
            }
            Ok(())
        },
        move |ui, result| match result {
            Ok(()) => {
                let failed_tools = with(|s| s.steps.iter().skip(4).any(|r| r.state == "error")).unwrap_or(false);
                let mut text = format!("{} · {}", update::label(VERSION), dir.display());
                if failed_tools {
                    text.push_str(&t(
                        "\nИнструмент мододела не поставился — его можно поставить позже, подробности в журнале.",
                        "\nA modder tool didn't install — you can install it later, see the log.",
                    ));
                }
                ui.set_done_text(text.into());
                ui.set_page("done".into());
            }
            Err(e) => fail(ui, &e),
        },
    );
}

// ----------------------------------------------------------------- удаление ---

fn start_uninstall(ui: &SetupWindow) {
    set_steps(vec![
        key("Ярлыки", "Shortcuts"),
        key("Регистрация в Windows", "Windows registration"),
        key("Файлы Modly", "Modly files"),
        key("Загрузки и данные", "Downloads and data"),
    ]);
    ui.set_progress(0.0);
    ui.set_page("progress".into());
    let dir = with(|s| s.dir.clone()).unwrap_or_default();
    let remove_data = ui.get_remove_data();
    let pids = system::modly_pids(&dir);
    background(
        ui,
        move |rep| {
            if !pids.is_empty() {
                rep(Report::Status(t("Закрываем Modly…", "Closing Modly…")));
                for pid in pids {
                    system::close_process(pid);
                }
            }
            ops::uninstall(&dir, remove_data, rep)
        },
        move |ui, result| match result {
            Ok(()) => {
                ui.set_done_text(t("Спасибо, что пользовались Modly. Вернуться можно в любой момент — mods.hackerstone.xyz", "Thanks for using Modly. Come back any time — mods.hackerstone.xyz").into());
                ui.set_page("done".into());
            }
            Err(e) => fail(ui, &e),
        },
    );
}

// --------------------------------------------------------------- обновление ---

fn channel() -> Channel {
    match modly_core::settings::Settings::load().update_channel.as_str() {
        "stable" => Channel::Stable,
        "beta" => Channel::Beta,
        _ => Channel::default_for(&with(|s| s.current.clone()).unwrap_or_default()),
    }
}

fn start_check(ui: &SetupWindow) {
    ui.set_page("check".into());
    let current = with(|s| s.current.clone()).unwrap_or_default();
    let ch = channel();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = update::check(&current, ch);
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok(None) => {
                ui.set_done_text(format!("Modly {} — {}", update::label(&current), t("самая новая версия.", "is the latest version.")).into());
                ui.set_page("uptodate".into());
            }
            Ok(Some(u)) => {
                ui.set_version(update::label(&u.manifest.version).into());
                let lang = if ops::english() { "en" } else { "ru" };
                ui.set_notes(u.manifest.notes(lang).into());
                with(|s| s.found = Some(u));
                ui.set_page("available".into());
            }
            Err(e) => fail(&ui, &e),
        });
    });
}

/// Скачать найденное обновление и передать его новому обновлятору из пакета.
fn start_download(ui: &SetupWindow) {
    let Some(found) = with(|s| s.found.clone()).flatten() else { return };
    let dir = with(|s| s.dir.clone()).unwrap_or_default();
    let current = with(|s| s.current.clone()).unwrap_or_default();
    ui.set_progress(0.0);
    ui.set_indeterminate(false);
    ui.set_status(t("Скачиваем обновление…", "Downloading the update…").into());
    ui.set_page("progress".into());
    background(
        ui,
        move |rep| {
            let target = update::updates_dir().join(&found.manifest.version);
            let zip = update::download(&found, &target, &mut |got, total| {
                rep(Report::Progress(got as f32 / total.max(1) as f32 * 0.9));
                rep(Report::Status(format!("{} {:.1} / {:.1} МБ", t("Скачиваем", "Downloading"), got as f64 / 1048576.0, total as f64 / 1048576.0)));
                Ok(())
            })?;
            rep(Report::Status(t("Проверяем и распаковываем…", "Verifying and unpacking…")));
            let payload = target.join("payload");
            let _ = std::fs::remove_dir_all(&payload);
            update::extract_package(&zip, &payload)?;
            rep(Report::Progress(1.0));
            let pid = system::modly_pids(&dir).first().copied();
            let mut args = vec!["--apply".to_owned(), "--target".to_owned(), dir.display().to_string(), "--from".to_owned(), current.clone()];
            if let Some(pid) = pid {
                args.extend(["--pid".to_owned(), pid.to_string()]);
            }
            let updater = payload.join(update::UPDATER_EXE);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            system::launch(&updater, &args)?;
            Ok(())
        },
        |ui, result| match result {
            Ok(()) => {
                let _ = slint::quit_event_loop();
            }
            Err(e) => fail(ui, &e),
        },
    );
}

/// Поставить пакет, рядом с которым лежит этот обновлятор.
fn start_apply(ui: &SetupWindow, pid: Option<u32>) {
    let dir = with(|s| s.dir.clone()).unwrap_or_default();
    let from = with(|s| s.current.clone()).unwrap_or_default();
    let src = modly_core::paths::exe_dir();
    set_steps(vec![
        key("Ждём, пока Modly закроется", "Waiting for Modly to close"),
        key("Заменяем файлы", "Replacing files"),
        key("Запускаем Modly", "Starting Modly"),
    ]);
    ui.set_progress(0.0);
    ui.set_indeterminate(true);
    ui.set_status(t("Ждём, пока Modly закроется…", "Waiting for Modly to close…").into());
    ui.set_page("progress".into());
    let weak = ui.as_weak();
    background(
        ui,
        move |rep| {
            rep(Report::Step(0, "active"));
            let mut waited = 0u32;
            loop {
                let mut running: Vec<u32> = system::modly_pids(&dir);
                if let Some(p) = pid {
                    if !system::wait_exit(p, 0) && !running.contains(&p) {
                        running.push(p);
                    }
                }
                if running.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
                waited += 500;
                if waited == 6000 {
                    let _ = weak.upgrade_in_event_loop(|ui| {
                        ui.set_modly_running(true);
                        ui.set_status(t("Modly ещё открыт — закройте его, чтобы обновить", "Modly is still open — close it to update").into());
                    });
                }
            }
            let _ = weak.upgrade_in_event_loop(|ui| ui.set_modly_running(false));
            rep(Report::Step(0, "done"));
            rep(Report::Progress(0.3));

            rep(Report::Step(1, "active"));
            rep(Report::Status(t("Заменяем файлы…", "Replacing files…")));
            ops::apply_update(&src, &dir, VERSION)?;
            rep(Report::Step(1, "done"));
            rep(Report::Progress(0.75));

            rep(Report::Step(2, "active"));
            rep(Report::Status(t("Запускаем Modly…", "Starting Modly…")));
            let exe = dir.join(update::APP_EXE);
            let mut child = system::launch(&exe, &["--updated-from", &from])?;
            // Новый Modly упал сразу — вернуть старую версию.
            for _ in 0..8 {
                std::thread::sleep(Duration::from_millis(500));
                if let Ok(Some(status)) = child.try_wait() {
                    if !status.success() {
                        log::error!("new Modly exited with {status}, rolling back");
                        ops::rollback(&dir)?;
                        let _ = system::launch(&exe, &[]);
                        anyhow::bail!("{}", t("новая версия не запустилась — вернули прежнюю", "the new version didn't start — the previous one is back"));
                    }
                    break;
                }
            }
            rep(Report::Step(2, "done"));
            rep(Report::Progress(1.0));
            Ok(())
        },
        move |ui, result| match result {
            Ok(()) => {
                ui.set_done_text(format!("{} {}", t("Modly запускается — версия", "Modly is starting — version"), update::label(VERSION)).into());
                ui.set_page("done".into());
                slint::Timer::single_shot(Duration::from_millis(2600), || {
                    let _ = slint::quit_event_loop();
                });
            }
            Err(e) => fail(ui, &e),
        },
    );
}
