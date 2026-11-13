// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit app-owned hiding and an idle-only, acknowledged normal close.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use egui::{Context, ViewportCommand};
use serde_json::{Value, json};

use crate::MailApp;

/// How long an accepted close waits for its bridge to release before normal
/// application work resumes and the close is reported as cancelled.
pub(crate) const BRIDGE_RELEASE_WAIT: Duration = Duration::from_secs(30);

/// Native transport receipt for a close acknowledgement. It is not a permission
/// token and cannot be supplied by a JSON caller. A failed write keeps the app open.
#[derive(Clone, Default)]
pub struct CloseReceipt(Arc<AtomicU8>);

impl CloseReceipt {
    const DELIVERED: u8 = 1;
    const FAILED: u8 = 2;
    const RELEASED: u8 = 4;

    pub fn complete(&self, delivered: bool) {
        self.0.fetch_or(if delivered { Self::DELIVERED } else { Self::FAILED }, Ordering::Release);
    }

    pub fn delivery(&self) -> Option<bool> {
        let state = self.0.load(Ordering::Acquire);
        if state & Self::FAILED != 0 {
            Some(false)
        } else if state & Self::DELIVERED != 0 {
            Some(true)
        } else {
            None
        }
    }

    /// The accepting bridge's connection ended; a shared workflow can release
    /// while the GUI is still alive, then stop its bridge through normal lifecycle.
    pub fn release(&self) {
        self.0.fetch_or(Self::RELEASED, Ordering::Release);
    }

