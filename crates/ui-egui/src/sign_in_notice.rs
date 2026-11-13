// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account whose sign-in has run out (owner, 0.12.5: "Shouldn't the app notify of login
//! issues? Nothing looked wrong"). A Send/Receive nobody asked for never opens a sign-in page,
//! so one that found the sign-in gone used to end in a line of the log: headless there was
//! nothing at all to see, and the Outbox just stayed full. Now the account is kept as needing a
//! sign-in until one succeeds: the desktop says so in the status bar and beside the account's
//! name, `status` lists it for agents, and headless the owner is asked once, in the host's own
//! small window, whether to sign in now. Only the owner's click there opens the page; nothing
//! an agent asks for does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::agent_gate::OwnerQuestion;
use crate::{MailApp, approval};

/// How long the headless question stays up before it counts as Later: ten minutes.
pub(crate) const SECONDS: u64 = 600;

/// What the title bar's shield says of an account ([`MailApp::account_troubles`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// A sign-in waits for the owner on the provider's page.
    Waiting,
    /// The owner has to sign in again.
    SignIn,
    /// The last Send/Receive had problems.
    Problems,
}

/// Headless: the host's window asking about one account ([`tick`]).
pub(crate) struct Shown {
    account: Id,
    answer: Receiver<bool>,
    cancel: Arc<AtomicBool>,
}

/// Whether a sign-in nobody asked for ended with `error` because only a new sign-in on the
/// provider's page helps: the kept sign-in was refused, has run out, or there is none.
pub fn needs_new_sign_in(error: &str) -> bool {
    error == approval::GOOGLE_REAUTH_REQUIRED
        || error == approval::GOOGLE_NEEDS_SIGN_IN
        || (error.starts_with(approval::SIGN_IN_RAN_OUT_START) && error.ends_with(approval::SIGN_IN_RAN_OUT_END))
}

/// What the owner sees the account as: its address, or its name without one.
fn shown_name(app: &MailApp, account: Id) -> Option<String> {
    app.session.mailbox.account(account).map(|a| if a.email.trim().is_empty() { a.name.clone() } else { a.email.clone() })
}

impl MailApp {
    /// A sign-in nobody asked for ended with `error`: when that means the owner has to sign in
    /// again, the account is kept as needing it ([`needs_new_sign_in`]).
    pub(crate) fn note_sign_in_error(&mut self, account: Id, error: &str) {
        if needs_new_sign_in(error) {
            self.mark_sign_in_needed(account);
        }
    }

    /// `account` can't send or receive until the owner signs in again.
    pub(crate) fn mark_sign_in_needed(&mut self, account: Id) {
        if self.session.mailbox.account(account).is_some() && self.sign_in_needed.insert(account) {
            log::warn!("sign-in, account {account}: the owner has to sign in again");
        }
    }

    /// A sign-in of `account` succeeded: it needs none, and may be asked about again later.
    pub(crate) fn note_signed_in(&mut self, account: Id) {
        if self.sign_in_needed.remove(&account) {
            log::info!("sign-in, account {account}: signed in again");
        }
        self.sign_in_asked.remove(&account);
    }

    /// The accounts that can't send or receive until the owner signs in again, in order.
    pub fn sign_in_needed(&self) -> Vec<Id> {
        self.sign_in_needed.iter().copied().filter(|a| self.session.mailbox.account(*a).is_some()).collect()
    }

    /// What `status` says of them: the account and its address, for an agent to tell the owner.
    pub(crate) fn sign_in_needed_status(&self) -> Value {
        Value::Array(
            self.sign_in_needed()
                .into_iter()
                .map(|account| json!({"account": account, "address": shown_name(self, account).unwrap_or_default()}))
                .collect(),
        )
    }

    /// What `compose.send` answers, with why the message waits in the Outbox when its account
    /// needs the owner to sign in again (`waits`): sent, as far as MailCraft goes, but not gone.
    pub(crate) fn note_waiting_send(&self, id: &str, r: Result<Value, String>) -> Result<Value, String> {
        let Ok(mut v) = r else { return r };
        if id == "compose.send"
            && v.is_object()
            && let Some(account) = v.get("id").and_then(Value::as_u64).and_then(|m| self.session.mailbox.message(m)).map(|m| m.account)
            && self.sign_in_needed.contains(&account)
        {
            let name = shown_name(self, account).unwrap_or_default();
            v["waits"] = json!(crate::i18n::fmt(
                tl!("It waits in the Outbox: {account} needs you to sign in again before anything is sent."),
                &[("account", &name)]
            ));
        }
        Ok(v)
    }

