// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules: what to do with arriving mail.
//!
//! A rule is for one account's mail (#566); one from before rules had accounts is for every
//! account's, on this computer. A rule of a Microsoft account is kept on its server when the
//! server can do all of it ([`server_cannot`]): it is then sent there (`rules_remote`) and runs
//! there, whether MailCraft is open or not. The rules of an account are in two lists, as they
//! run: those of its server first, then those of this computer; Move Up and Move Down move a rule
//! within its list.

use mailcraft_model::{Action, Condition, Id, Importance, Keep, Mailbox, Rule, clean_name};
use serde_json::{Value, json};

use super::{bad, fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

use mailcraft_model::MAX_RULES;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rules.add", "Create Rule", "Home › Move", add).params(
            concat!(
                r#"{"name": string, "account"?: number (absent: every account), "conditions": [Condition], "exceptions"?: [Condition] (any vetoes), "actions": [Action], "enabled"?: bool (default true), "stop"?: bool, "server"?: bool (false: local; true: require server support; absent: server when supported), "runIn"?: folder id (only the new predicate, before consolidation). "#,
                r#"Condition: {"type":"fromContains"|"toContains"|"subjectContains"|"bodyContains","value":string} | {"type":"fromAny","value":[string]} (up to 200 sender substrings) | {"type":"fromAddresses"|"toAddresses","value":[string]} (up to 200 exact plain transport sender or To/Cc addresses, ASCII case-insensitive, no display-name matching, 254 UTF-8 bytes/address) | {"type":"hasAttachment"} | {"type":"importance","value":"low"|"normal"|"high"} | {"type":"text","value":{"field":"from"|"to"|"subject"|"body","pattern":string,"regex"?:bool,"caseSensitive"?:bool}}. "#,
                r#"Action: {"type":"moveTo"|"copyTo","value":folder id} | {"type":"categorize","value":string} | {"type":"flag"|"markRead"|"delete"} | {"type":"setImportance","value":string}. At most 20 conditions/exceptions/actions each; legacy text 200 characters, advanced pattern 4096 UTF-8 bytes; oversized values rejected, never truncated. Exact alternatives, advanced matching, exceptions and copies stay local. Adjacent compatible sender filing rules consolidate automatically without mixing substring and exact semantics. Answers {"id" (survivor),"keep":"server"|"here","mergedIds":[retired ids],"execution"?: result}. See docs/local-rules.md."#,
            ),
        ),
        CommandSpec::new("rules.remove", "Delete Rule", "Home › Move", remove).params(r#"{"id": number} — a rule its server has is deleted there too"#),
        CommandSpec::new("rules.edit", "Edit Local Rule", "Home › Move", edit)
            .params(r#"{"id": number, ...rules.add fields} — replace a local rule while preserving its identity, order and enabled state; server-owned originals are never rewritten"#),
        CommandSpec::new("rules.preview", "Preview Rules", "Home › Move", preview).reads()
            .params(r#"{"folder"?: number, "id"?: number, "rule"?: rules.add fields, "editing"?: stored id replaced by draft, "limit"?: number} — non-mutating evaluation of an entire folder, with matched total and up to 1000 detailed rows; id and draft rule are mutually exclusive; unavailable content is an explicit failure before any action"#),
        CommandSpec::new("rules.conflicts", "Check Rule Conflicts", "Home › Move", super::rules_conflicts::inspect).reads()
            .params(r#"{"id"?: number, "rule"?: rules.add fields} — inspect a stored or draft rule against enabled rules across all destinations; id excludes the edited original. Confirmed competing moves prevent activation; potential regex/complex overlaps remain explicit and actual-message execution is checked before any action. Read-only; copies to multiple destinations are intentional."#),
        CommandSpec::new("rules.pending", "Inspect Pending Classification", "Home › Move", pending).reads()
            .params(r#"{"account"?: number, "folder"?: number, "limit"?: number (default 100, max 1000)} — all pending/error totals and bounded message identities with durable classification status and content availability. Read-only; never runs rules or fetches mail."#),
        CommandSpec::new("rules.enable", "Enable Rule", "Home › Move", enable).params(r#"{"id": number, "value"?: bool (default: toggle)}"#),
        CommandSpec::new("rules.list", "Manage Rules", "Home › Move", list).reads().params(
            r#"{"account"?: number, "destination"?: folder id} — rules in execution order; destination includes disabled move/copy rules for that exact folder identity across accounts. Each includes its full stored rule and where it is kept: server, waiting, here or everyAccount. pendingDeletionCount covers all provider definitions awaiting confirmed removal for the chosen account(s); pendingDeletions contains up to 1000 records, with blocked explaining a definitive refusal and pendingDeletionsTruncated marking omitted details."#,
        ),
        CommandSpec::new("rules.run", "Run Rules Now", "Folder › Clean Up", run)
            .params(r#"{"folder"?: number (default: the folder in view), "id"?: number (that rule alone)} — execute enabled rules for the folder's account, including server rules. Answers examined/matched/applied message counts, failed rule count and first 100 failure details (with truncation flag); each failure includes rule/message ids and completedActions. Invalid local destinations pause the rule before any action. Source removal still waits for safe cross-account placement."#),
        CommandSpec::new("rules.fromMessage", "Always Move Messages From", "Home › Move", from_message)
            .params(r#"{"folder": number, "id"?: number, "server"?: bool (as for `rules.add`)} — a rule moving this sender's mail to the folder, run at once over the mail in the message's folder (not for a shared mailbox's message: its rules are its own); the window opens the New rule builder filled in instead (`ui.dialog` "rules" with "folder")"#)
            .selection(needs_message),
        CommandSpec::new("rules.move", "Move Rule", "Home › Move", move_rule).params(
            r#"{"id": number, "direction": "up"|"down", "account"?: number (the list it is moved in, for a rule for every account; default: the account of the folder in view)} — within its list: the rules its account's server keeps, or those of this computer"#,
        ),
        CommandSpec::new("rules.keepOnServer", "Keep Rule on Server", "Home › Move", keep_on_server).params(
            r#"{"id": number, "account"?: number (for a rule for every account: the Microsoft account whose server is to keep it, which it is then for alone)} — a rule kept on this computer goes to its Microsoft account's server at the next look, when the server can do all of it"#,
        ),
    ]
}

/// Why a rule can't be kept on an account's server (#566). Graph's rules know no flag; move mail
/// only to a folder of the same mailbox, and to one at most; and take several texts of one kind
/// as "any of them", where MailCraft's conditions all have to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotOnServer {
    /// MailCraft keeps no rules on the account's server (an account of this computer, IMAP).
    NoServer,
    Flag,
    /// The folder the rule moves mail to is on this computer.
    FolderHere,
    /// The folder is another account's.
    FolderElsewhere,
    /// The folder isn't on the account's server yet, or is gone.
    FolderNotThere,
    TwoMoves,
    TwoOfAKind,
    TwoImportances,
    /// It looks for a name in the sender or the recipients (words with a space, say), and the
    /// server looks at their addresses alone.
    Names,
    /// A Microsoft account kept in step by IMAP: its rules on the server are reached through
    /// Microsoft Graph.
    NotInStep,
    LocalMatching,
}

impl NotOnServer {
    /// Why, in words.
    pub fn said(self) -> &'static str {
        match self {
            NotOnServer::NoServer => "MailCraft can't keep this account's rules on its server",
            NotOnServer::Flag => "the server can't flag mail",
            NotOnServer::FolderHere => "the folder it moves mail to is on this computer",
            NotOnServer::FolderElsewhere => "the folder it moves mail to is in another account",
            NotOnServer::FolderNotThere => "the folder it moves mail to isn't on the server",
            NotOnServer::TwoMoves => "it moves mail to more than one folder",
            NotOnServer::TwoOfAKind => "it has two conditions of one kind, and the server would take either of them rather than both",
            NotOnServer::TwoImportances => "it sets more than one importance",
            NotOnServer::Names => "it looks for a name, and the server looks at addresses only",
            NotOnServer::NotInStep => "MailCraft reaches this account's rules on the server once it switches to Microsoft Graph",
            NotOnServer::LocalMatching => {
                "exact address alternatives, regex, explicit case matching, exceptions and copy actions run on this computer"
            }
        }
    }
}

/// Why `rule` can't be kept on the server of `account`, or `None` when it can.
pub fn server_cannot(mb: &Mailbox, rule: &Rule, account: Id) -> Option<NotOnServer> {
    if rule.conditions.iter().any(|c| {
        matches!(c, Condition::Text(_) | Condition::Unless(_) | Condition::FromAny(_) | Condition::FromAddresses(_) | Condition::ToAddresses(_))
    }) || rule.actions.iter().any(|a| matches!(a, Action::CopyTo(_)))
    {
        return Some(NotOnServer::LocalMatching);
    }
    if !mb.keeps_rules(account) {
        let imap = mb.account(account).is_some_and(|a| a.on_imap_with_microsoft() && !a.is_shared());
        return Some(if imap { NotOnServer::NotInStep } else { NotOnServer::NoServer });
    }
    let kind = |c: &Condition| std::mem::discriminant(c);
    if rule.conditions.iter().enumerate().any(|(i, c)| rule.conditions.iter().skip(i + 1).any(|d| kind(c) == kind(d))) {
        return Some(NotOnServer::TwoOfAKind);
    }
    if rule.conditions.iter().any(|c| matches!(c, Condition::FromContains(t) | Condition::ToContains(t) if !in_an_address(t))) {
        return Some(NotOnServer::Names);
    }
    let mut moves = 0usize;
    let mut importance: Option<Importance> = None;
    for action in &rule.actions {
        match action {
            Action::Flag => return Some(NotOnServer::Flag),
            Action::MoveTo(folder) => {
                moves += 1;
                let Some(f) = mb.folder(*folder) else { return Some(NotOnServer::FolderNotThere) };
                if f.account != account {
                    let local = mb.account(f.account).is_some_and(|a| !a.kind.has_server());
                    return Some(if local { NotOnServer::FolderHere } else { NotOnServer::FolderElsewhere });
                }
                if f.server.as_ref().is_none_or(|s| s.id.is_empty()) {
                    return Some(NotOnServer::FolderNotThere);
                }
            }
            Action::CopyTo(_) => return Some(NotOnServer::LocalMatching),
            Action::SetImportance(i) => {
                if importance.is_some_and(|had| had != *i) {
                    return Some(NotOnServer::TwoImportances);
                }
                importance = Some(*i);
            }
            Action::Categorize(_) | Action::MarkRead | Action::Delete => {}
        }
    }
    if moves > 1 { Some(NotOnServer::TwoMoves) } else { None }
}

/// Whether `words` can be part of an address: no space, none of the marks that set a name or a
/// comment apart from it. A rule looking for others looks for a name, which the server doesn't.
fn in_an_address(words: &str) -> bool {
    !words.chars().any(|c| c.is_whitespace() || "\"(),:;<>[]\\".contains(c))
}

/// Where a rule is kept, as `rules.list` says it, and why it is kept here when it could be
/// expected on the server.
pub fn kept(mb: &Mailbox, r: &Rule) -> (&'static str, Option<String>) {
    if r.account == 0 {
        return ("everyAccount", None);
    }
    if r.on_server() {
        return ("server", None);
    }
    let why = server_cannot(mb, r, r.account);
    if r.keep == Keep::Server {
        return ("waiting", why.map(|w| w.said().to_string()));
    }
    if !r.refused.is_empty() {
        return ("here", Some(format!("the server didn't take it: {}", r.refused)));
    }
    match why {
        Some(NotOnServer::NoServer | NotOnServer::NotInStep) | None => ("here", None),
        Some(why) => ("here", Some(why.said().to_string())),
    }
}

fn text(v: &Value) -> Result<String, CmdError> {
    let t = p::str(v, "value").unwrap_or("").trim().to_string();
    if t.chars().count() > 200 {
        return Err(bad("a literal condition supports at most 200 characters; the value was not truncated"));
    }
    if t.is_empty() { Err(bad("a condition needs a `value`")) } else { Ok(t) }
}

fn condition(v: &Value) -> Result<Condition, CmdError> {
    Ok(match p::req_str(v, "type")? {
        "fromContains" => Condition::FromContains(text(v)?),
        "fromAny" | "fromAddresses" | "toAddresses" => {
            let condition: Condition = serde_json::from_value(v.clone()).map_err(|error| bad(format!("invalid sender alternatives: {error}")))?;
            condition.validate().map_err(bad)?;
            condition
        }
        "toContains" => Condition::ToContains(text(v)?),
        "subjectContains" => Condition::SubjectContains(text(v)?),
        "bodyContains" => Condition::BodyContains(text(v)?),
        "hasAttachment" => Condition::HasAttachment,
        "importance" => {
            Condition::Importance(Importance::parse(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("importance is low, normal or high"))?)
        }
        "text" => {
            let text: mailcraft_model::RuleText = serde_json::from_value(v.get("value").cloned().unwrap_or(Value::Null))
                .map_err(|error| bad(format!("invalid text condition: {error}")))?;
            text.validate().map_err(bad)?;
            Condition::Text(text)
        }
        other => return Err(bad(format!("unknown condition type \"{other}\""))),
    })
}

fn action(s: &Session, v: &Value) -> Result<Action, CmdError> {
    Ok(match p::req_str(v, "type")? {
        "moveTo" | "copyTo" => {
            let folder = p::req_id(v, "value")?;
            refuse_send_queue(s, folder)?;
            let folder = s.mailbox.folder(folder).map(|f| f.id).ok_or_else(|| failed!("no folder with id {id}", id = folder))?;
            if p::str(v, "type") == Some("copyTo") { Action::CopyTo(folder) } else { Action::MoveTo(folder) }
        }
        "categorize" => Action::Categorize(clean_name(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("categorize needs a category name"))?),
        "flag" => Action::Flag,
        "markRead" => Action::MarkRead,
        "setImportance" => {
            Action::SetImportance(Importance::parse(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("importance is low, normal or high"))?)
        }
        "delete" => Action::Delete,
        other => return Err(bad(format!("unknown action type \"{other}\""))),
    })
}

/// A rule never moves mail into an Outbox: the next Send/Receive would send it.
fn refuse_send_queue(s: &Session, folder: Id) -> Result<(), CmdError> {
    if s.mailbox.is_send_queue(folder) { Err(fail("a rule can't move mail to an Outbox: that would send it")) } else { Ok(()) }
}

/// The account whose own mail a folder's is: a shared mailbox's is its owner's (rules leave a
/// shared mailbox's mail alone, and are made for the account it is opened through).
fn own_account(mb: &Mailbox, account: Id) -> Id {
    match mb.account(account) {
        Some(a) if a.is_shared() => mb.owner_of(account).map_or(account, |o| o.id),
        _ => account,
    }
}

/// The account a new rule is for: the one `v` names, or none (for every account's mail, kept on
/// this computer, as rules were before they had accounts: what a script that names none asked
/// for then).
fn rule_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let Some(account) = p::id_strict(v, "account")? else { return Ok(0) };
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.is_shared() {
        return Err(fail("a rule is for an account's own mail, not a shared mailbox's"));
    }
    Ok(account)
}

/// The rule `v` asks for, with no id yet (shared by the command and the agent's question).
pub(crate) fn rule_from_params(s: &Session, v: &Value) -> Result<Rule, CmdError> {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    for key in ["conditions", "exceptions", "actions"] {
        if v.get(key).and_then(Value::as_array).is_some_and(|parts| parts.len() > 20) {
            return Err(bad(format!("`{key}` supports at most 20 parts; none were truncated")));
        }
    }
    let mut conditions = v
        .get("conditions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`conditions` (a list) is required"))?
        .iter()
        .map(condition)
        .collect::<Result<Vec<_>, _>>()?;
    let actions = v
        .get("actions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`actions` (a list) is required"))?
        .iter()
        .map(|a| action(s, a))
        .collect::<Result<Vec<_>, _>>()?;
    if conditions.is_empty() || actions.is_empty() {
        return Err(bad("a rule needs at least one condition and one action"));
    }
    let exceptions = match v.get("exceptions") {
        None => Vec::new(),
        Some(Value::Array(parts)) => parts.iter().map(condition).collect::<Result<Vec<_>, _>>()?,
        _ => return Err(bad("`exceptions` must be a list")),
    };
    let account = rule_account(s, v)?;
    if !exceptions.is_empty() {
        conditions.push(Condition::Unless(exceptions));
    }
    let mut rule = Rule {
        id: 0,
        name,
        enabled: p::opt_bool(v, "enabled")?.unwrap_or(true),
        conditions,
        actions,
        stop: p::opt_bool(v, "stop")?.unwrap_or(false),
        account,
        ..Default::default()
    };
    rule.validation().map_err(bad)?;
    place(s, &mut rule, v)?;
    Ok(rule)
}

/// Where a new rule is kept, by `v`'s `server`: on a server that can do all of it, as Outlook
/// keeps an Exchange account's rules, unless the owner keeps it on this computer.
fn place(s: &Session, rule: &mut Rule, v: &Value) -> Result<(), CmdError> {
    let account = rule.account;
    let cannot = if account == 0 { None } else { server_cannot(&s.mailbox, rule, account) };
    match (p::opt_bool(v, "server")?, cannot) {
        (Some(false), _) => {}
        (Some(true), _) if account == 0 => return Err(bad("say which account's server is to keep it (`account`)")),
        (Some(true), Some(why)) => return Err(CmdError::Failed(why.said().to_string())),
        (_, None) if account != 0 => rule.keep = Keep::Server,
        _ => {}
    }
    Ok(())
}

/// Why the rule `v` describes (the params of `rules.add`, finished or not) couldn't be kept on
/// its account's server, or `None` when it could: what the New rule form says before the rule
/// is made. Parts not filled in yet are left out.
pub fn server_cannot_draft(s: &Session, v: &Value) -> Option<NotOnServer> {
    let account = p::id(v, "account").filter(|a| s.mailbox.account(*a).is_some())?;
    let parts = |key: &str| v.get(key).and_then(Value::as_array).map(|a| a.iter().take(20).collect::<Vec<_>>()).unwrap_or_default();
    let mut conditions: Vec<Condition> = parts("conditions").into_iter().filter_map(|c| condition(c).ok()).collect();
    let actions = parts("actions").into_iter().filter_map(|a| action(s, a).ok()).collect();
    let exceptions: Vec<Condition> = parts("exceptions").into_iter().filter_map(|c| condition(c).ok()).collect();
    if !exceptions.is_empty() {
        conditions.push(Condition::Unless(exceptions));
    }
    let rule = Rule { conditions, actions, account, ..Default::default() };
    server_cannot(&s.mailbox, &rule, account)
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let mut rule = rule_from_params(s, v)?;
    super::rules_conflicts::ensure_unambiguous(s, &rule)?;
    let run_in = p::id_strict(v, "runIn")?;
    if let Some(folder) = run_in {
        if s.mailbox.folder(folder).is_none() {
            return Err(failed!("no folder with id {id}", id = folder));
        }
        receiving(s, folder)?;
    }
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    rule.id = id;
    let keep = if rule.keep == Keep::Server { "server" } else { "here" };
    s.mailbox.rules.push(rule);
    // Run precisely the new predicate before a merge widens the stored sender group.
    let execution = run_in.map(|folder| run_on(s, folder, Some(id))).transpose()?;
    let (id, merged) = super::rules_merge::consolidate(s, id);
    let mut result = json!({"id": id, "keep": keep, "mergedIds": merged});
    if let Some(execution) = execution {
        result["execution"] = execution;
    }
    Ok(result)
}

fn changeable(s: &Session, id: Id) -> Result<usize, CmdError> {
    let i = s.mailbox.rules.iter().position(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if s.mailbox.rules.get(i).is_some_and(Rule::read_only) {
        return Err(fail("the server doesn't let this rule be changed"));
    }
    Ok(i)
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let index = changeable(s, id)?;
    let previous = s.mailbox.rules.get(index).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if previous.on_server() || previous.unanswered || !previous.known() {
        return Err(fail("preserve the server-owned or unresolved original; make a separate disabled local migration instead"));
    }
    let enabled = previous.enabled;
    let (provenance, conditions, actions) = (previous.merged_from.clone(), previous.conditions.clone(), previous.actions.clone());
    let mut rule = rule_from_params(s, v)?;
    if rule.keep != Keep::Here {
        return Err(bad("editing a local rule keeps it here; specify server=false"));
    }
    rule.id = id;
    rule.enabled = enabled;
    // Merge provenance records the original senders and destinations; a changed sender set or
    // destination no longer describes what the edited rule combines.
    if rule.conditions == conditions && rule.actions == actions {
        rule.merged_from = provenance;
    }
    super::rules_conflicts::ensure_unambiguous(s, &rule)?;
    let slot = s.mailbox.rules.get_mut(index).ok_or_else(|| failed!("no rule with id {id}", id))?;
    *slot = rule;
    let (id, merged) = super::rules_merge::consolidate(s, id);
    Ok(json!({"id": id, "keep": "here", "enabled": enabled, "mergedIds": merged}))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let i = changeable(s, id)?;
    let _ = s.mailbox.remove_rule_definition(i);
    Ok(json!({"deleted": id}))
}

fn enable(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let value = p::opt_bool(v, "value")?;
    let i = changeable(s, id)?;
    if let Some(stored) = s.mailbox.rules.get(i)
        && value.unwrap_or(!stored.enabled)
    {
        let mut candidate = stored.clone();
        candidate.enabled = true;
        super::rules_conflicts::ensure_unambiguous(s, &candidate)?;
    }
    let r = s.mailbox.rules.get_mut(i).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if value.unwrap_or(!r.enabled) && !r.repair.is_empty() {
        return Err(fail("repair the rule's definition before enabling it"));
    }
    r.enabled = value.unwrap_or(!r.enabled);
    let enabled = r.enabled;
    let (id, merged) = super::rules_merge::consolidate(s, id);
    Ok(json!({"id": id, "enabled": enabled, "mergedIds": merged}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let mb = &s.mailbox;
    let destination = p::id_strict(v, "destination")?;
    if let Some(id) = destination
        && mb.folder(id).is_none()
    {
        return Err(failed!("no folder with id {id}", id));
    }
    let order: Vec<usize> = match p::id_strict(v, "account")? {
        Some(account) => mb.rules_in_order(account),
        None => (0..mb.rules.len()).collect(),
    };
    let rules: Vec<Value> = order
        .into_iter()
        .filter_map(|i| mb.rules.get(i))
        .filter(|rule| destination.is_none_or(|folder| rule.targets(folder)))
        .map(|r| {
            let (where_kept, why) = kept(mb, r);
            let mut entry = json!({
                "id": r.id, "name": r.name, "enabled": r.enabled, "stop": r.stop, "text": r.describe_as(mb.rule_reads_addresses(r)),
                "account": r.account, "kept": where_kept, "rule": r,
            });
            if let Some(why) = why {
                entry["why"] = json!(why);
            }
            if let Some(server) = r.server.as_ref() {
                if !server.unknown.is_empty() {
                    entry["unknownParts"] = json!(server.unknown);
                }
                if server.read_only {
                    entry["readOnly"] = json!(true);
                }
                if server.error {
                    entry["error"] = json!(true);
                }
            }
            entry
        })
        .collect();
    let account = p::id_strict(v, "account")?;
    let pending = || mb.rule_gone.iter().filter(|gone| account.is_none_or(|account| gone.account == account));
    let total = pending().count();
    let details: Vec<_> = pending().take(1000).collect();
    Ok(json!({"rules": rules, "pendingDeletionCount": total, "pendingDeletions": details, "pendingDeletionsTruncated": total > details.len()}))
}

fn pending(s: &mut Session, params: &Value) -> CmdResult {
    let account = p::id_strict(params, "account")?;
    let folder = p::id_strict(params, "folder")?;
    let limit = p::id_strict(params, "limit")?.unwrap_or(100).min(1000) as usize;
    if account.is_some_and(|id| s.mailbox.account(id).is_none()) || folder.is_some_and(|id| s.mailbox.folder(id).is_none()) {
        return Err(bad("unknown account or folder"));
    }
    let mut total = 0usize;
    let mut waiting = 0usize;
    let mut messages = Vec::new();
    for message in
        s.mailbox.messages.values().filter(|message| account.is_none_or(|id| message.account == id) && folder.is_none_or(|id| message.folder == id))
    {
        let Some(state) = message.rule_processing.as_deref() else { continue };
        total += 1;
        waiting += usize::from(message.rules_wait_for_content());
        if messages.len() < limit {
            messages.push(json!({"id":message.id,"account":message.account,"folder":message.folder,"state":state,
                "unfetched":message.unfetched,"incomplete":message.incomplete,"notDownloaded":message.not_downloaded}));
        }
    }
    Ok(json!({"total":total,"waitingForContent":waiting,"failed":total - waiting,"messages":messages,
        "detailsTruncated":total > messages.len(),"mutated":false}))
}

/// Conditions and cross-account copies must see available on-disk content too.
fn reads_content(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.rules.iter().any(|rule| {
        (rule.enabled || rule.server.as_ref().is_some_and(|server| server.synced_enabled))
            && rule.is_for(account)
            && (rule.reads_body()
                || rule.actions.iter().any(|action| match action {
                    Action::CopyTo(folder) => mailbox.folder(*folder).is_some_and(|folder| folder.account != account),
                    _ => false,
                }))
    })
}

/// Rules classify received mail only, as ingest does ([`Mailbox::can_classify_received`]): a
/// folder that is wholly outside receiving (Drafts, Sent, Deleted Items, Junk, Outbox and their
/// children, or a shared mailbox) is refused rather than silently examined.
fn receiving(s: &Session, folder: Id) -> Result<(), CmdError> {
    let shared = s.mailbox.folder(folder).and_then(|f| s.mailbox.account(f.account)).is_none_or(|a| a.is_shared());
    if shared || !super::filing::receiving_folder(s, folder) {
        return Err(bad("rules run on received mail only, outside Drafts, Sent, Deleted Items, Junk, Outbox and shared mailboxes"));
    }
    Ok(())
}

/// The messages of `folder` rules may classify, and how many others were skipped.
fn classifiable(s: &Session, folder: Id) -> (Vec<Id>, usize) {
    let mut skipped = 0usize;
    let ids: Vec<Id> = s
        .mailbox
        .messages_in(folder)
        .map(|m| m.id)
        .filter(|id| {
            let keep = s.mailbox.can_classify_received(*id);
            skipped += usize::from(!keep);
            keep
        })
        .collect();
    (ids, skipped)
}

/// Run every enabled rule for the folder's account on the received messages of `folder`, those
/// its server has among them, or the rule `only` alone; answers how many messages a rule acted on.
fn run_on(s: &mut Session, folder: Id, only: Option<Id>) -> CmdResult {
    receiving(s, folder)?;
    let (ids, skipped) = classifiable(s, folder);
    let account = s.mailbox.folder(folder).map_or(0, |f| f.account);
    // A rule that reads the body reads it whole: one kept on disk is brought in first (one that
    // can't be read says so in its place, and is matched as that).
    let reads_bodies = reads_content(&s.mailbox, account);
    let (mut matched, mut failed, mut applied) = (0usize, 0usize, 0usize);
    let mut conflicting_messages = 0usize;
    let mut failures = Vec::new();
    for id in &ids {
        if reads_bodies {
            s.load_where(*id, |_| false)?;
        }
        let result = match only {
            Some(rule) => s.mailbox.run_rule(*id, rule),
            None => s.mailbox.run_rules(*id, true),
        };
        matched += usize::from(result.matched > 0);
        conflicting_messages += usize::from(result.conflicts);
        applied += usize::from(!result.applied.is_empty());
        failed += result.failures.len();
        for failure in result.failures {
            if failures.len() < 100 {
                failures.push(failure);
            }
        }
    }
    Ok(
        json!({"examined": ids.len(), "skipped": skipped, "matched": matched, "applied": applied, "failed": failed, "conflictingMessages":conflicting_messages,
        "failures": failures, "failureDetailsTruncated": failed > failures.len(), "dryRun": false}),
    )
}

fn run(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::id_strict(v, "folder")?.unwrap_or(s.view.folder);
    let only = p::id_strict(v, "id")?;
    if s.mailbox.folder(folder).is_none() {
        return Err(failed!("no folder with id {id}", id = folder));
    }
    if let Some(rule) = only
        && !s.mailbox.rules.iter().any(|r| r.id == rule)
    {
        return Err(failed!("no rule with id {id}", id = rule));
    }
    run_on(s, folder, only)
}

fn preview(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::id_strict(v, "folder")?.unwrap_or(s.view.folder);
    let account = s.mailbox.folder(folder).ok_or_else(|| failed!("no folder with id {id}", id = folder))?.account;
    let mut only = p::id_strict(v, "id")?;
    let editing = p::id_strict(v, "editing")?;
    if only.is_some() && v.get("rule").is_some() {
        return Err(bad("choose an id or an unsaved rule, not both"));
    }
    let draft = v.get("rule").map(|rule| rule_from_params(s, rule)).transpose()?;
    if let Some(id) = only
        && !s.mailbox.rules.iter().any(|rule| rule.id == id)
    {
        return Err(failed!("no rule with id {id}", id));
    }
    let limit = p::id_strict(v, "limit")?.unwrap_or(100).min(1000) as usize;
    receiving(s, folder)?;
    let (ids, skipped) = classifiable(s, folder);
    let mut shadow = s.mailbox.clone();
    shadow.messages.clear();
    shadow.pending.clear();
    shadow.queued.clear();
    if let Some(mut rule) = draft {
        rule.id = 0;
        shadow.rules.retain(|stored| Some(stored.id) != editing);
        shadow.rules.push(rule);
        only = Some(0);
    }
    let reads_body = reads_content(&shadow, account);
    let mut matched = 0usize;
    let mut failed = 0usize;
    let mut conflicting_messages = 0usize;
    let mut failures = Vec::new();
    let mut messages = Vec::new();
    for id in &ids {
        let Some(message) = s.mailbox.message(*id) else { continue };
        let message = if reads_body { s.loaded_message_where(*id, |_| false)? } else { std::sync::Arc::new(message.clone()) };
        shadow.messages.insert(*id, message);
        let result = match only {
            Some(rule) => shadow.run_rule(*id, rule),
            None => shadow.run_rules(*id, true),
        };
        failed += result.failures.len();
        conflicting_messages += usize::from(result.conflicts);
        for failure in result.failures {
            if failures.len() < 100 {
                failures.push(failure);
            }
        }
        if result.matched > 0 {
            matched += 1;
            if messages.len() < limit
                && let Some(after) = shadow.message(*id)
            {
                let copies: Vec<Value> = shadow
                    .messages
                    .values()
                    .filter(|message| message.id != *id)
                    .map(|message| json!({"account": message.account, "folder": message.folder}))
                    .collect();
                messages.push(json!({"id": id, "sourceAccount": account, "sourceFolder": folder,
                    "destinationAccount": after.account, "destinationFolder": after.folder, "read": after.read,
                    "copyDestinations": copies}));
            }
        }
        shadow.messages.clear();
        shadow.pending.clear();
        shadow.queued.clear();
    }
    Ok(
        json!({"examined": ids.len(), "skipped": skipped, "matched": matched, "messages": messages, "failed": failed, "conflictingMessages":conflicting_messages,
        "failures": failures, "failureDetailsTruncated": failed > failures.len(),
        "detailsTruncated": matched > messages.len(), "mutated": false}),
    )
}

/// The rule `rules.fromMessage` makes from `v`, with no id yet, and the folder the message is in
/// (shared by the command and the agent's question).
pub(crate) fn rule_from_message(s: &Session, v: &Value) -> Result<(Rule, Id), CmdError> {
    let folder = p::req_id(v, "folder")?;
    let target = s.mailbox.folder(folder).map(|f| f.name.clone()).ok_or_else(|| failed!("no folder with id {id}", id = folder))?;
    refuse_send_queue(s, folder)?;
    let mid = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(mid).ok_or_else(|| failed!("no message with id {id}", id = mid))?;
    // A shared mailbox's mail is left alone by the rules, and one for its owner's account would
    // move the owner's own mail into the folder it names.
    if s.mailbox.account(m.account).is_some_and(|a| a.is_shared()) {
        return Err(fail("a shared mailbox's rules are its own: this version doesn't make them"));
    }
    if m.from.email.trim().is_empty() {
        return Err(fail("the sender has no address to match"));
    }
    let name = format!("{} to {target}", m.from.display());
    let mut rule = Rule {
        name: name.chars().take(mailcraft_model::MAX_NAME).collect(),
        enabled: true,
        conditions: vec![Condition::FromContains(m.from.email.clone())],
        actions: vec![Action::MoveTo(folder)],
        account: m.account,
        ..Default::default()
    };
    place(s, &mut rule, v)?;
    Ok((rule, m.folder))
}

fn from_message(s: &mut Session, v: &Value) -> CmdResult {
    let (mut rule, source) = rule_from_message(s, v)?;
    super::rules_conflicts::ensure_unambiguous(s, &rule)?;
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    rule.id = id;
    s.mailbox.rules.push(rule);
    let mut result = run_on(s, source, Some(id))?;
    let (id, merged) = super::rules_merge::consolidate(s, id);
    result["id"] = json!(id);
    result["mergedIds"] = json!(merged);
    Ok(result)
}

/// The list a rule is in, in the rules of `account`: those for its server, or those of this
/// computer (with the rules for every account).
fn same_list(r: &Rule, account: Id, server: bool) -> bool {
    let for_server = r.account == account && r.keep == Keep::Server;
    for_server == server && r.is_for(account)
}

fn move_rule(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let up = match p::req_str(v, "direction")? {
        "up" => true,
        "down" => false,
        _ => return Err(bad("`direction` is \"up\" or \"down\"")),
    };
    let i = changeable(s, id)?;
    let rule = s.mailbox.rules.get(i).cloned().ok_or_else(|| failed!("no rule with id {id}", id))?;
    let account = match rule.account {
        0 => p::id_strict(v, "account")?.or_else(|| s.mailbox.folder(s.view.folder).map(|f| f.account)).map_or(0, |a| own_account(&s.mailbox, a)),
        own => own,
    };
    let server = rule.account == account && rule.keep == Keep::Server;
    let rules = &s.mailbox.rules;
    let next = if up {
        rules.iter().enumerate().take(i).rev().find(|(_, r)| same_list(r, account, server)).map(|(j, _)| j)
    } else {
        rules.iter().enumerate().skip(i + 1).find(|(_, r)| same_list(r, account, server)).map(|(j, _)| j)
    };
    let Some(j) = next else {
        return Ok(json!({"id": id, "moved": false}));
    };
    let other = rules.get(j).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if other.read_only() {
        return Err(fail("the server doesn't let the rule next to it be moved"));
    }
    if server && rule.on_server() != other.on_server() {
        return Err(fail("a rule waiting to go to the server goes after those there; it can be moved once it is there"));
    }
    // The one of the two that is in this account's list alone is taken out and put in on the
    // other side of the other, not swapped with it: a rule for every account is in every
    // account's list, and moving one across others would reorder another account's rules.
    if rule.account == 0 && other.account != 0 {
        let neighbour = s.mailbox.rules.remove(j);
        s.mailbox.rules.insert(i, neighbour);
    } else {
        let moving = s.mailbox.rules.remove(i);
        s.mailbox.rules.insert(j, moving);
    }
    if server {
        renumber(&mut s.mailbox, account);
    }
    let (id, merged) = super::rules_merge::consolidate(s, id);
    Ok(json!({"id": id, "moved": true, "mergedIds": merged}))
}

/// Give the rules the server of `account` has the places they now stand in: the same numbers as
/// before, in their new order (all of them afresh when two had the same). One the server doesn't
/// let be changed keeps its number, which no other is given: a new one would never be sent.
pub(crate) fn renumber(mb: &mut Mailbox, account: Id) {
    let on: Vec<usize> = mb.rules.iter().enumerate().filter(|(_, r)| r.account == account && r.on_server()).map(|(i, _)| i).collect();
    let locked = |i: &&usize| mb.rules.get(**i).is_some_and(Rule::read_only);
    let number = |i: &usize| mb.rules.get(*i).and_then(|r| r.server.as_ref()).map(|s| s.sequence);
    let taken: Vec<i32> = on.iter().filter(locked).filter_map(number).collect();
    let mut numbers: Vec<i32> = on.iter().filter(|i| !locked(i)).filter_map(number).collect();
    numbers.sort_unstable();
    // (The server's places start at 1: one below is sent as 1, and would stand with that one.)
    let distinct = numbers.windows(2).all(|w| w.first() < w.get(1)) && numbers.first().is_none_or(|n| *n >= 1);
    if !distinct || numbers.iter().any(|n| taken.contains(n)) {
        // Starting low enough that those listed before the last rule the server won't change stay
        // before it, where two had the same number and the list put this one second.
        let last = on.iter().rposition(|i| locked(&i)).unwrap_or(0);
        let before = on.iter().take(last).filter(|i| !locked(i)).count();
        let low = numbers.first().into_iter().chain(taken.iter()).copied().min().unwrap_or(1);
        let mut next = i32::try_from(before).map_or(1, |b| low.saturating_sub(b)).max(1);
        numbers.clear();
        for i in &on {
            if locked(&i) {
                next = next.max(number(i).unwrap_or(0).saturating_add(1));
                continue;
            }
            while taken.contains(&next) && next < i32::MAX {
                next = next.saturating_add(1);
            }
            numbers.push(next);
            next = next.saturating_add(1);
        }
    }
    let movable: Vec<usize> = on.iter().filter(|i| !locked(i)).copied().collect();
    for (i, number) in movable.into_iter().zip(numbers) {
        if let Some(server) = mb.rules.get_mut(i).and_then(|r| r.server.as_mut()) {
            server.sequence = number;
        }
    }
}

fn keep_on_server(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let i = s.mailbox.rules.iter().position(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    let rule = s.mailbox.rules.get(i).cloned().ok_or_else(|| failed!("no rule with id {id}", id))?;
    if rule.keep == Keep::Server {
        return Ok(json!({"id": id, "account": rule.account, "keep": "server"}));
    }
    let account = match (rule.account, p::id_strict(v, "account")?) {
        (0, Some(account)) => account,
        (0, None) => {
            let servers: Vec<Id> = s.mailbox.accounts.iter().map(|a| a.id).filter(|a| s.mailbox.keeps_rules(*a)).collect();
            match servers.as_slice() {
                [only] => *only,
                [] => return Err(fail("no account here keeps its rules on its server")),
                _ => return Err(bad("say which account's server is to keep it (`account`)")),
            }
        }
        (own, Some(other)) if other != own => return Err(fail("a rule of one account can't go to another account's server")),
        (own, _) => own,
    };
    if let Some(why) = server_cannot(&s.mailbox, &rule, account) {
        return Err(CmdError::Failed(why.said().to_string()));
    }
    if let Some(r) = s.mailbox.rules.get_mut(i) {
        r.account = account;
        r.keep = Keep::Server;
        r.refused.clear();
    }
    Ok(json!({"id": id, "account": account, "keep": "server"}))
}
