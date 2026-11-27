// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you ([`crate::for_you`]): its tiers, its section in All mail and the Inbox, the
//! record of opened links, and its commands.

use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::for_you::{ForYouFolder, ForYouPrefs, HEADING, Own, SenderStats, Tier, is_bulk, self_triggered, tier};
use crate::Session;

const NOW: i64 = 1_791_993_600;
const DAY: i64 = 86_400;

/// The sample mailbox with every message of it read, so that only the mail a test adds is
/// unread; by date, in groups, a message to a row.
fn session() -> Session {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let ids: Vec<Id> = s.mailbox.messages.values().map(|m| m.id).collect();
    for id in ids {
        s.mailbox.message_mut(id).unwrap().read = true;
    }
    s.prefs.show_groups = true;
    s.prefs.sender_groups = false;
    s.view.conversations = false;
    s.prefs.for_you.all_mail = true;
    s.prefs.for_you.inbox = true;
    s
}

fn message(from: &str, days_ago: i64, read: bool) -> Message {
    let at = NOW - days_ago * DAY;
    Message {
        from: Address { name: String::new(), email: from.into() },
        subject: format!("from {from}, {days_ago} days ago"),
        date: at,
        received: at,
        read,
        ..Default::default()
    }
}

fn add(s: &mut Session, folder: Id, m: Message) -> Id {
    let id = s.mailbox.add_message(Message { folder, ..m }).unwrap();
    s.rev += 1;
    id
}

fn inbox(s: &Session) -> Id {
    s.mailbox.inbox().unwrap()
}

/// The ids of the rows of the section, in order, and what each is there for.
fn section(s: &mut Session) -> Vec<(Id, Tier)> {
    s.rows().iter().filter_map(|r| r.for_you.map(|t| (r.id, t))).collect()
}

fn tier_of(messages: &[Message], m: &Message) -> Option<Tier> {
    let p = ForYouPrefs::default();
    let stats = SenderStats::of(messages.iter().chain(std::iter::once(m)), NOW, &p);
    tier(m, &stats, &p, &Own::default(), NOW)
}

fn bulk(mut m: Message) -> Message {
    m.headers.push(("List-Unsubscribe".into(), "<mailto:leave@news.example>".into()));
    m
}

#[test]
fn a_first_mail_from_a_person_is_a_new_sender() {
    assert_eq!(tier_of(&[], &message("ana@people.example", 1, false)), Some(Tier::NewSender));
}

#[test]
fn a_sender_first_seen_over_a_month_ago_is_not_new() {
    let before = [message("ana@people.example", 40, false)];
    assert_eq!(tier_of(&before, &message("ana@people.example", 1, false)), None);
}

#[test]
fn a_first_newsletter_is_not_a_new_sender() {
    assert_eq!(tier_of(&[], &bulk(message("news@news.example", 1, false))), None);
    let mut bot = message("ci@github.example", 1, false);
    bot.from.name = "codex[bot]".into();
    assert_eq!(tier_of(&[], &bot), None);
    assert!(is_bulk(&message("noreply@shop.example", 1, false)));
}

#[test]
fn a_newsletter_he_reads_qualifies_as_read() {
    let before: Vec<Message> = (0..4).map(|i| bulk(message("news@news.example", 50 - i * 10, i != 0))).collect();
    // 3 of the 5 of the last two months read.
    assert_eq!(tier_of(&before, &bulk(message("news@news.example", 1, false))), Some(Tier::Read));
}

#[test]
fn half_read_is_not_usually() {
    let before = [bulk(message("news@news.example", 50, true))];
    // 1 of 2 read: not more than half.
    assert_eq!(tier_of(&before, &bulk(message("news@news.example", 1, false))), None);
}

#[test]
fn one_answer_in_the_last_ten_keeps_replied() {
    let mut before: Vec<Message> = (1..=9).map(|i| message("bo@people.example", 40 + i, false)).collect();
    if let Some(m) = before.last_mut() {
        m.answered = true;
    }
    // The answered one is the 10th newest, counting the one in question.
    assert_eq!(tier_of(&before, &message("bo@people.example", 1, false)), Some(Tier::Replied));
}

#[test]
fn an_answer_eleven_messages_back_does_not() {
    let mut before: Vec<Message> = (1..=10).map(|i| message("bo@people.example", 40 + i, false)).collect();
    if let Some(m) = before.last_mut() {
        m.answered = true;
    }
    assert_ne!(tier_of(&before, &message("bo@people.example", 1, false)), Some(Tier::Replied));
}

