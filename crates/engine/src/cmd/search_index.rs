// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only coverage and located matches. Tokens preserve the original query
//! until completion/close; index retry changes only reproducible derived data.

use serde_json::Value;

#[cfg(not(target_arch = "wasm32"))]
use super::bad;
use super::fail;
#[cfg(not(target_arch = "wasm32"))]
use crate::p;
use crate::{CmdResult, CommandSpec, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("search.indexStatus","Search Index Coverage","Search",status).reads(),
        CommandSpec::new("search.sources","Where Search Text Matched","Search",sources).reads()
            .params(r#"{"id":number,"query"?:string,"offset"?:number,"limit"?:number (1..100),"token"?:number,"close"?:bool}; repeat the same original parameters and token until ready, then close it; no selection or mailbox change"#),
        CommandSpec::new("search.gaps","Search Coverage Gaps","Search",gaps).reads()
            .params(r#"{"account"?:number,"offset"?:number,"limit"?:number (1..100),"token"?:number,"close"?:bool}; bounded detail pages with exact aggregate counts"#),
        CommandSpec::new("search.coverageDetails","Inspect Native Extraction Coverage","Search",coverage).reads()
            .params(r#"{"id":number,"attachment"?:number,"field"?:string,"section"?:"gaps"|"sources","offset"?:number,"limit"?:number,"token"?:number,"close"?:bool}; select an attachment or retained body field, follow nextOffset, close each original token"#),
        CommandSpec::new("search.retryIndex","Retry Missing or Failed Search Content","Search",retry).reads()
            .params("{}; re-read current missing/failed sources; does not move, delete, download or send mail"),
    ]
}

fn status(s: &mut Session, _: &Value) -> CmdResult {
    s.poll_content_index();
    Ok(s.content_index_status())
}

fn retry(s: &mut Session, _: &Value) -> CmdResult {
    s.retry_content_index().map_err(fail)
}

#[cfg(not(target_arch = "wasm32"))]
fn page(v: &Value) -> Result<(usize, usize), crate::CmdError> {
    let offset = usize::try_from(p::u64(v, "offset").unwrap_or(0)).map_err(|_| bad("the inspection offset is too large"))?;
    let limit = usize::try_from(p::u64(v, "limit").unwrap_or(20)).map_err(|_| bad("the inspection limit is too large"))?;
    if !(1..=100).contains(&limit) {
        return Err(bad("the inspection limit is within 1..100"));
    }
    Ok((offset, limit))
}

#[cfg(not(target_arch = "wasm32"))]
fn inspect(s: &mut Session, v: &Value, request: crate::content_index::DetailRequest) -> CmdResult {
    s.content_index_detail(request, p::u64(v, "token"), p::bool(v, "close").unwrap_or(false)).map_err(fail)
}

fn sources(s: &mut Session, v: &Value) -> CmdResult {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let document = p::req_id(v, "id")?;
        if p::bool(v, "close") != Some(true) && !s.mailbox.messages.contains_key(&document) {
            return Err(bad("the requested message is no longer in this mailbox"));
        }
        let query = mailcraft_model::search::Query::parse(p::str(v, "query").unwrap_or(&s.view.search)).on(s.today(), s.prefs.week_start);
        if let Some(why) = query.error() {
            return Err(bad(why));
        }
        let (offset, limit) = page(v)?;
        let options = s.view.search_options.clone();
        inspect(s, v, crate::content_index::DetailRequest::Sources { document, query, options, offset, limit })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, v);
        Err(fail("located persistent search requires the native app"))
    }
}

fn gaps(s: &mut Session, v: &Value) -> CmdResult {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let account = p::u64(v, "account");
        if p::bool(v, "close") != Some(true) && account.is_some_and(|id| !s.mailbox.accounts.iter().any(|account| account.id == id)) {
            return Err(bad("the requested account is no longer in this mailbox"));
        }
        let (offset, limit) = page(v)?;
        inspect(s, v, crate::content_index::DetailRequest::Gaps { account, offset, limit })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, v);
        Err(fail("persistent search coverage requires the native app"))
    }
}

fn coverage(s: &mut Session, v: &Value) -> CmdResult {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let document = p::req_id(v, "id")?;
        let message = s.mailbox.messages.get(&document);
        let closing = p::bool(v, "close") == Some(true);
        if !closing && message.is_none() {
            return Err(bad("the requested message is no longer in this mailbox"));
        }
        let attachment = p::u64(v, "attachment").map(usize::try_from).transpose().map_err(|_| bad("the attachment index is too large"))?;
        let field = p::str(v, "field").map(ToOwned::to_owned);
        if attachment.is_some() == field.is_some() {
            return Err(bad("select one attachment index or retained body field"));
        }
        if !closing && attachment.is_some_and(|index| message.is_none_or(|message| index >= message.attachments.len())) {
            return Err(bad("the requested attachment no longer exists"));
        }
        let section = p::str(v, "section").unwrap_or("gaps").to_owned();
        if !matches!(section.as_str(), "gaps" | "sources") {
            return Err(bad("the detail section is gaps or sources"));
        }
        let (offset, limit) = page(v)?;
        inspect(s, v, crate::content_index::DetailRequest::Coverage { document, attachment, field, section, offset, limit })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, v);
        Err(fail("native extraction coverage requires the desktop app"))
    }
}
