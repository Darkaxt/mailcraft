// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you's commands ([`crate::for_you`]): its settings, Show all, and forgetting the links
//! opened.

use serde_json::{Value, json};

use super::bad;
use crate::for_you::ForYouPrefs;
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("forYou.settings", "For You Settings", "View › Arrangement", settings).reads().params(
            r#"{"allMail"?: bool, "inbox"?: bool, "maxDays"?: 1-3650 (only mail of the last that many days), "newSender"?: bool, "newSenderDays"?: 1-3650, "replied"?: bool, "repliedLast"?: 1-1000, "links"?: bool, "linksDays"?: 1-3650, "read"?: bool, "readDays"?: 1-3650, "cap"?: 0-500 (0: no cap), "ownNames"?: [string], "folders"?: [{"folder": id, "always"?: bool, "maxAgeMinutes"?: number | null}]} — the For you section at the top of All mail and the Inbox: unread mail from new senders, people you reply to, senders whose links you open and senders you read, and in the Inbox unread mail filed into other folders (those with "always" every time, those with "maxAgeMinutes" until that old). What is not given stays as it is; numbers out of bounds are brought within them. Answers the settings in force"#,
        ),
        CommandSpec::new("forYou.showAll", "Show All For You", "View › Arrangement", show_all)
            .reads()
            .params(r#"{"value"?: bool} (default: toggle) — the For you section lists every message that qualifies, not only the first of the cap"#),
        CommandSpec::new("forYou.clearLinks", "Clear Link History", "View › Arrangement", clear_links).params(
            r#"{} — forgets which messages had a link opened from MailCraft (For you's "Links you open"). Answers {"cleared": how many messages had one}"#,
        ),
    ]
}

fn settings(s: &mut Session, v: &Value) -> CmdResult {
    if let Some(given) = v.as_object().filter(|o| !o.is_empty()) {
        let mut now = serde_json::to_value(&s.prefs.for_you).map_err(|e| bad(e.to_string()))?;
        let known = serde_json::to_value(ForYouPrefs { own_names: vec![String::new()], folders: vec![Default::default()], ..Default::default() })
            .map_err(|e| bad(e.to_string()))?;
        let Some(obj) = now.as_object_mut() else { return Err(bad("For you's settings are not an object")) };
        for (key, value) in given {
            if known.get(key).is_none() {
                return Err(bad(format!("`{key}` is not one of For you's settings")));
            }
            obj.insert(key.clone(), value.clone());
        }
        let mut prefs: ForYouPrefs = serde_json::from_value(now).map_err(|e| bad(format!("For you's settings: {e}")))?;
        if let Some(missing) = prefs.folders.iter().find(|f| s.mailbox.folder(f.folder).is_none()) {
            return Err(bad(format!("no folder with id {}", missing.folder)));
        }
        prefs.tidy();
        s.prefs.for_you = prefs;
    }
    serde_json::to_value(&s.prefs.for_you).map_err(|e| bad(e.to_string()))
}

fn show_all(s: &mut Session, v: &Value) -> CmdResult {
    s.view.for_you_all = p::bool(v, "value").unwrap_or(!s.view.for_you_all);
    Ok(json!({"value": s.view.for_you_all}))
}

fn clear_links(s: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({"cleared": s.clear_link_history()}))
}
