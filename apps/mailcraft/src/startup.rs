// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One mailbox initialization path for the desktop and the windowless mail host.

use super::*;

pub(crate) struct Startup {
    pub sample: bool,
    pub files: Vec<String>,
    pub prefs: Option<UiState>,
    pub settings_unread: Option<SettingsUnread>,
    pub restored: Option<WindowGeometry>,
    pub updates: Option<HostUpdates>,
    pub update_start: mailcraft_ui_egui::updates::UpdateStart,
    pub claimer: Option<single_instance::Claimer>,
    pub hidden: bool,
    pub headless: bool,
}

impl Startup {
    pub fn builder(self, ctx: egui::Context, file_dialog: Option<file_dialogs::Hook>) -> mailcraft_ui_egui::opening::Build {
        let anchors = trust_anchors();
        Box::new(move |arrival| {
            let (mailbox, path, unavailable, unchecked, damaged) =
                (arrival.mailbox, arrival.path, arrival.unavailable, arrival.unchecked, arrival.damaged);
            let mut session = Session::new(mailbox);
            let kept = path.is_some();
            session.path = path;
            session.index_words = true;
            let kept_pictures = if kept && !self.sample && !self.headless && prefs_enabled() { keep_pictures() } else { None };
            let mut services = services(file_dialog, ctx.clone(), kept_pictures, self.updates.as_ref(), self.headless);
            if !self.headless {
                services.window_identity =
                    Some(mailcraft_ui_egui::windows::WindowIdentity { icon: app_icon().map(std::sync::Arc::new), app_id: Some(APP_ID.into()) });
            }
            services.update_start = self.update_start;
            services.claim_mailbox =
                self.claimer.map(|c| Box::new(move |path: &std::path::Path| c.claim(path)) as Box<dyn Fn(&std::path::Path) -> bool>);
            let mut app = MailApp::new(session, services);
            app.trust_anchors_arriving = Some(anchors);
            app.autosave = kept;
            if let Some(path) = &app.session.path {
                app.services.saver = background_saver(&ctx, path);
            }
            if !self.headless {
                app.index_words_later();
            }
            let later = match &app.session.path {
                Some(path) if unchecked => content_later(&ctx, path, !self.headless),
                _ => None,
            };
            let later = later.map(|(missing, bodies)| {
                app.check_parts_later(missing, unavailable, !damaged);
                app.take_bodies_later(bodies);
            });
            if damaged {
                app.status(mailcraft_ui_egui::damaged_mailbox_notice());
            } else if later.is_none() && unavailable > 0 {
                app.status(format!("{unavailable} bodies or attachments are missing from the mailbox's content folder, or damaged."));
            }
            let after = mailcraft_engine::cmd::file::after_opening(&mut app.session);
            if after.favorites_now_graph > 0 {
                app.status(mailcraft_ui_egui::favorites_sync::FAVORITES_NOW_GRAPH);
            }
            for notice in mailcraft_ui_egui::fetched_again_notices(after.unreadable, after.hollow) {
                app.status(notice);
            }
            app.offer_setup = kept && !self.sample && !self.headless;
            app.offer_graph_switch = kept && !self.sample && !self.headless;
            match self.prefs {
                Some(ui) => app.apply_prefs(ui),
                None => app.fresh_prefs(),
            }
            app.settings_unread = self.settings_unread;
            app.ui.window = self.restored;
            app.integrated_titlebar = cfg!(target_os = "macos");
            app.windows_titlebar = cfg!(target_os = "windows");
            if self.headless {
                app.start_headless();
            } else if self.hidden {
                app.start_hidden();
            }
            for file in self.files {
                app.open_from_system(&file);
            }
            app
        })
    }
}