#[test]
fn links_opened_in_most_of_the_last_two_months() {
    let before: Vec<Message> = (0..3)
        .map(|i| {
            let mut m = bulk(message("deals@shop.example", 40 - i * 10, false));
            m.link_opened = Some(NOW - 30 * DAY);
            m
        })
        .collect();
    // 3 of 4 opened, none read.
    assert_eq!(tier_of(&before, &bulk(message("deals@shop.example", 1, false))), Some(Tier::Links));
}

#[test]
fn the_highest_tier_wins() {
    let mut before: Vec<Message> = (0..3).map(|i| message("cy@people.example", 50 - i, true)).collect();
    before[0].answered = true;
    assert_eq!(tier_of(&before, &message("cy@people.example", 1, false)), Some(Tier::Replied));
}

#[test]
fn read_mail_never_qualifies() {
    assert_eq!(tier_of(&[], &message("ana@people.example", 1, true)), None);
}

#[test]
fn his_own_github_activity_never_qualifies() {
    let mut mb = crate::sample::mailbox(NOW, 0);
    mb.accounts[0].email = "darkaxt.remaxku@gmail.com".into();
    let own = Own::of(&mb, &ForYouPrefs::default());
    let mut push = message("notifications@github.example", 1, false);
    push.from.name = "Darkaxt".into();
    assert!(self_triggered(&push, &own));
    assert!(self_triggered(&message("Darkaxt.Remaxku@gmail.com", 1, false), &own));
    let mut other = push.clone();
    other.from.name = "Someone Else".into();
    assert!(!self_triggered(&other, &own));
    // Names he sets replace those of his addresses.
    let own = Own::of(&mb, &ForYouPrefs { own_names: vec!["Jose Miguel".into()], ..Default::default() });
    other.from.name = "jose miguel".into();
    assert!(self_triggered(&other, &own) && !self_triggered(&push, &own));
}

#[test]
fn all_mail_lists_for_you_above_the_dates() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    let new = add(&mut s, inbox, message("ana@people.example", 1, false));
    let rows = s.rows();
    let first = rows.first().unwrap();
    assert_eq!((first.id, first.group.as_deref(), first.for_you), (new, Some(HEADING), Some(Tier::NewSender)));
    // The next row starts a date heading of its own, and no other row is the section's.
    let second = rows.get(1).unwrap();
    assert!(second.for_you.is_none() && second.group.is_some());
    assert_eq!(rows.iter().filter(|r| r.for_you.is_some()).count(), 1);
}

#[test]
fn a_message_in_for_you_is_not_listed_again_below() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    let new = add(&mut s, inbox, message("ana@people.example", 1, false));
    assert_eq!(s.rows().iter().filter(|r| r.id == new || r.members.contains(&new)).count(), 1);
}

#[test]
fn reading_it_drops_it_from_for_you() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    let new = add(&mut s, inbox, message("ana@people.example", 1, false));
    assert_eq!(section(&mut s), vec![(new, Tier::NewSender)]);
    s.mailbox.message_mut(new).unwrap().read = true;
    s.rev += 1;
    assert!(section(&mut s).is_empty());
}

#[test]
fn a_pinned_message_stays_in_pinned() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    let new = add(&mut s, inbox, Message { pinned: true, ..message("ana@people.example", 1, false) });
    assert!(section(&mut s).is_empty());
    let rows = s.rows();
    assert_eq!(rows.iter().filter(|r| r.id == new).count(), 1);
    assert_eq!(rows.first().map(|r| (r.id, r.group.as_deref())), Some((new, Some("Pinned"))));
}

#[test]
fn a_conversation_with_an_older_unread_qualifying_message_moves_once() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    s.view.conversations = true;
    let inbox = inbox(&s);
    let first = Message { message_id: "first@people.example".into(), subject: "Plans".into(), ..message("ana@people.example", 2, false) };
    let older = add(&mut s, inbox, first);
    let reply = Message {
        message_id: "reply@shop.example".into(),
        in_reply_to: "first@people.example".into(),
        references: vec!["first@people.example".into()],
        subject: "Re: Plans".into(),
        ..message("someone@else.example", 1, true)
    };
    let newest = add(&mut s, inbox, reply);
    let rows = s.rows();
    let listed: Vec<_> = rows.iter().filter(|r| r.members.contains(&older)).collect();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id, listed[0].for_you), (newest, Some(Tier::NewSender)));
}

#[test]
fn warned_mail_never_qualifies() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    let lookalike = |days, read| {
        let mut m = message("support@evil.example", days, read);
        m.from.name = "billing@bank.example".into();
        m
    };
    for days in [40, 30, 20] {
        add(&mut s, inbox, lookalike(days, true));
    }
    add(&mut s, inbox, lookalike(1, false));
    assert!(section(&mut s).is_empty());
}