    /// What needs the owner, account by account, for the title bar's shield: a sign-in waiting
    /// on the provider's page first, then the accounts to sign in again, then those whose last
    /// Send/Receive had problems; one line each.
    pub(crate) fn account_troubles(&self) -> Vec<(Id, String, Trouble)> {
        let mut out: Vec<(Id, String, Trouble)> = Vec::new();
        let add = |account: Id, trouble: Trouble, out: &mut Vec<(Id, String, Trouble)>| {
            if !out.iter().any(|(a, ..)| *a == account)
                && let Some(name) = shown_name(self, account)
            {
                out.push((account, name, trouble));
            }
        };
        if let Some(waiting) = self.signing_in.as_ref().filter(|s| !s.quiet) {
            add(waiting.account, Trouble::Waiting, &mut out);
        }
        for account in self.sign_in_needed() {
            add(account, Trouble::SignIn, &mut out);
        }
        for (account, ..) in &self.sync_problems {
            add(*account, Trouble::Problems, &mut out);
        }
        out
    }

    /// Whether a Send/Receive or a sign-in is under way, or one waits its turn.
    fn busy_for_sign_in(&self, account: Id) -> bool {
        self.sync_rx.is_some()
            || self.graph_rx.is_some()
            || self.signing_in.is_some()
            || !self.sync_queue.is_empty()
            || self.calendar_job_for(account)
    }
}

/// The text of the headless question about `name`.
pub(crate) fn question(name: &str, cancel: Arc<AtomicBool>) -> OwnerQuestion {
    OwnerQuestion {
        title: tl!("Sign In Needed").to_string(),
        text: crate::i18n::fmt(tl!("{account} needs you to sign in again. Until you do, its mail isn't sent or received."), &[("account", name)]),
        note: tl!("Sign In opens the sign-in page in your browser.").to_string(),
        yes: tl!("Sign In").to_string(),
        no: tl!("Later").to_string(),
        seconds: SECONDS,
        cancel,
        preview: None,
    }
}

/// Headless, every frame: the owner's answer to the window that is up, the Send/Receive that
/// signs in once the owner said Sign In, and the next account to ask about, once each until it
/// is signed in again. Never over an agent's question.
pub(crate) fn tick(app: &mut MailApp, ctx: &egui::Context) {
    if let Some(shown) = app.sign_in_window.take() {
        match shown.answer.try_recv() {
            Ok(true) => {
                log::info!("sign-in, account {}: the owner said Sign In", shown.account);
                app.owner_sign_in = Some(shown.account);
            }
            Ok(false) | Err(TryRecvError::Disconnected) => log::info!("sign-in, account {}: the owner said Later", shown.account),
            Err(TryRecvError::Empty) if app.sign_in_needed.contains(&shown.account) => app.sign_in_window = Some(shown),
            // Signed in meanwhile (an agent's Send/Receive found a good sign-in after all).
            Err(TryRecvError::Empty) => shown.cancel.store(true, Ordering::Relaxed),
        }
    }
    if let Some(account) = app.owner_sign_in {
        if app.session.mailbox.account(account).is_none() {
            app.owner_sign_in = None;
        } else if app.busy_for_sign_in(account) {
            // Once what runs now has finished.
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        } else {
            match app.start_sync(&json!({"account": account})) {
                // The system's store is asked first; the run goes on, as the owner's, when it answers.
                Ok(v) if v.get("pending").and_then(Value::as_str) == Some("secrets") => {}
                Ok(_) => app.owner_sign_in = None,
                Err(e) => {
                    app.owner_sign_in = None;
                    log::warn!("sign-in, account {account}: the owner's Send/Receive didn't start");
                    app.status(e);
                }
            }
        }
    }
    if app.sign_in_window.is_some() || app.owner_window.is_some() || app.owner_sign_in.is_some() || !app.asking.is_empty() {
        return;
    }
    let Some(account) = app.sign_in_needed().into_iter().find(|a| !app.sign_in_asked.contains(a)) else { return };
    let Some(name) = shown_name(app, account) else { return };
    let Some(ask) = &app.services.ask_owner else { return };
    let cancel = Arc::new(AtomicBool::new(false));
    let answer = ask(question(&name, cancel.clone()));
    app.sign_in_asked.insert(account);
    app.sign_in_window = Some(Shown { account, answer, cancel });
}
