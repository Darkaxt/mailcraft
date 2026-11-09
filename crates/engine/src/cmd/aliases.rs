// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only forwarding identity inspection and account-scoped virtual alias views.

use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Module, Scope, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("aliases.list", "Inspect SimpleLogin Aliases", "Mail", list).reads().params(
            r#"{"account": number, "offset"?: number, "limit"?: number (1..500)} — observed receiving aliases across this account's folders, excluding Deleted/Junk descendants; complete totals plus paged aliases and nextOffset. Header-reported, not authenticated; unresolved/missing-original coverage is explicit. No body loading or provider request."#),
        CommandSpec::new("mail.identity", "Inspect Message Identities", "Mail", identity).reads().params(
            r#"{"id": number} — actual transport sender/reply recipients plus separately attributed SimpleLogin receiving alias, original sender and metadata gaps. Reads retained headers only, never changes reply routing or authenticates sender claims."#),
        CommandSpec::new("view.alias", "Show Mail for Alias", "Mail", view).reads().params(
            r#"{"account": number, "alias": string} — exact header-reported receiving alias across this account's folders, excluding Deleted/Junk descendants. Changes only the view/selection; creates no physical folder and never moves messages."#),
        CommandSpec::new("aliases.rowColor", "Alias row color", "Folder Pane", row_color).params(
            r##"{"account":number,"alias":string,"color":"#RRGGBB"|null} — locally persisted exact receiving-alias tint in combined views; null restores its account colour. Display metadata only, not a claim that forwarded headers authenticate an identity."##),
    ]
}

fn list(session: &mut Session, value: &Value) -> CmdResult {
    p::only(value, &["account", "offset", "limit"])?;
    let account = p::req_id(value, "account")?;
    if session.mailbox.account(account).is_none() {
        return Err(failed!("no account with id {id}", id = account));
    }
    let offset = match value.get("offset") {
        None => 0,
        Some(number) => usize::try_from(number.as_u64().ok_or_else(|| bad("`offset` must be a whole number at least zero"))?).unwrap_or(usize::MAX),
    };
    let limit = p::id_strict(value, "limit")?.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(bad("`limit` must be between 1 and 500; use nextOffset for the remaining aliases"));
    }
    let inventory = session.aliases();
    let empty = crate::aliases::AccountAliases::default();
    let entries = inventory.accounts.get(&account).unwrap_or(&empty);
    let end = offset.saturating_add(limit as usize).min(entries.aliases.len());
    let page = entries.aliases.get(offset..end).unwrap_or_default();
    Ok(json!({"account": account, "aliases": page, "total": entries.aliases.len(),
        "nextOffset": (end < entries.aliases.len()).then_some(end),
        "messagesWithMetadata": entries.messages_with_metadata, "unresolvedAlias": entries.unresolved_alias,
        "provenance": "messageHeaders", "authenticated": false, "coverage": "downloaded retained headers; no claim of complete provider history"}))
}

fn identity(session: &mut Session, value: &Value) -> CmdResult {
    p::only(value, &["id"])?;
    let id = p::req_id(value, "id")?;
    let message = session.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    let reply = if message.reply_to.is_empty() { vec![message.from.clone()] } else { message.reply_to.clone() };
    Ok(json!({"id": id, "account": message.account, "transportFrom": message.from, "replyTo": reply,
        "simpleLogin": message.simple_login()}))
}

fn view(session: &mut Session, value: &Value) -> CmdResult {
    p::only(value, &["account", "alias"])?;
    let account = p::req_id(value, "account")?;
    if session.mailbox.account(account).is_none() {
        return Err(failed!("no account with id {id}", id = account));
    }
    let alias = p::req_str(value, "alias")?.trim();
    if alias.len() > mailcraft_model::MAX_ADDRESS || !mailcraft_model::search::is_address(alias) {
        return Err(bad("`alias` must be one complete email address"));
    }
    let folder = session
        .mailbox
        .folders
        .iter()
        .find(|folder| folder.account == account && folder.kind == mailcraft_model::FolderKind::Inbox)
        .ok_or_else(|| fail("the account has no Inbox to anchor its alias view"))?
        .id;
    session.show_whole_folder();
    session.view.filing = None;
    session.sel.clear_messages();
    session.view.folder = folder;
    session.view.module = Module::Mail;
    session.view.scope = Scope::Mailbox;
    session.view.search = format!("alias:{}", mailcraft_model::search::keyword_value(alias));
    Ok(json!({"account": account, "alias": alias, "found": crate::list::listed(&session.rows()), "mutated": false}))
}

fn row_color(session: &mut Session, value: &Value) -> CmdResult {
    p::only(value, &["account", "alias", "color"])?;
    let account = p::req_id(value, "account")?;
    let alias = p::req_str(value, "alias")?.trim();
    if alias.len() > mailcraft_model::MAX_ADDRESS || !mailcraft_model::search::is_address(alias) {
        return Err(bad("alias must be one complete email address"));
    }
    let alias = alias.to_ascii_lowercase();
    let color = match value.get("color") {
        Some(Value::Null) => None,
        Some(Value::String(text)) => Some(mailcraft_model::Rgb::parse(text).ok_or_else(|| bad("color is #RRGGBB or null"))?),
        _ => return Err(bad("color is #RRGGBB or null")),
    };
    let account = session.mailbox.account_mut(account)?;
    if account.kind == mailcraft_model::AccountKind::Unknown {
        return Err(bad("preserve the settings of this unsupported account"));
    }
    if let Some(color) = color {
        if account.alias_row_colors.len() >= 10_000 && !account.alias_row_colors.contains_key(&alias) {
            return Err(bad("too many saved alias colours"));
        }
        account.alias_row_colors.insert(alias.clone(), color);
    } else {
        account.alias_row_colors.remove(&alias);
    }
    Ok(json!({"account":account.id,"alias":alias,"color":account.origin_row_color(Some(&alias)).hex(),"custom":color.is_some()}))
}