#[test]
fn the_inbox_brings_back_mail_filed_into_switched_on_folders() {
    let mut s = session();
    s.run("view.allInboxes", &json!({})).unwrap();
    let account = s.mailbox.folder(inbox(&s)).unwrap().account;
    let travels = s.mailbox.add_folder(account, None, "Travels").unwrap();
    // From a sender who is in no tier: he has never read their mail.
    let old = add(&mut s, travels, bulk(message("tickets@air.example", 90, false)));
    let ticket = add(&mut s, travels, bulk(message("tickets@air.example", 1, false)));
    assert!(section(&mut s).is_empty());
    s.prefs.for_you.folders = vec![ForYouFolder { folder: travels, always: true, max_age_minutes: None }];
    s.rev += 1;
    // Without an age limit, all its unread mail, however old, newest first.
    assert_eq!(section(&mut s), vec![(ticket, Tier::Folder), (old, Tier::Folder)]);
    // Selecting it there keeps it selected, though it is in no Inbox.
    s.run("mail.select", &json!({"ids": [ticket]})).unwrap();
    assert_eq!(s.sel.messages, vec![ticket]);
    // All mail has no folder tier, the selection's kept place included. (Opening the ticket
    // read it, so its sender is now one he reads: other tiers may list his mail there.)
    s.run("view.allMail", &json!({})).unwrap();
    assert!(section(&mut s).iter().all(|(_, t)| *t != Tier::Folder));
}

#[test]
fn switched_on_folders_are_never_capped() {
    let mut s = session();
    s.run("view.allInboxes", &json!({})).unwrap();
    let inbox = inbox(&s);
    let account = s.mailbox.folder(inbox).unwrap().account;
    let security = s.mailbox.add_folder(account, None, "Security").unwrap();
    s.prefs.for_you.folders = vec![ForYouFolder { folder: security, always: true, max_age_minutes: None }];
    for i in 0..20 {
        add(&mut s, security, bulk(message(&format!("codes{i}@bank.example"), 2, false)));
    }
    for i in 0..20 {
        add(&mut s, inbox, message(&format!("person{i}@people.example"), 1, false));
    }
    let shown = section(&mut s);
    assert_eq!(shown.iter().filter(|(_, t)| *t == Tier::Folder).count(), 20);
    assert_eq!(shown.iter().filter(|(_, t)| *t == Tier::NewSender).count(), 15);
    // Folder mail first, though older.
    assert!(shown.iter().take(20).all(|(_, t)| *t == Tier::Folder));
    assert!(s.rows().first().is_some_and(|r| r.for_you_more));
}

#[test]
fn show_all_lifts_the_cap() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    for i in 0..20 {
        add(&mut s, inbox, message(&format!("person{i}@people.example"), 1, false));
    }
    assert_eq!(section(&mut s).len(), 15);
    s.run("forYou.showAll", &json!({"value": true})).unwrap();
    assert_eq!(section(&mut s).len(), 20);
    assert!(s.rows().first().is_some_and(|r| !r.for_you_more));
}

#[test]
fn an_otp_older_than_its_folder_limit_leaves_for_you() {
    let mut s = session();
    s.run("view.allInboxes", &json!({})).unwrap();
    let account = s.mailbox.folder(inbox(&s)).unwrap().account;
    let codes = s.mailbox.add_folder(account, None, "Codes").unwrap();
    s.prefs.for_you.folders = vec![ForYouFolder { folder: codes, always: true, max_age_minutes: Some(30) }];
    let at = |minutes: i64| Message { date: NOW - minutes * 60, received: NOW - minutes * 60, ..message("otp@bank.example", 0, false) };
    let fresh = add(&mut s, codes, at(5));
    add(&mut s, codes, at(31));
    assert_eq!(section(&mut s), vec![(fresh, Tier::Folder)]);
}

#[test]
fn no_for_you_while_searching_or_in_other_folders() {
    let mut s = session();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    add(&mut s, inbox, message("ana@people.example", 1, false));
    assert_eq!(section(&mut s).len(), 1);
    s.run("view.search", &json!({"query": "ana", "scope": "inboxes"})).unwrap();
    assert!(section(&mut s).is_empty());
    s.run("view.clearSearch", &json!({})).unwrap();
    let sent = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Sent).map(|f| f.id).unwrap();
    s.run("folder.select", &json!({"id": sent})).unwrap();
    assert!(section(&mut s).is_empty());
    // Turned off for All mail.
    s.run("view.allMail", &json!({})).unwrap();
    s.run("forYou.settings", &json!({"allMail": false})).unwrap();
    assert!(section(&mut s).is_empty());
}

