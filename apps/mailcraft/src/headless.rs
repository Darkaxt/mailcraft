// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The single mail owner without a native window, event loop, renderer or GPU.
//! Existing mailbox services wake it for work; their requested schedules govern
//! waiting. A scheduled wake never cancels work or changes its outcome.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use mailcraft_ui_egui::MailApp;

use crate::{control_server, launch, single_instance, startup};

#[derive(Default)]
struct Wake {
    next: Mutex<Option<Instant>>,
    changed: Condvar,
}

impl Wake {
    fn request(&self, at: Instant) {
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        *next = Some(next.map_or(at, |old| old.min(at)));
        self.changed.notify_one();
    }

    fn wait(&self, delay: Duration) {
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = Instant::now().checked_add(delay) {
            *next = Some(next.map_or(at, |old| old.min(at)));
        }
        loop {
            match *next {
                Some(at) => {
                    let Some(left) = at.checked_duration_since(Instant::now()) else {
                        *next = None;
                        return;
                    };
                    let (guard, _) = self.changed.wait_timeout(next, left).unwrap_or_else(|e| e.into_inner());
                    next = guard;
                }
                None => next = self.changed.wait(next).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }
}

fn context() -> (egui::Context, Arc<Wake>) {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    let wake = Arc::new(Wake::default());
    let requested = wake.clone();
    ctx.set_request_repaint_callback(move |request| {
        if let Some(at) = Instant::now().checked_add(request.delay) {
            requested.request(at);
        }
    });
    (ctx, wake)
}

pub(crate) fn run(
    startup: startup::Startup,
    guard: Option<single_instance::Guard>,
    mailbox: Option<PathBuf>,
    port: Option<u16>,
) -> Result<(), String> {
    let port = port.ok_or("headless mode needs an authenticated control port")?;
    let (ctx, wakes) = context();
    let progress = std::sync::Arc::new(mailcraft_engine::io::OpenProgress::default());
    let reading = progress.clone();
    let wake = ctx.clone();
    let sample = startup.sample;
    let arriving = launch::start(move || launch::first_mailbox(sample, mailbox, &reading, true), move || wake.request_repaint());
    let prefs = startup.prefs.clone();
    let initial_options = prefs.as_ref().map_or_else(|| mailcraft_ui_egui::UiState::default().options, |ui| ui.options.clone());
    let prefs_path = crate::prefs_path();
    let build = startup.builder(ctx.clone(), None);
    let mut app = MailApp::opening(mailcraft_ui_egui::opening::Opening::new(arriving, progress, build), prefs, crate::system_locale::system_locale());
    app.start_headless();
    let (requests, key_file) = control_server::start(port, ctx.clone(), mailcraft_control_key::settings_dir().as_deref(), true)
        .ok_or("headless control endpoint could not start; no second mailbox owner was created")?;
    let key_file = key_file.ok_or("headless control key could not be written; no requests were admitted")?;
    app = app.with_control(requests);
    if let Some(guard) = &guard {
        guard.wake(&ctx);
    }
    log::info!("headless mail runtime ready; no native window or graphics initialized");
    // The manual launcher waits for this actual endpoint-ready event rather than
    // a GUI idle hint, a guessed delay, or changing the selected process identity.
    {
        use std::io::Write;
        let ready = serde_json::json!({"state": "ready", "mode": "headless", "pid": std::process::id(), "controlPort": port});
        if let Err(why) = writeln!(std::io::stdout().lock(), "{ready}") {
            log::warn!("headless startup notification was not delivered; the native runtime remains available: {why}");
        }
    }
    pump(&mut app, &ctx, wakes, guard.as_ref(), prefs_path.as_deref().map(|path| (path, &initial_options)));
    key_file.remove();
    if let Some(guard) = &guard {
        guard.close();
    }
    Ok(())
}

/// Persist changed engine options without writing the windowless host's layout
/// or navigation over the desktop's settings. Unknown JSON fields stay intact.
fn save_options_at(app: &MailApp, initial: &mailcraft_engine::Prefs, path: &Path) -> Result<(), String> {
    if !crate::prefs_enabled() || app.data_removed || app.is_opening() || &app.session.prefs == initial {
        return Ok(());
    }
    if crate::PREFS_LEFT_IN_PLACE.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("the unreadable settings were preserved; changed preferences could not be saved".into());
    }
    let save = || -> std::io::Result<()> {
        let initial = serde_json::to_value(initial).map_err(std::io::Error::other)?;
        let current = serde_json::to_value(&app.session.prefs).map_err(std::io::Error::other)?;
        let mut saved = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)?,
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => serde_json::json!({"options": initial.clone()}),
            Err(why) => return Err(why),
        };
        let root = saved.as_object_mut().ok_or_else(|| std::io::Error::other("settings are not an object"))?;
        let options = root.entry("options").or_insert_with(|| serde_json::json!({}));
        let options = options.as_object_mut().ok_or_else(|| std::io::Error::other("saved options are not an object"))?;
        if let (Some(before), Some(after)) = (initial.as_object(), current.as_object()) {
            for key in before.keys().filter(|key| !after.contains_key(*key)) {
                options.remove(key);
            }
            for (key, value) in after.iter().filter(|(key, value)| before.get(*key) != Some(*value)) {
                options.insert(key.clone(), value.clone());
            }
        }
        if let Some(dir) = path.parent() {
            mailcraft_engine::store::create_private_dir_all(dir)?;
        }
        mailcraft_engine::store::write_atomic(path, &serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other)?)
    };
    save().map_err(|why| format!("headless preferences could not be saved ({:?}); settings were left in place", why.kind()))
}

fn pump(
    app: &mut MailApp,
    ctx: &egui::Context,
    wakes: Arc<Wake>,
    guard: Option<&single_instance::Guard>,
    preferences: Option<(&Path, &mailcraft_engine::Prefs)>,
) {
    // The first logic pass follows the interface language for unchosen clock
    // and week settings. That initialization is not an explicit preference edit.
    let preferences = preferences.map(|(path, initial)| {
        let mut initial = initial.clone();
        initial.follow_language(app.language());
        (path, initial)
    });
    let started = Instant::now();
    loop {
        // The context supplies the existing command/job scheduler; no UI is laid
        // out, tessellated, painted or submitted to a graphics device.
        ctx.begin_pass(egui::RawInput { time: Some(started.elapsed().as_secs_f64()), ..Default::default() });
        if let Some(guard) = guard
            && let Some(handed) = guard.handed_over()
        {
            log::warn!(
                "headless runtime cannot open desktop files or links ({} received, {} refused); use the desktop manually",
                handed.open.len(),
                handed.refused
            );
        }
        app.logic(ctx);
        if let Some(guard) = guard
            && !app.is_opening()
        {
            guard.follow(app.session.path.as_deref());
        }
        let mut output = ctx.end_pass();
        let delay = output.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        output.textures_delta.clear();
        if app.headless_close_ready() {
            let saved = app.save_on_exit().and_then(|()| preferences.as_ref().map_or(Ok(()), |(path, initial)| save_options_at(app, initial, path)));
            match saved {
                Ok(()) => break,
                Err(why) => app.headless_save_failed(&why),
            }
        }
        wakes.wait(delay);
    }
}

#[cfg(test)]
mod headless_tests;
#[cfg(all(test, windows))]
mod index_native_tests;
#[cfg(test)]
mod opening_retention_tests;
#[cfg(test)]
mod preferences_tests;
