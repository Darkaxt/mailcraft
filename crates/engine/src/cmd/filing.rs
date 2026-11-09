// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Unified filing uses real Outlook folders and the existing transactional command/move path.

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Module, Scope, Session, p};
use mailcraft_model::{AccountKind, Action, Condition, FolderKind, Id, Keep, Rule};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("view.filing", "Unified filing", "Folder Pane", view).reads().params(
            r#"{"account":number,"folder"?:number} — use this existing Microsoft account's real folder tree. Its Inbox includes received mail not yet filed in that tree, across all accounts; other nodes show their real contents. No folder-name inference or mailbox mutation."#),
        CommandSpec::new("filing.preview", "Review sender filing", "Mail", preview).reads().params(
            r#"{"ids":[number],"folder":number,"unavailableOffset"?:number} — read-only exact-transport-sender filing review, including disabled/unsupported definitions, old/new real destinations, account scope, exact historical/unavailable totals and 200 unavailable details per page. Retain review unchanged for filing.apply. Header-reported originals never replace transport addresses."#),
        CommandSpec::new("filing.apply", "File and learn senders", "Mail", apply).params(
            r#"{"ids":[number],"folder":number,"review":string,"choice"?:"auto"|"reuse"|"reroute"|"once","reuseFolder"?:number} — recheck the unchanged preview before any mutation. auto creates/extends compatible exact-sender rules only without conflicts; reuse files into an existing reviewed destination and preserves rules; reroute splits only these sender alternatives and relocates matching received history; once files only the selection with durable per-message exceptions. Refuses protected/deleting/unsupported rule rewrites and unsafe destinations. One native undo/save transaction; provider placement remains pending until confirmed. Never replay an uncertain apply: inspect mail.filingStatus first."#),
    ]
}

pub(crate) fn receiving_folder(s: &Session, id: Id) -> bool {
    let mut next = Some(id);
    for _ in 0..s.mailbox.folders.len() {
        let Some(folder) = next.and_then(|id| s.mailbox.folder(id)) else { return false };
        if matches!(folder.kind, FolderKind::Drafts | FolderKind::Sent | FolderKind::Deleted | FolderKind::Junk | FolderKind::Outbox) {
            return false;
        }
        next = folder.parent;
        if next.is_none() {
            return true;
        }
    }
    false
}

pub fn unfiled_folders(s: &Session, account: Id) -> std::collections::HashSet<Id> {
    s.mailbox.folders.iter().filter(|f| (f.account != account || f.kind == FolderKind::Inbox) && receiving_folder(s, f.id)).map(|f| f.id).collect()
}

pub fn is_unfiled_view(s: &Session) -> bool {
    s.view.filing.is_some_and(|account| s.mailbox.folder(s.view.folder).is_some_and(|f| f.account == account && f.kind == FolderKind::Inbox))
        && matches!(s.view.scope, Scope::Folder | Scope::Inboxes)
}

fn canonical(s: &Session, account: Id) -> Result<(), crate::CmdError> {
    if !s.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft && !a.is_shared()) {
        return Err(bad("choose an existing non-shared Microsoft account for the canonical tree"));
    }
    Ok(())
}

fn view(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["account", "folder"])?;
    let account = p::req_id(v, "account")?;
    canonical(s, account)?;
    let folder = p::id_strict(v, "folder")?
        .or_else(|| s.mailbox.folder_of_kind(account, FolderKind::Inbox))
        .ok_or_else(|| fail("the canonical account has no real Inbox yet"))?;
    if !s.mailbox.folder(folder).is_some_and(|f| f.account == account) {
        return Err(bad("the folder must belong to the canonical account"));
    }
    s.view.module = Module::Mail;
    s.view.gmail = None;
    s.view.folder = folder;
    s.view.filing = Some(account);
    s.view.search_folder = None;
    s.view.scope = Scope::Folder;
    s.view.search.clear();
    s.view.open.clear();
    s.sel.clear_messages();
    s.server_search.forget();
    Ok(json!({"account":account,"folder":folder,"virtual":true}))
}

fn destination(s: &Session, folder: Id) -> Result<Id, crate::CmdError> {
    let account = s.mailbox.folder(folder).ok_or_else(|| bad("the destination no longer exists"))?.account;
    canonical(s, account)?;
    if !receiving_folder(s, folder) || s.mailbox.is_send_queue(folder) {
        return Err(bad("choose an ordinary receiving folder, outside Sent, Drafts, Trash, Junk and send queues"));
    }
    Ok(account)
}

