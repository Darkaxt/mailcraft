// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The single mail owner without a native window, event loop, renderer or GPU.
//! Existing mailbox services wake it for work; their requested schedules govern
//! waiting. A scheduled wake never cancels work or changes its outcome.

use std::path::PathBuf;
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
    pump(&mut app, &ctx, wakes, guard.as_ref());
    key_file.remove();
    if let Some(guard) = &guard {
        guard.close();
    }
    // Desktop layout/preferences are not changed by a windowless mail check.
    Ok(())
}

fn pump(app: &mut MailApp, ctx: &egui::Context, wakes: Arc<Wake>, guard: Option<&single_instance::Guard>) {
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
            match app.save_on_exit() {
                Ok(()) => break,
                Err(why) => app.headless_save_failed(&why),
            }
        }
        wakes.wait(delay);
    }
}

#[cfg(test)]
mod headless_tests;
