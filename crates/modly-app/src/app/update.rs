//! Обновления Modly: проверка выпусков на GitHub, загрузка пакета и передача его обновлятору.
//!
//! Проверка — при запуске (через 8 секунд) и раз в 6 часов, если человек её не выключил. Пакет
//! обновления проверяется по подписанному манифесту (см. `modly_core::update`) и распаковывается
//! в `%LOCALAPPDATA%\Modly\updates\<версия>`. Ставит его обновлятор из самого пакета
//! (`modly-updater.exe --apply`, crates/modly-setup): он ждёт, пока Modly закроется, меняет файлы,
//! при сбое возвращает старые и запускает Modly снова с `--updated-from <версия>`.

use super::{App, VERSION, post, with};
use crate::Store;
use modly_core::update::{self, Channel, Update};
use modly_protocol::Tone;
use slint::ComponentHandle;
use std::time::Duration;

const PERIOD: Duration = Duration::from_secs(6 * 3600);
const FIRST_CHECK: Duration = Duration::from_secs(8);

#[derive(Default)]
pub(super) struct UpdateState {
    found: Option<Update>,
    busy: bool,
    timer: Option<slint::Timer>,
}

pub(super) fn wire(s: &Store) {
    s.on_update_check(|| with(|a| a.update_check(true)));
    s.on_update_install(|| with(|a| a.update_install()));
    s.on_update_open_page(|| with(|a| a.update_open_page()));
    s.on_set_update_beta(|v| with(move |a| a.set_update_beta(v)));
    s.on_set_update_auto(|v| with(move |a| a.set_update_auto(v)));
}

/// Обновлять можно только настоящую установку: не сборку из `target` при разработке
/// (кроме проверки с `MODLY_UPDATE_API`).
fn updatable() -> bool {
    std::env::var_os("MODLY_UPDATE_API").is_some()
        || !modly_core::paths::exe_dir().components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"))
}