#[test]
fn noting_a_link_open_keeps_the_time_and_changes_nothing_else() {
    let mut s = session();
    let inbox = inbox(&s);
    let id = add(&mut s, inbox, message("ana@people.example", 1, false));
    let before = s.mailbox.message(id).unwrap().clone();
    s.note_link_opened(id);
    let after = s.mailbox.message(id).unwrap().clone();
    assert_eq!(after.link_opened, Some(NOW));
    assert_eq!(Message { link_opened: None, ..after }, before);
}

#[test]
fn clearing_the_link_history_forgets_every_open() {
    let mut s = session();
    let inbox = inbox(&s);
    let a = add(&mut s, inbox, message("ana@people.example", 1, false));
    let b = add(&mut s, inbox, message("bo@people.example", 1, false));
    s.note_link_opened(a);
    s.note_link_opened(b);
    assert_eq!(s.run("forYou.clearLinks", &json!({})).unwrap(), json!({"cleared": 2}));
    assert!(s.mailbox.messages.values().all(|m| m.link_opened.is_none()));
}

#[test]
fn the_link_record_survives_a_save_and_reload() {
    let m = Message { link_opened: Some(NOW), ..message("ana@people.example", 1, false) };
    let back: Message = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    assert_eq!(back.link_opened, Some(NOW));
    // A message from before it was kept reads as none.
    let old: Message = serde_json::from_value(json!({"id": 1, "subject": "old"})).unwrap();
    assert_eq!(old.link_opened, None);
}

#[test]
fn for_you_settings_round_trip() {
    let mut s = session();
    let r = s.run("forYou.settings", &json!({"cap": 5, "readDays": 0, "ownNames": [" Darkaxt ", ""]})).unwrap();
    assert_eq!(r["cap"], json!(5));
    assert_eq!(s.prefs.for_you.cap, 5);
    // Kept within bounds.
    assert_eq!(s.prefs.for_you.read_days, 1);
    assert_eq!(s.prefs.for_you.own_names, vec!["Darkaxt".to_string()]);
    assert_eq!(s.run("forYou.settings", &json!({})).unwrap()["cap"], json!(5));
    assert!(s.run("forYou.settings", &json!({"cap": "many"})).is_err());
}

#[test]
fn for_you_commands_need_no_consent() {
    use crate::agent::{Access, access};
    for id in ["forYou.settings", "forYou.showAll", "view.paneAutoHide"] {
        assert_eq!(access(id), Some(Access::Safe), "{id}");
    }
}

#[test]
fn clearing_the_link_history_asks_an_agent_first() {
    use crate::agent::{Access, Class, access};
    // The history has no undo, and the "links you open" tier lives on it.
    assert_eq!(access("forYou.clearLinks"), Some(Access::Owner(Class::Delete)));
}

#[test]
fn folder_rows_are_a_row_style() {
    let mut s = session();
    assert_eq!(s.run("view.rowStyle", &json!({"value": "folder"})).unwrap(), json!({"value": "folder"}));
    assert!(s.prefs.folder_rows && !s.prefs.one_line);
    assert_eq!(s.run("view.rowStyle", &json!({"value": "extended"})).unwrap(), json!({"value": "extended"}));
    assert!(!s.prefs.folder_rows);
}

#[test]
fn the_pane_hides_itself_once_turned_on() {
    let mut s = session();
    assert!(!s.prefs.pane_auto_hide);
    assert_eq!(s.run("view.paneAutoHide", &json!({})).unwrap(), json!({"value": true}));
    let back: crate::Prefs = serde_json::from_value(serde_json::to_value(&s.prefs).unwrap()).unwrap();
    assert!(back.pane_auto_hide);
    // Settings saved before it was there have it off, and the usual rows.
    let old: crate::Prefs = serde_json::from_value(json!({})).unwrap();
    assert!(!old.pane_auto_hide && !old.folder_rows);
}

#[test]
fn off_until_turned_on() {
    let mut s = session();
    s.prefs.for_you = ForYouPrefs::default();
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    add(&mut s, inbox, message("ana@people.example", 1, false));
    assert!(section(&mut s).is_empty());
    s.run("forYou.settings", &json!({"allMail": true})).unwrap();
    assert_eq!(section(&mut s).len(), 1);
}

#[test]
fn without_groups_the_rest_has_a_heading_of_its_own() {
    let mut s = session();
    s.prefs.show_groups = false;
    s.run("view.allMail", &json!({})).unwrap();
    let inbox = inbox(&s);
    add(&mut s, inbox, message("ana@people.example", 1, false));
    let rows = s.rows();
    assert_eq!(rows.first().and_then(|r| r.group.as_deref()), Some(HEADING));
    assert_eq!(rows.get(1).and_then(|r| r.group.as_deref()), Some(crate::for_you::REST));
    assert!(rows.iter().skip(2).all(|r| r.group.is_none()));
}