    pub fn outcome(&self) -> Option<bool> {
        match self.delivery() {
            Some(false) => Some(false),
            Some(true) if self.0.load(Ordering::Acquire) & Self::RELEASED != 0 => Some(true),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    headless: bool,
    native_visible: Option<bool>,
    native_focused: Option<bool>,
    hidden: bool,
    apply_hidden: bool,
    pending_close: Option<CloseReceipt>,
    /// When the pending close was accepted; bounds the wait for bridge release.
    pub(crate) pending_since: Option<Instant>,
    closing: bool,
    blocked: Option<&'static str>,
}

impl State {
    pub(crate) fn window_is_hidden(&self) -> bool {
        self.hidden || self.native_visible == Some(false)
    }

    pub(crate) fn inspect(&self) -> Value {
        json!({"hiddenRequested": self.hidden, "nativeVisible": self.native_visible, "nativeFocused": self.native_focused,
            "mode": if self.headless { "headless" } else { "desktop" },
            "nativeWindow": if self.headless { Some(false) } else { self.native_visible.map(|_| true) },
            "close": if self.pending_close.as_ref().is_some_and(|r| r.delivery() == Some(true)) { "awaitingBridgeRelease" } else if self.pending_close.is_some() { "awaitingReplyDelivery" } else if self.closing { "closing" } else { "idle" },
            "blocked": self.blocked})
    }
}

impl MailApp {
    /// App-owned service mode: no window, layout, fonts, graphics, or owner input.
    /// This is host state, never a JSON permission or a persisted UI preference.
    pub fn start_headless(&mut self) {
        self.lifecycle.headless = true;
        self.offer_setup = false;
        self.offer_graph_switch = false;
    }

    pub fn is_headless(&self) -> bool {
        self.lifecycle.headless
    }

    /// The same save/close decision as the desktop, without a native window event.
    pub fn headless_close_ready(&mut self) -> bool {
        if !self.is_headless() || !self.lifecycle.closing || !self.quit_requested {
            return false;
        }
        self.quit_requested = false;
        self.close_requested()
    }

    pub fn headless_save_failed(&mut self, why: &str) {
        self.lifecycle.closing = false;
        self.lifecycle.blocked = Some("changes could not be saved; the app stays open");
        // Unsaved work stays held; the next accepted headless close retries the save
        // (headless.rs) and reports it again if it still fails.
        self.close_anyway = true;
        self.quit_requested = false;
        self.status(why.to_string());
    }
    pub(crate) fn lifecycle_status(&self) -> Value {
        let mut status = self.lifecycle.inspect();
        let blocker = if self.lifecycle.pending_close.is_some() || self.lifecycle.closing {
            Some("the original close request is still pending")
        } else {
            self.lifecycle_blocker()
        };
        status["canClose"] = json!(blocker.is_none());
        status["openingContentPending"] = json!(self.opening_content_pending());
        status["closeBlocker"] = json!(blocker);
        status["interactionRequired"] = json!(!self.asking.is_empty() || self.dialog.is_some() || self.windows.asks());
        status["permissionsNeeded"] = json!(self.rules_permissions_needed());
        // Accounts whose mail can't be sent or received until the owner signs in again: what
        // waits in their Outbox stays there till then ([`crate::sign_in_notice`]).
        status["signInNeeded"] = self.sign_in_needed_status();
        status["interaction"] = match &self.dialog {
            Some(crate::dialogs::Dialog::Confirm { command, ask, .. }) => json!({"kind": "confirm", "command": command, "title": ask.title}),
            Some(dialog) => json!({"kind": dialog.name()}),
            None => Value::Null,
        };
        status
    }

    pub(crate) fn lifecycle_before_request(&mut self, method: &str) -> Result<(), &'static str> {
        let probe = matches!(method, "app.status" | "app.lifecycle" | "ui.inspect" | "mailbox.inspect" | "engine.commands");
        if self.lifecycle.closing && !probe {
            return Err("the app is closing; no new work was started");
        }
        if self.lifecycle.pending_close.is_some() && !probe {
            self.lifecycle.pending_close = None;
            self.lifecycle.pending_since = None;
            self.lifecycle.blocked = Some("another request resumed application work; the original close was cancelled");
        }
        Ok(())
    }

    /// Actual native-window diagnostics, supplied by the host on the UI thread.
    /// Unknown visibility stays unknown, rather than being inferred from a request.
    pub fn observe_native_window(&mut self, visible: Option<bool>, focused: bool) {
        self.lifecycle.native_visible = visible;
        self.lifecycle.native_focused = Some(focused);
    }

    /// The native host's explicit hidden startup; never persisted as a user setting.
    pub fn start_hidden(&mut self) {
        self.lifecycle.hidden = true;
        self.lifecycle.apply_hidden = true;
    }

    pub(crate) fn apply_background_visibility(&mut self, ctx: &Context) {
        if std::mem::take(&mut self.lifecycle.apply_hidden) {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
    }

    pub(crate) fn lifecycle_blocker(&self) -> Option<&'static str> {
        if (self.opening.is_some() && !(self.is_headless() && self.opening_needs_attention()))
            || self.reading_ahead.is_some()
            || self.after_save.is_some()
            || self.graph_switch_copy.is_some()
            || self.file_dialog.is_some()
        {
            Some("a mailbox operation is pending")
        } else if !self.asking.is_empty() || self.dialog.is_some() || self.windows.asks() {
            Some("an owner question or dialog is open")
        } else if self.session.all_composing().next().is_some() {
            Some("a message being written must be saved and closed first")
        } else if self.windows.has_open() || self.session.signature_edit.is_some() {
            Some("an editor or secondary window is open")
        } else if self.session.source_export_busy()
            || self.session.content_index_busy()
            || self.search_index_pane.inspection_running()
            || self.sync_rx.is_some()
            || self.graph_rx.is_some()
            || self.graph_pieces.is_some()
            || !self.sync_queue.is_empty()
            || !self.inbox_queue.is_empty()
            || !self.sync_after_secrets.is_empty()
            || self.fetching.busy()
            || self.server_searching.busy()
            || self.calendar_working()
            || self.tasks.busy()
            || self.rules.busy()
            || self.auto_replies.busy()
            || self.signing_in.is_some()
            || self.approving()
            || self.shared_check_next.is_some()
            || self.open_shared.waiting.is_some()
            || !self.pending_shots.is_empty()
        {
            Some("native synchronization, sign-in or another job is active or queued")
        } else if self.close_anyway && !self.is_headless() {
            // Headless close saves synchronously, retries once and reports a failure
            // without exiting, so a past failure does not refuse every later close.
            Some("changes could not be saved; resolve them before closing")
        } else {
            None
        }
    }

    pub(crate) fn lifecycle_action(&mut self, ctx: &Context, params: &Value, receipt: Option<&CloseReceipt>) -> Result<Value, String> {
        let fields = params.as_object().ok_or("lifecycle parameters must be an object")?;
        if fields.len() != 1 {
            return Err("only action is accepted; force and discard are not supported".into());
        }
        match params.get("action").and_then(Value::as_str) {
            Some("hide") => {
                if self.is_headless() {
                    return Ok(json!({"requested": "headless", "windowCreated": false, "focusRequested": false}));
                }
                if !self.asking.is_empty() || self.dialog.is_some() || self.windows.asks() {
                    return Err("an owner question or dialog is open; the app was not hidden".into());
                }
                if self.windows.has_open() {
                    return Err("close secondary windows before hiding the app".into());
                }
                self.start_hidden();
                self.apply_background_visibility(ctx);
                Ok(json!({"requested": "hidden", "focusRequested": false}))
            }
            Some("close") => {
                if self.lifecycle.pending_close.is_some() || self.lifecycle.closing {
                    return Err("the original close request is still pending; inspect its outcome".into());
                }
                if let Some(why) = self.lifecycle_blocker() {
                    return Err(why.into());
                }
                let receipt = receipt.ok_or("the native host cannot acknowledge a close; no close was requested")?;
                self.lifecycle.pending_close = Some(receipt.clone());
                self.lifecycle.pending_since = Some(Instant::now());
                self.lifecycle.blocked = None;
                Ok(json!({"requested": "close", "pending": "bridgeRelease", "forced": false}))
            }
            _ => Err("action is hide or close".into()),
        }
    }

    /// Returns true while accepted close owns the idle app. Polling is driven by
    /// the control transport's repaint and the saver's real completion signal.
    pub(crate) fn lifecycle_close_tick(&mut self, ctx: &Context) -> bool {
        if let Some(receipt) = &self.lifecycle.pending_close {
            let Some(delivered) = receipt.outcome() else {
                let since = *self.lifecycle.pending_since.get_or_insert_with(Instant::now);
                let waited = since.elapsed();
                if waited < BRIDGE_RELEASE_WAIT {
                    ctx.request_repaint_after(BRIDGE_RELEASE_WAIT - waited);
                    return true;
                }
                // The client never released: application work resumes; the close was not made.
                self.lifecycle.pending_close = None;
                self.lifecycle.pending_since = None;
                self.lifecycle.blocked = Some("the bridge did not release within 30 seconds; the close was cancelled and the app stays open");
                self.status("A close request timed out waiting for its client; MailCraft stays open.");
                return false;
            };
            self.lifecycle.pending_close = None;
            self.lifecycle.pending_since = None;
            let blocker = if delivered { self.lifecycle_blocker() } else { Some("close acknowledgement was not delivered; the app stays open") };
            if let Some(why) = blocker {
                self.lifecycle.blocked = Some(why);
                return false;
            }
            self.lifecycle.closing = true;
            self.quit_requested = true;
        }
        if self.lifecycle.closing {
            self.autosave_tick(ctx);
            if self.close_anyway && !self.is_headless() {
                self.lifecycle.closing = false;
                self.lifecycle.blocked = Some("the mailbox save failed; the app stays open");
                self.quit_requested = false;
            }
        }
        self.lifecycle.closing
    }
}