impl App {
    pub(super) fn update_init(&mut self) {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--updated-from") {
            let from = args.get(i + 1).cloned().unwrap_or_default();
            let text = format!(
                "{} {} → {}",
                self.t("Modly обновлён:", "Modly updated:"),
                update::label(&from),
                update::label(VERSION)
            );
            let title = self.t("Готово", "Done");
            self.toast_for(Tone::Success, &title, &text, Duration::from_secs(8));
        }
        // Скачанные пакеты уже поставленных версий не нужны. Не сразу: обновлятор из пакета ещё
        // показывает «Готово» и держит свой exe.
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(20));
            let Some(current) = update::Version::parse(VERSION) else { return };
            if let Ok(dir) = std::fs::read_dir(update::updates_dir()) {
                for e in dir.flatten() {
                    let stale = e.file_name().to_str().and_then(update::Version::parse).is_none_or(|v| v <= current);
                    if stale {
                        let _ = std::fs::remove_dir_all(e.path());
                    }
                }
            }
        });
        self.update_settings_to_ui();
        if !updatable() {
            self.set_update_state("dev");
            return;
        }
        self.update_schedule();
    }

    fn update_schedule(&mut self) {
        self.upd.timer = None;
        if self.settings.no_update_check || !updatable() {
            return;
        }
        slint::Timer::single_shot(FIRST_CHECK, || with(|a| a.update_check(false)));
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, PERIOD, || with(|a| a.update_check(false)));
        self.upd.timer = Some(timer);
    }

    fn channel(&self) -> Channel {
        match self.settings.update_channel.as_str() {
            "stable" => Channel::Stable,
            "beta" => Channel::Beta,
            _ => Channel::default_for(VERSION),
        }
    }

    fn update_settings_to_ui(&self) {
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        s.set_update_beta(self.channel() == Channel::Beta);
        s.set_update_auto(!self.settings.no_update_check);
    }

    fn set_update_state(&self, state: &str) {
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_update_state(state.into());
        }
    }

    /// Проверить, нет ли новой версии. `manual` — нажали «Проверить»: тогда и ошибку показываем.
    pub(super) fn update_check(&mut self, manual: bool) {
        if self.upd.busy || !updatable() {
            return;
        }
        self.upd.busy = true;
        if manual || self.upd.found.is_none() {
            self.set_update_state("checking");
        }
        let channel = self.channel();
        std::thread::spawn(move || {
            let result = update::check(VERSION, channel);
            post(move |a| a.on_update_checked(result, manual));
        });
    }

    fn on_update_checked(&mut self, result: anyhow::Result<Option<Update>>, manual: bool) {
        self.upd.busy = false;
        let Some(ui) = self.ui() else { return };
        let s = ui.global::<Store>();
        s.set_update_checked(format!("{} {}", self.t("проверено в", "checked at"), &modly_core::win::local_time_hms()[..5]).into());
        match result {
            Ok(Some(u)) => {
                let fresh = self.upd.found.as_ref().is_none_or(|f| f.manifest.version != u.manifest.version);
                s.set_update_version(update::label(&u.manifest.version).into());
                s.set_update_notes(u.manifest.notes(self.lang()).into());
                s.set_update_state("available".into());
                if fresh && !manual {
                    let text = format!(
                        "{} {} — {}",
                        self.t("Доступна версия", "Version"),
                        update::label(&u.manifest.version),
                        self.t("обновить можно в настройках", "is available — update in Settings")
                    );
                    let title = self.t("Обновление Modly", "Modly update");
                    self.toast_for(Tone::Info, &title, &text, Duration::from_secs(10));
                }
                self.upd.found = Some(u);
            }
            Ok(None) => {
                self.upd.found = None;
                s.set_update_state("latest".into());
            }
            Err(e) => {
                log::info!("update check: {e:#}");
                if manual || self.upd.found.is_none() {
                    s.set_update_error(format!("{e:#}").into());
                    s.set_update_state(if manual { "error" } else { "idle" }.into());
                }
            }
        }
    }

    /// Скачать обновление и передать его обновлятору; Modly закроется и откроется уже новым.
    pub(super) fn update_install(&mut self) {
        let Some(found) = self.upd.found.clone() else { return };
        if self.upd.busy {
            return;
        }
        self.upd.busy = true;
        self.set_update_state("downloading");
        if let Some(ui) = self.ui() {
            ui.global::<Store>().set_update_progress(0.0);
        }
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let dir = update::updates_dir().join(&found.manifest.version);
                let mut last = -1i32;
                let zip = update::download(&found, &dir, &mut |got, total| {
                    let pct = (got * 100 / total.max(1)) as i32;
                    if pct != last {
                        last = pct;
                        let p = got as f32 / total.max(1) as f32;
                        post(move |a| {
                            if let Some(ui) = a.ui() {
                                ui.global::<Store>().set_update_progress(p);
                            }
                        });
                    }
                    Ok(())
                })?;
                let payload = dir.join("payload");
                let _ = std::fs::remove_dir_all(&payload);
                update::extract_package(&zip, &payload)?;
                let updater = payload.join(update::UPDATER_EXE);
                if !updater.is_file() {
                    anyhow::bail!("в пакете нет обновлятора");
                }
                std::process::Command::new(&updater)
                    .arg("--apply")
                    .arg("--target")
                    .arg(modly_core::paths::exe_dir())
                    .args(["--from", VERSION, "--pid", &std::process::id().to_string()])
                    .spawn()?;
                Ok(())
            })();
            post(move |a| {
                a.upd.busy = false;
                match result {
                    Ok(()) => {
                        log::info!("update handed to the updater — exiting");
                        let _ = slint::quit_event_loop();
                    }
                    Err(e) => {
                        log::warn!("update install: {e:#}");
                        if let Some(ui) = a.ui() {
                            let s = ui.global::<Store>();
                            s.set_update_error(format!("{e:#}").into());
                            s.set_update_state("error".into());
                        }
                    }
                }
            });
        });
    }

    fn update_open_page(&mut self) {
        let url = self.upd.found.as_ref().map(|u| u.page_url.clone()).unwrap_or_else(|| format!("https://github.com/{}/releases", update::REPO));
        modly_core::win::shell_open(&url);
    }

    fn set_update_beta(&mut self, on: bool) {
        self.settings.update_channel = if on { "beta" } else { "stable" }.to_owned();
        self.settings.save();
        self.update_settings_to_ui();
        self.upd.found = None;
        self.update_check(true);
    }

    fn set_update_auto(&mut self, on: bool) {
        self.settings.no_update_check = !on;
        self.settings.save();
        self.update_settings_to_ui();
        self.update_schedule();
    }
}
