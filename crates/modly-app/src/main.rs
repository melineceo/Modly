//! Modly — менеджер модов и панель управления ими.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod app;
mod icons;
mod live;
mod logger;
mod thumbs;

use std::time::Duration;

fn main() {
    logger::init();
    log::info!("Modly {} starting", app::VERSION);

    #[cfg(windows)]
    if !modly_core::win::acquire_single_instance() {
        log::info!("another Modly is running — focusing it");
        // Ссылка modly:// с сайта при уже открытом Modly — передаём её ему.
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Some(url) = app::url_from_args(&args) {
            if !app::forward_to_running(&url) {
                log::warn!("could not pass {url} to the running Modly");
            }
        }
        modly_core::win::focus_window("Modly");
        return;
    }

    // Программный рендерер Slint: ~30 МБ памяти вместо ~100 у OpenGL (драйвер видеокарты тяжёлый),
    // а интерфейс в основном статичный и перерисовывается кусками. SLINT_BACKEND из окружения — главнее.
    if std::env::var_os("SLINT_BACKEND").is_none() {
        let renderer = if modly_core::settings::Settings::load().gpu_rendering { "femtovg" } else { "software" };
        if let Err(e) = slint::BackendSelector::new().backend_name("winit".into()).renderer_name(renderer.into()).select() {
            log::warn!("renderer {renderer}: {e}");
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("modly-io")
        .enable_all()
        .build()
        .expect("tokio runtime");

    let ui = AppWindow::new().expect("create window");
    app::init(&ui, rt.handle().clone());
    ui.show().expect("show window");

    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(30), move || {
        if let Some(ui) = weak.upgrade() {
            app::style_window(&ui);
            // Рамка окна после смены темы заголовка пересчитывается, а программный рендерер держит
            // буфер старого размера (внизу оставалась белая полоса) — «толкаем» размер туда и обратно.
            let size = ui.window().size();
            ui.window().set_size(slint::PhysicalSize::new(size.width, size.height + 1));
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(16), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.window().set_size(size);
                }
            });
        }
    });

    slint::run_event_loop().expect("event loop");
    log::info!("Modly exiting");
    rt.shutdown_timeout(Duration::from_millis(500));
}