fn location(s: &Session, id: Id) -> Value {
    let account = s.mailbox.folder(id).map(|f| f.account);
    json!({"id":id,"account":account,"path":s.mailbox.folder_path(id),
        "email":account.and_then(|id|s.mailbox.account(id)).map(|a|a.email.as_str())})
}

fn candidate(senders: &[String], folder: Id) -> Rule {
    Rule {
        name: format!("File senders to #{folder}"),
        enabled: true,
        account: 0,
        conditions: vec![Condition::FromAddresses(senders.to_vec())],
        actions: vec![Action::MoveTo(folder)],
        ..Default::default()
    }
}

fn blocked(s: &Session, rule: &Rule) -> Option<String> {
    if !rule.known() || rule.read_only() || rule.unanswered || rule.server.is_some() || rule.keep != Keep::Here {
        return Some("preserve the server-owned, unsupported or unresolved original; it cannot be rewritten from a local review".into());
    }
    if !rule.repair.is_empty() || rule.validation().is_err() {
        return Some("repair this incomplete rule before rerouting".into());
    }
    if rule.conditions.iter().filter(|c| !matches!(c, Condition::Unless(_))).count() >= 20
        || rule.conditions.iter().any(|c| matches!(c,Condition::Unless(parts) if parts.len()>=20))
    {
        return Some("the existing rule has no capacity for a lossless sender split; narrow it explicitly first".into());
    }
    if crate::agent::rule_can_delete_mail(&s.mailbox, rule) {
        return Some("this rule has deleting effects; sender filing does not rewrite it".into());
    }
    None
}

struct Plan {
    ids: Vec<Id>,
    senders: Vec<String>,
    conflicts: Vec<Rule>,
    history: Vec<Id>,
    report: Value,
}

