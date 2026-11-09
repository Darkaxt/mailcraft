// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saved indexed queries are native local metadata, with exact real scope bindings.

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Module, Session, p};
use mailcraft_model::{Id, SearchFolder, SearchScope};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("folder.newSearchFolder","New Search Folder","Folder › New",add).params(
            r#"{"name":string,"query":string,"scope":"folder"|"subfolders"|"mailbox"|"everywhere"|"inboxes","folder"?:number,"filing"?:number} — save a named local indexed query, never a provider folder. Folder/mailbox scopes require the exact real folder id. No mail is changed; native Undo removes the definition."#),
        CommandSpec::new("searchFolders.list","List Search Folders","Search",list).reads().params(
            r#"{} — all saved local query definitions (at most200) with exact scope, availability/reason and selected id. Does not execute each search, load bodies or contact providers."#),
        CommandSpec::new("searchFolders.edit","Edit Search Folder","Search",edit).params(
            r#"{"id":number,"name":string,"query":string,"scope":string,"folder"?:number,"filing"?:number} — atomically replace this local definition after validating the full query/scope; updates its result view when currently open, without changing messages."#),
        CommandSpec::new("searchFolders.preview","Preview Search Folder","Search",preview).reads().params(
            r#"{"id"?:number,"name":string,"query":string,"scope":"folder"|"subfolders"|"mailbox"|"everywhere"|"inboxes","folder"?:number,"filing"?:number} — the complete New/Edit definition. Read-only first50 real matches plus exact known total, bounded continuation and index coverage. Does not save a definition, alter the owner's view/selection or request a provider search. Partial coverage is not a complete negative."#),
        CommandSpec::new("searchFolders.select","Open Search Folder","Search",select).reads().params(
            r#"{"id":number} — run a saved query in the existing native indexed view; missing anchors are refused without broadening scope. Real message ids and normal result/coverage semantics apply."#),
        CommandSpec::new("searchFolders.remove","Delete Search Folder","Search",remove).params(
            r#"{"id":number} — remove only this local saved definition; does not delete, copy or move mail. Reversible with native Undo."#),
    ]
}

pub fn unavailable(s: &Session, search: &SearchFolder) -> Option<String> {
    if search.query.len() > mailcraft_model::search::text::MAX_QUERY_BYTES {
        return Some("search query is longer than 16384 UTF-8 bytes".into());
    }
    if search.query.trim().is_empty() {
        return Some("the saved search has no query; edit it before opening".into());
    }
    if let Some(why) = mailcraft_model::search::Query::parse(&search.query).error() {
        return Some(why.into());
    }
    anchor_problem(s, search)
}

/// Cheap sidebar metadata check; does not parse or execute every saved query each frame.
pub fn anchor_problem(s: &Session, search: &SearchFolder) -> Option<String> {
    if matches!(search.scope, SearchScope::Folder | SearchScope::Subfolders | SearchScope::Mailbox)
        && !search.folder.is_some_and(|id| s.mailbox.folder(id).is_some())
    {
        return Some("the exact folder/mailbox anchor is unavailable; choose a real folder in Edit search folder".into());
    }
    if let Some(account) = search.filing
        && (!s.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft && !a.is_shared())
            || !search.folder.is_some_and(|id| s.mailbox.folder(id).is_some_and(|f| f.account == account)))
    {
        return Some("the exact unified filing account/folder is unavailable; edit the saved binding".into());
    }
    None
}

pub fn active_problem(s: &Session) -> Option<String> {
    s.view.search_folder.and_then(|id| s.mailbox.search_folders.iter().find(|search| search.id == id)).and_then(|search| anchor_problem(s, search))
}

fn definition(s: &Session, v: &Value, id: Id) -> Result<SearchFolder, crate::CmdError> {
    p::only(v, &["id", "name", "query", "scope", "folder", "filing"])?;
    let name = p::req_str(v, "name")?.trim();
    if name.is_empty() || name.chars().count() > mailcraft_model::MAX_NAME || name.chars().any(char::is_control) {
        return Err(bad(format!("provide a nonempty search-folder name of at most {} characters, without controls", mailcraft_model::MAX_NAME)));
    }
    let query = p::req_str(v, "query")?.to_owned();
    let scope = crate::session::parse_enum::<SearchScope>(p::req_str(v, "scope")?)
        .ok_or_else(|| bad("scope is folder, subfolders, mailbox, everywhere or inboxes"))?;
    let folder = p::id_strict(v, "folder")?;
    let filing = p::id_strict(v, "filing")?;
    let search = SearchFolder { id, name: name.into(), query, scope, folder, filing };
    if let Some(why) = unavailable(s, &search) {
        return Err(bad(why));
    }
    if s.mailbox.search_folders.iter().any(|other| other.id != id && other.name.to_lowercase() == name.to_lowercase()) {
        return Err(bad("another search folder already has this name"));
    }
    Ok(search)
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    if v.get("id").is_some() {
        return Err(bad("a new search folder has no id parameter"));
    }
    let mut search = definition(s, v, 0)?;
    if s.mailbox.search_folders.len() >= mailcraft_model::MAX_SEARCH_FOLDERS {
        return Err(fail("this mailbox already has 200 saved search folders"));
    }
    search.id = s.mailbox.alloc_id();
    let id = search.id;
    s.mailbox.search_folders.push(search);
    Ok(json!({"id":id,"virtual":true,"messagesChanged":0}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &[])?;
    let searches: Vec<_> = s
        .mailbox
        .search_folders
        .iter()
        .map(|search| {
            let reason = unavailable(s, search);
            json!({"definition":search,"available":reason.is_none(),"reason":reason})
        })
        .collect();
    Ok(json!({"searchFolders":searches,"selected":s.view.search_folder,"queriesExecuted":0,"providerWrites":0}))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let replacement = definition(s, v, id)?;
    let search = s.mailbox.search_folders.iter_mut().find(|r| r.id == id).ok_or_else(|| bad("no search folder with this id"))?;
    *search = replacement;
    // Saving an edit is an explicit replacement of the open definition. Rebind it exactly;
    // dropping a broken anchor and keeping its query could accidentally widen the scope.
    if s.view.search_folder == Some(id) {
        select(s, &json!({"id":id}))?;
    }
    Ok(json!({"id":id,"virtual":true,"messagesChanged":0}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id"])?;
    let id = p::req_id(v, "id")?;
    let search = s.mailbox.search_folders.iter().find(|r| r.id == id).cloned().ok_or_else(|| bad("no search folder with this id"))?;
    if let Some(why) = unavailable(s, &search) {
        return Err(fail(why));
    }
    s.view.module = Module::Mail;
    s.view.gmail = None;
    s.view.filter = crate::Filter::All;
    s.view.folder = search.folder.unwrap_or_else(|| s.mailbox.inbox().unwrap_or(0));
    s.view.scope = search.scope;
    s.view.search = search.query;
    s.view.filing = search.filing;
    s.view.search_folder = Some(id);
    s.view.open.clear();
    s.sel.clear_messages();
    s.server_search.forget();
    Ok(json!({"id":id,"query":s.view.search,"scope":s.view.scope,"folder":s.view.folder,
        "found":crate::list::listed(&s.rows()),"searchIndex":s.content_index_status(),"virtual":true}))
}

fn preview(s: &mut Session, v: &Value) -> CmdResult {
    let search = definition(s, v, p::id_strict(v, "id")?.unwrap_or(0))?;
    let view = s.view.clone();
    let selection = s.sel.clone();
    s.view.module = Module::Mail;
    s.view.filter = crate::Filter::All;
    s.view.gmail = None;
    s.view.folder = search.folder.unwrap_or_else(|| s.mailbox.inbox().unwrap_or(0));
    s.view.scope = search.scope;
    s.view.search = search.query;
    s.view.filing = search.filing;
    s.view.search_folder = None;
    s.view.open.clear();
    let result = s.run("mail.list", &json!({"limit":50}));
    s.view = view;
    s.sel = selection;
    result
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id"])?;
    let id = p::req_id(v, "id")?;
    if !s.mailbox.search_folders.iter().any(|r| r.id == id) {
        return Err(bad("no search folder with this id"));
    }
    let broken = active_problem(s).is_some();
    s.mailbox.search_folders.retain(|search| search.id != id);
    if s.view.search_folder == Some(id) {
        s.view.search_folder = None;
        if broken {
            s.clear_search();
            s.view.scope = SearchScope::Folder;
            s.view.filing = None;
        }
    }
    Ok(json!({"deletedDefinition":id,"messagesDeleted":0,"providerWrites":0}))
}