fn plan(s: &Session, v: &Value) -> Result<Plan, crate::CmdError> {
    let folder = p::req_id(v, "folder")?;
    destination(s, folder)?;
    let ids = p::ids_strict(v)?.filter(|ids| !ids.is_empty()).ok_or_else(|| bad("provide a nonempty explicit ids selection"))?;
    let ids: Vec<_> = ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    let mut senders = BTreeSet::new();
    let mut selected = Vec::new();
    for id in &ids {
        let m = s.mailbox.message(*id).ok_or_else(|| bad("a selected message no longer exists"))?;
        if !s.mailbox.can_classify_received(*id) {
            return Err(bad("sender filing accepts received mail only, outside Drafts, Sent, Trash, Junk and shared mailboxes"));
        }
        if !mailcraft_model::search::is_address(&m.from.email) || m.from.email.len() > mailcraft_model::MAX_ADDRESS {
            return Err(bad("the selected message has no exact plain transport sender address"));
        }
        senders.insert(m.from.email.to_ascii_lowercase());
        selected.push(json!({"id":m.id,"account":m.account,"folder":m.folder,"from":m.from.email,
            "originAccount":m.origin_account(),"alias":m.origin_alias(),"override":m.filing_override,
            "messageId":m.message_id,"server":m.server,"movingOut":m.moving_out,"uid":m.uid,
            "unfetched":m.unfetched,"incomplete":m.incomplete,"notDownloaded":m.not_downloaded}));
    }
    let senders: Vec<_> = senders.into_iter().collect();
    if senders.len() > mailcraft_model::MAX_RULE_SENDERS {
        return Err(bad("one filing operation supports at most 200 exact sender alternatives"));
    }
    let draft = candidate(&senders, folder);
    draft.validation().map_err(bad)?;
    let conflicts: Vec<_> =
        s.mailbox.rules.iter().filter(|r| !r.known() || mailcraft_model::rule_conflicts::competing_moves(&draft, r)).cloned().collect();
    // Literal disjoint sender groups are provably independent. More complex unmatched
    // predicates remain visible as potential conflicts, rather than silently assuming safety.
    let conflicts: Vec<_> = conflicts
        .into_iter()
        .filter(|r| {
            if !r.known() {
                return true;
            }
            !r.conditions.iter().any(|condition| match condition {
                Condition::FromAddresses(addresses) => !addresses.iter().any(|a| senders.iter().any(|b| a.eq_ignore_ascii_case(b))),
                Condition::Unless(parts) => parts.iter().any(|part| {
                    matches!(part,Condition::FromAddresses(addresses)
                if senders.iter().all(|sender|addresses.iter().any(|a|a.eq_ignore_ascii_case(sender))))
                }),
                _ => false,
            })
        })
        .collect();
    let mut by_folder: BTreeMap<Id, usize> = BTreeMap::new();
    let mut history = Vec::new();
    let mut unavailable = Vec::new();
    let mut probe = s.mailbox.clone();
    for m in s.mailbox.messages.values().filter(|m| {
        s.mailbox.can_classify_received(m.id) && m.filing_override.is_none() && senders.iter().any(|a| a.eq_ignore_ascii_case(&m.from.email))
    }) {
        *by_folder.entry(m.folder).or_default() += 1;
        history.push(m.id);
        let previous = probe.clone();
        if let Err(error) = probe.move_message(m.id, folder) {
            probe = previous;
            unavailable.push(json!({"id":m.id,"reason":error.to_string()}));
        }
    }
    let rules: Vec<_> = conflicts
        .iter()
        .map(|r| {
            let destinations: Vec<_> = mailcraft_model::rule_conflicts::move_destinations(r).into_iter().map(|id| location(s, id)).collect();
            let mut enabled = r.clone();
            enabled.enabled = true;
            json!({"id":r.id,"name":r.name,"account":r.account,"enabled":r.enabled,"stop":r.stop,
            "text":r.describe_as(s.mailbox.rule_reads_addresses(r)),
            "accountEmail":s.mailbox.account(r.account).map(|a|a.email.as_str()),
            "conditions":r.conditions,"actions":r.actions,"destinations":destinations,
            "certainty":if r.known() && mailcraft_model::rule_conflicts::proven_overlap(&s.mailbox,&draft,&enabled) {"confirmed"} else {"potential"},
            "rerouteBlocked":blocked(s,r)})
        })
        .collect();
    let scope: Vec<_> = by_folder.into_iter().map(|(id, count)| json!({"folder":location(s,id),"messages":count})).collect();
    let review = serde_json::to_string(&json!({"revision":s.rev(),"folder":location(s,folder),
        "selected":selected,"rules":s.mailbox.rules,"history":history,"unavailable":unavailable}))
    .map_err(|e| fail(e.to_string()))?;
    // Optimistic state comparison, not a permission/admission token. Apply still performs
    // every current validation inside the ordinary native command transaction.
    let stamp = format!("{:016x}", mailcraft_model::text_hash(&review));
    let selected_unavailable = unavailable.iter().filter(|m| m["id"].as_u64().is_some_and(|id| ids.contains(&id))).count();
    let offset = match v.get("unavailableOffset") {
        None => 0,
        Some(value) => usize::try_from(value.as_u64().ok_or_else(|| bad("unavailableOffset is a whole number"))?)
            .map_err(|_| bad("unavailableOffset is too large"))?,
    };
    let end = offset.saturating_add(200).min(unavailable.len());
    let page = unavailable.get(offset..end).unwrap_or_default();
    let report = json!({"review":stamp,"destination":location(s,folder),"selected":ids.len(),"senders":senders,
        "sourceAccounts":"all configured accounts; exact transport From only","conflicts":rules,
        "historicalMessages":history.len(),"historicalScope":scope,"unavailable":page,
        "unavailableTotal":unavailable.len(),"unavailableOffset":offset,"nextUnavailableOffset":(end<unavailable.len()).then_some(end),
        "selectedUnavailable":selected_unavailable,"rerouteAllowed":conflicts.iter().all(|r|blocked(s,r).is_none()),
        "oneTimeExceptionsExcluded":true,"mutated":false});
    Ok(Plan { ids, senders, conflicts, history, report })
}

fn preview(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["ids", "folder", "unavailableOffset"])?;
    Ok(plan(s, v)?.report)
}

/// Exclude exactly these transport alternatives from an existing rule, preserving all its
/// remaining conditions, exceptions, actions, scope and enabled/Stop state.
fn except(rule: &mut Rule, senders: &[String]) -> Result<(), crate::CmdError> {
    let exclusion = Condition::FromAddresses(senders.to_vec());
    if let Some(Condition::Unless(parts)) = rule.conditions.iter_mut().find(|c| matches!(c, Condition::Unless(_))) {
        parts.push(exclusion);
    } else {
        rule.conditions.push(Condition::Unless(vec![exclusion]));
    }
    rule.validation().map_err(bad)
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["ids", "folder", "review", "choice", "reuseFolder"])?;
    let plan = plan(s, v)?;
    if p::req_str(v, "review")? != plan.report["review"].as_str().unwrap_or("") {
        return Err(fail("filing review is stale; preview the current rules/messages again before any change"));
    }
    let choice = match v.get("choice") {
        None => "auto",
        Some(Value::String(choice)) => choice.as_str(),
        _ => return Err(bad("choice is auto, reuse, reroute or once")),
    };
    if !matches!(choice, "auto" | "reuse" | "reroute" | "once") {
        return Err(bad("choice is auto, reuse, reroute or once"));
    }
    if choice == "auto" && !plan.conflicts.is_empty() {
        return Err(fail("existing destinations require reuse, reroute or a one-time exception; nothing changed"));
    }
    let folder = if choice == "reuse" {
        let existing = p::req_id(v, "reuseFolder")?;
        if !plan.conflicts.iter().any(|r| mailcraft_model::rule_conflicts::move_destinations(r).contains(&existing)) {
            return Err(bad("reuseFolder must be one of the reviewed existing destinations"));
        }
        destination(s, existing)?;
        existing
    } else {
        p::req_id(v, "folder")?
    };
    if choice == "reroute" && plan.conflicts.iter().any(|r| blocked(s, r).is_some()) {
        return Err(fail("a reviewed rule cannot be rewritten safely; preserve it and use a one-time filing or its existing destination"));
    }
    let before = s.rows();
    let mut new_rules = Vec::new();
    if matches!(choice, "auto" | "reroute") {
        if choice == "reroute" {
            for original in &plan.conflicts {
                let index = s.mailbox.rules.iter().position(|r| r.id == original.id).ok_or_else(|| fail("a reviewed rule disappeared"))?;
                let mut remainder = original.clone();
                except(&mut remainder, &plan.senders)?;
                let mut redirected = original.clone();
                redirected.id = s.mailbox.alloc_id();
                redirected.name = format!("{} — selected senders", original.name);
                redirected.conditions.push(Condition::FromAddresses(plan.senders.clone()));
                for action in &mut redirected.actions {
                    if let Action::MoveTo(id) = action {
                        *id = folder;
                    }
                }
                redirected.merged_from.clear();
                redirected.validation().map_err(bad)?;
                *s.mailbox.rules.get_mut(index).ok_or_else(|| fail("a reviewed rule disappeared"))? = remainder;
                s.mailbox.rules.insert(index, redirected.clone());
                new_rules.push(redirected.id);
            }
        }
        let mut rule = candidate(&plan.senders, folder);
        rule.id = s.mailbox.alloc_id();
        rule.name = format!("File senders to {}", s.mailbox.folder_path(folder));
        super::rules_conflicts::ensure_unambiguous(s, &rule)?;
        let id = rule.id;
        s.mailbox.rules.push(rule);
        let (survivor, _) = super::rules_merge::consolidate(s, id);
        new_rules.push(survivor);
        if s.mailbox.rules.len() > mailcraft_model::MAX_RULES {
            return Err(fail("not enough rule capacity for a lossless sender split after consolidation; nothing changed"));
        }
    }
    // All changes above and below are inside Session::run's existing rollback/undo snapshot.
    // No provider request happens here: existing queued placement owns confirmation/recovery.
    s.mailbox.move_messages(&plan.ids, folder)?;
    if matches!(choice, "once" | "reuse") {
        for id in &plan.ids {
            s.mailbox.message_mut(*id)?.filing_override = Some(folder);
        }
    } else {
        for id in &plan.ids {
            s.mailbox.message_mut(*id)?.filing_override = None;
        }
    }
    let mut moved = plan.ids.clone();
    let mut unavailable = Vec::new();
    if choice == "reroute" {
        for id in plan.history.iter().filter(|id| !plan.ids.contains(id)) {
            let previous = s.mailbox.clone();
            match s.mailbox.move_message(*id, folder) {
                Ok(()) => moved.push(*id),
                Err(error) => {
                    s.mailbox = previous;
                    let reason = error.to_string();
                    s.mailbox.message_mut(*id)?.rule_processing = Some(Box::new(mailcraft_model::RuleProcessing::Failed {
                        rule: new_rules.last().copied().unwrap_or(0),
                        error: reason.clone(),
                    }));
                    unavailable.push(json!({"id":id,"reason":reason}));
                }
            }
        }
    }
    super::select_after_removal(s, &before, &moved);
    Ok(json!({"choice":choice,"folder":folder,"selected":plan.ids.len(),"processed":moved.len(),"rules":new_rules,
        "unavailable":unavailable.iter().take(200).collect::<Vec<_>>(),"unavailableTotal":unavailable.len(),
        "unavailableDetails":"first 200; all failures remain in rules.pending, with paged offsets",
        "oneTime":matches!(choice,"once"|"reuse"),
        "placement":"local transaction queued; use mail.filingStatus for actual provider confirmation", "replay":"do not replay an uncertain apply"}))
}
