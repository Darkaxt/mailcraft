// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real store/index/search boundaries. Written with the feature; executed only
//! once its complete wiring and package are ready for stage-end validation.

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use mailcraft_model::{Account, Address, Attachment, Body, ContentRef, FolderKind, Mailbox, Message};
use serde_json::{Value, json};

use crate::{Scope, Session, io::ContentSource};

struct Counting {
    inner: Arc<dyn ContentSource>,
    opens: Arc<AtomicUsize>,
}

impl ContentSource for Counting {
    fn fetch(&self, path: &Path, reference: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch(path, reference)
    }
    fn open(&self, path: &Path, reference: &ContentRef) -> Result<Box<dyn std::io::Read + Send>, String> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.inner.open(path, reference)
    }
}

fn open(path: &Path) -> (Session, Arc<AtomicUsize>) {
    crate::store::clear_cache();
    let mailbox = crate::store::with_budget(1, || crate::store::open(path)).unwrap().mailbox;
    let mut session = Session::new(mailbox);
    session.path = Some(path.to_path_buf());
    session.index_words = true;
    let opens = Arc::new(AtomicUsize::new(0));
    session.content_source = Some(Arc::new(Counting { inner: session.source().unwrap(), opens: opens.clone() }));
    (session, opens)
}

fn search(session: &mut Session, query: &str) -> Vec<u64> {
    session.view.scope = Scope::Everywhere;
    session.view.search = query.into();
    session.wait_for_word_index();
    let mut ids: Vec<_> = session.rows().iter().map(|row| row.id).collect();
    ids.sort_unstable();
    ids
}

fn fixture(path: &Path) -> (u64, u64, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery", "avery@example.org");
    let first = mailbox.accounts[0].id;
    let second = mailbox.add_account(Account { name: "Other account".into(), email: "other@example.net".into(), ..Default::default() });
    let folder =
        |mailbox: &Mailbox, account| mailbox.folders.iter().find(|folder| folder.account == account && folder.kind == FolderKind::Inbox).unwrap().id;
    let id = mailbox
        .add_message(Message {
            account: first,
            folder: folder(&mailbox, first),
            subject: "Case and complete content".into(),
            from: Address::new("Receipts", "receipts@example.org"),
            to: vec![Address::new("Avery", "avery@example.org")],
            body: Body {
                text: format!("First line. A Café invoice, precisely!\nΚαλημέρα κόσμε{}", " ".repeat(9000)),
                html: Some("<p>First line.</p><span hidden>InvisibleViolet</span>".into()),
            },
            headers: vec![("X-Receipt-Trace".into(), "HeaderOnlyAster".into())],
            attachments: vec![Attachment::new(
                "receipts.txt",
                "text/plain",
                format!("AttachmentOnlyCopper and exact, punctuation!{}", " ".repeat(5000)).into_bytes(),
            )],
            ..Default::default()
        })
        .unwrap();
    let other = mailbox
        .add_message(Message {
            account: second,
            folder: folder(&mailbox, second),
            subject: "Other account".into(),
            body: Body { text: "OtherAccountQuartz".into(), html: None },
            ..Default::default()
        })
        .unwrap();
    crate::store::save(path, &mailbox).unwrap();
    (id, other, second)
}

#[test]
fn original_parts_stream_from_memory_or_verified_storage_without_loading_display_content() {
    use std::io::Read;

    use super::input::{EXTRACTOR_VERSION, Input, reference};
    use crate::Part;

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, _, _) = fixture(&path);
    let (mut session, _) = open(&path);
    let input = Input::new(&session, id, EXTRACTOR_VERSION).unwrap();
    assert!(input.open(Part::Original).is_err());
    let bytes = b"Content-Type: application/pkcs7-mime\r\n\r\nExact original bytes";
    let message = Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap());
    message.original = mailcraft_model::Original::new(bytes.to_vec());
    let expected = reference(message, Part::Original).unwrap();
    let input = Input::new(&session, id, EXTRACTOR_VERSION).unwrap();
    let mut read = Vec::new();
    input.open(Part::Original).unwrap().read_to_end(&mut read).unwrap();
    assert_eq!(read, bytes);
    let kept = crate::store::stash(&path, &crate::store::Digest::of(bytes), bytes).unwrap();
    assert_eq!(kept, expected);
    let original = Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap()).original.as_mut().unwrap();
    original.kept = Some(kept);
    original.data = Arc::new(Vec::new());
    let input = Input::new(&session, id, EXTRACTOR_VERSION).unwrap();
    assert_eq!(reference(&input.message, Part::Original).unwrap(), expected);
    let mut read = Vec::new();
    input.open(Part::Original).unwrap().read_to_end(&mut read).unwrap();
    assert_eq!(read, bytes);
    assert!(session.mailbox.message(id).unwrap().original.as_ref().unwrap().data.is_empty());
    let hash = crate::store::Digest::of(bytes).hex();
    let mut corrupt = bytes.to_vec();
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(crate::store::blob_dir(&path).join(&hash[..2]).join(hash), corrupt).unwrap();
    let mut read = Vec::new();
    assert!(input.open(Part::Original).unwrap().read_to_end(&mut read).is_err());
}

#[test]
fn persistent_full_sources_case_quotes_and_snippets_survive_reopen_without_body_reads() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, other, _) = fixture(&path);
    let (mut session, opens) = open(&path);
    for query in [
        "HeaderOnlyAster",
        "InvisibleViolet",
        "attachmenttext:AttachmentOnlyCopper",
        "\"Café invoice, precisely!\"",
        "case:sensitive \"Καλημέρα κόσμε\"",
    ] {
        assert_eq!(search(&mut session, query), [id], "{query}");
    }
    assert!(search(&mut session, "case:sensitive invisibleviolet").is_empty());
    assert_eq!(search(&mut session, "case:sensitive from:receipts@example.org HeaderOnlyAster"), [id]);
    assert!(search(&mut session, "case:sensitive from:Receipts@example.org HeaderOnlyAster").is_empty());
    assert!(search(&mut session, "\"Café invoice precisely\"").is_empty());
    assert_eq!(search(&mut session, "OtherAccountQuartz"), [other]);
    assert!(opens.load(Ordering::Relaxed) > 0, "indexing consumed retained sources");
    opens.store(0, Ordering::Relaxed);
    let listed = session.run("mail.list", &json!({"query":"HeaderOnlyAster","limit":10})).unwrap();
    assert!(listed["messages"].as_array().unwrap().iter().all(|row| row["snippet"].as_str().is_some_and(|text| !text.is_empty())));
    assert_eq!(opens.load(Ordering::Relaxed), 0, "listing uses persisted previews, not full bodies");
    drop(session);
    let (mut session, opens) = open(&path);
    assert_eq!(search(&mut session, "attachmenttext:AttachmentOnlyCopper"), [id]);
    assert_eq!(opens.load(Ordering::Relaxed), 0, "verified immutable source parts and previews are reused after restart");
}

#[test]
fn saved_header_and_attachment_queries_reopen_with_exact_native_scope_without_body_reads() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, other, second) = fixture(&path);
    let (mut session, opens) = open(&path);
    search(&mut session, "HeaderOnlyAster");
    let anchor = session.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let wanted = session
        .run("folder.newSearchFolder", &json!({"name":"Invoice attachment","query":"attachmenttext:\"exact, punctuation!\"","scope":"inboxes"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let excluded = session
        .run("folder.newSearchFolder", &json!({"name":"Other mailbox only","query":"HeaderOnlyAster","scope":"mailbox","folder":anchor}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    opens.store(0, Ordering::Relaxed);
    session.run("searchFolders.select", &json!({"id":wanted})).unwrap();
    session.wait_for_word_index();
    assert_eq!(session.rows().iter().map(|row| row.id).collect::<Vec<_>>(), [id]);
    session.run("searchFolders.select", &json!({"id":excluded})).unwrap();
    session.wait_for_word_index();
    assert!(session.rows().is_empty(), "a header hit in another account cannot escape the saved mailbox binding");
    assert_eq!(opens.load(Ordering::Relaxed), 0, "saved searches use indexed sources");
    crate::store::save(&path, &session.mailbox).unwrap();
    drop(session);
    let (mut session, opens) = open(&path);
    assert_eq!(session.mailbox.search_folders.len(), 2);
    session.run("searchFolders.select", &json!({"id":wanted})).unwrap();
    session.wait_for_word_index();
    assert_eq!(session.rows().iter().map(|row| row.id).collect::<Vec<_>>(), [id]);
    assert!(!session.rows().iter().any(|row| row.id == other));
    assert_eq!(opens.load(Ordering::Relaxed), 0, "reopened definitions reuse retained index content");
}

#[test]
fn edits_removal_and_account_scopes_cannot_reuse_stale_hits() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, other, second) = fixture(&path);
    let (mut session, _) = open(&path);
    assert_eq!(search(&mut session, "OtherAccountQuartz"), [other]);
    let first_folder = session.mailbox.message(id).unwrap().folder;
    session.view.folder = first_folder;
    session.view.scope = Scope::Folder;
    session.view.search = "OtherAccountQuartz".into();
    session.wait_for_word_index();
    assert!(session.rows().is_empty(), "the other account's hit cannot escape a folder scope");
    Arc::make_mut(session.mailbox.messages.get_mut(&other).unwrap()).body.text = "ReplacementJasper".into();
    session.mailbox_changed();
    assert!(search(&mut session, "OtherAccountQuartz").is_empty());
    assert_eq!(search(&mut session, "ReplacementJasper"), [other]);
    session.mailbox.remove_account(second).unwrap();
    session.mailbox_changed();
    assert!(search(&mut session, "ReplacementJasper").is_empty());
    assert_eq!(search(&mut session, "AttachmentOnlyCopper"), [id]);
}

#[test]
fn missing_content_has_explicit_coverage_and_never_proves_a_negative_match() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, _, _) = fixture(&path);
    let (mut session, _) = open(&path);
    Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap()).unfetched = true;
    session.mailbox_changed();
    assert!(!search(&mut session, "-UnseenKeyword").contains(&id));
    assert!(session.unsearched > 0);
    let status = session.content_index_status();
    assert_eq!(status["complete"], false);
    let accounts = status["coverage"]["accounts"].as_array().unwrap();
    assert!(accounts.iter().any(|account| account["downloaded"].as_u64().unwrap() < account["discovered"].as_u64().unwrap()));
}

#[test]
fn detail_tokens_are_bound_to_the_original_request_and_released_after_completion() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, _, _) = fixture(&path);
    let (mut session, _) = open(&path);
    search(&mut session, "AttachmentOnlyCopper");
    let params = json!({"id":id,"query":"AttachmentOnlyCopper","limit":1});
    let first = session.run("search.sources", &params).unwrap();
    let token = first["token"].as_u64().unwrap();
    session.wait_for_word_index();
    let mut original = params.clone();
    original["token"] = json!(token);
    let ready = session.run("search.sources", &original).unwrap();
    assert_eq!(ready["state"], "ready");
    assert!(ready["result"]["totalSources"].as_u64().unwrap() > 0);
    let mut wrong = original.clone();
    wrong["query"] = json!("different");
    assert!(session.run("search.sources", &wrong).is_err());
    original["close"] = Value::Bool(true);
    assert_eq!(session.run("search.sources", &original).unwrap()["state"], "closed");
}

#[test]
fn attachment_parts_cached_before_download_are_not_reused_after_download_completes() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (id, _, _) = fixture(&path);
    let (mut session, _) = open(&path);
    Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap()).unfetched = true;
    session.mailbox_changed();
    assert!(!search(&mut session, "AttachmentOnlyCopper").contains(&id));
    Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap()).unfetched = false;
    session.mailbox_changed();
    assert_eq!(search(&mut session, "AttachmentOnlyCopper"), [id], "the incomplete pre-download attachment part is not reused");
}

#[test]
fn extraction_panics_are_reported_as_text_for_that_document_only() {
    let message = std::panic::catch_unwind(|| panic!("decoder exploded")).unwrap_err();
    assert_eq!(super::documents::panic_text(&*message), "decoder exploded");
    let formatted = std::panic::catch_unwind(|| panic!("decoder {}", 7)).unwrap_err();
    assert_eq!(super::documents::panic_text(&*formatted), "decoder 7");
    let other = std::panic::catch_unwind(|| std::panic::panic_any(7u8)).unwrap_err();
    assert_eq!(super::documents::panic_text(&*other), "unknown panic");
}

#[cfg(windows)]
#[test]
fn deterministic_allowance_and_deadline_failures_are_resource_limits_not_retried_failures() {
    use super::extract::failure;
    assert_eq!(
        failure("resourceLimit: extracted native/OCR text exceeds the 256 MiB containing-attachment allowance; original OCR PID 1"),
        "resourceLimit"
    );
    assert_eq!(failure("resourceLimit: the native core did not answer within its 120-second response deadline and was ended"), "resourceLimit");
    assert_eq!(failure("the original OCR process failed"), "failed");
}

/// A mailbox of messages each with `subject` and `body` in the Inbox, and
/// `junk`/`deleted` bodies in Junk E-mail and Deleted Items.
fn words_fixture(path: &Path, bodies: &[(&str, &str)], junk: &str, deleted: &str) -> (Vec<u64>, u64, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery", "avery@example.org");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let ids = bodies
        .iter()
        .map(|(subject, body)| {
            mailbox
                .add_message(Message {
                    account,
                    folder: inbox,
                    subject: (*subject).into(),
                    body: Body { text: (*body).into(), html: None },
                    ..Default::default()
                })
                .unwrap()
        })
        .collect();
    let mut elsewhere = |kind, text: &str| {
        let folder = mailbox.folder_of_kind(account, kind).unwrap();
        mailbox
            .add_message(Message { account, folder, subject: "Elsewhere".into(), body: Body { text: text.into(), html: None }, ..Default::default() })
            .unwrap()
    };
    let junk = elsewhere(FolderKind::Junk, junk);
    let deleted = elsewhere(FolderKind::Deleted, deleted);
    crate::store::save(path, &mailbox).unwrap();
    (ids, junk, deleted)
}

fn with_options(session: &mut Session, options: Value) {
    session.run("view.searchOptions", &options).unwrap();
}

#[test]
fn substrings_expand_through_the_vocabulary_and_terms_are_anded() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (ids, _, _) = words_fixture(
        &path,
        &[("Monthly", "Please pay the invoice today"), ("Call", "A voice message for you"), ("Other", "Nothing relevant here")],
        "",
        "",
    );
    let (mut session, _) = open(&path);
    let mut both = vec![ids[0], ids[1]];
    both.sort_unstable();
    assert_eq!(search(&mut session, "voice"), both, "voice is found inside invoice");
    assert_eq!(search(&mut session, "voice pay"), [ids[0]], "free-text terms are ANDed");
    assert_eq!(search(&mut session, "VOICE"), both, "case-insensitive by default");
    assert!(search(&mut session, "zzzunknown").is_empty());
    with_options(&mut session, json!({"wholeWord": true}));
    assert_eq!(search(&mut session, "voice"), [ids[1]], "whole words do not match inside invoice");
    with_options(&mut session, json!({"wholeWord": false, "matchCase": true}));
    assert!(search(&mut session, "VOICE").is_empty(), "matchCase compares the stored text");
    assert_eq!(search(&mut session, "voice"), both);
    with_options(&mut session, json!({"matchCase": false, "regex": true}));
    assert_eq!(search(&mut session, "inv.ice"), [ids[0]]);
    assert!(session.run("view.searchOptions", &json!({"searchIn": []})).is_err(), "at least one part is searched");
    with_options(&mut session, json!({"regex": false, "searchIn": ["subject"]}));
    assert_eq!(search(&mut session, "monthly"), [ids[0]]);
    assert!(search(&mut session, "invoice").is_empty(), "the body is not searched");
}

#[test]
fn relevance_scores_whole_words_above_containing_words_and_ties_by_date() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (ids, _, _) = words_fixture(&path, &[("A", "an invoice"), ("B", "a voice"), ("C", "voiced it"), ("D", "the voice again")], "", "");
    let (mut session, _) = open(&path);
    session.view.sort = crate::SortBy::Relevance;
    session.view.sort_descending = true;
    search(&mut session, "voice");
    let rows = session.rows();
    let scored: Vec<_> = rows.iter().map(|row| (row.id, row.score)).collect();
    let score = |id| scored.iter().find(|(row, _)| *row == id).and_then(|(_, score)| *score);
    assert_eq!(score(ids[1]), Some(100));
    assert_eq!(score(ids[3]), Some(100));
    assert_eq!(score(ids[2]), Some(83), "voice in voiced: 5 of 6");
    assert_eq!(score(ids[0]), Some(71), "voice in invoice: 5 of 7");
    let order: Vec<_> = scored.iter().map(|(id, _)| *id).collect();
    let position = |id| order.iter().position(|row| *row == id).unwrap();
    assert!(position(ids[1]).max(position(ids[3])) < position(ids[2]));
    assert!(position(ids[2]) < position(ids[0]));
    let detail = &rows.iter().find(|row| row.id == ids[0]).unwrap().score_detail;
    assert_eq!(detail.first().map(|detail| detail.matched.as_str()), Some("invoice"));
}

#[test]
fn junk_and_deleted_items_are_not_indexed_but_still_searched() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (_, junk, deleted) = words_fixture(&path, &[("Inbox", "plain")], "JunkOnlyMarker", "DeletedOnlyMarker");
    let (mut session, _) = open(&path);
    search(&mut session, "plain");
    let stored = super::db::Database::reader(&crate::store::content_index_file(&path)).unwrap().document_ids().unwrap();
    assert!(!stored.contains(&junk) && !stored.contains(&deleted), "junk and deleted mail stay out of the sidecar");
    // (All Mailboxes leaves those folders out: each is searched in the folder itself.)
    let in_folder = |session: &mut Session, id: u64, query: &str| {
        let folder = session.mailbox.message(id).unwrap().folder;
        session.run("folder.select", &serde_json::json!({"id": folder})).unwrap();
        session.view.scope = Scope::Folder;
        session.view.search = query.into();
        session.wait_for_word_index();
        session.rows().iter().map(|row| row.id).collect::<Vec<_>>()
    };
    assert_eq!(in_folder(&mut session, junk, "JunkOnlyMarker"), [junk], "read from the message itself");
    assert_eq!(in_folder(&mut session, deleted, "DeletedOnlyMarker"), [deleted]);
}

#[test]
fn an_owned_older_schema_sidecar_is_rebuilt_and_a_foreign_file_preserved() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let (ids, _, _) = words_fixture(&path, &[("Old", "rebuilt content")], "", "");
    let sidecar = crate::store::content_index_file(&path);
    {
        let old = rusqlite::Connection::open(&sidecar).unwrap();
        old.execute_batch("CREATE TABLE units(folded TEXT); PRAGMA application_id=1296259121; PRAGMA user_version=1;").unwrap();
    }
    let (mut session, _) = open(&path);
    assert_eq!(search(&mut session, "rebuilt"), [ids[0]]);
    let version: i64 = rusqlite::Connection::open(&sidecar).unwrap().pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    assert_eq!(version, super::db::SCHEMA);
    drop(session);
    let foreign = scratch.path().join("foreign.sqlite");
    rusqlite::Connection::open(&foreign).unwrap().execute_batch("CREATE TABLE keep(x); PRAGMA user_version=1;").unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    assert!(super::db::Database::writer(&foreign, stop).is_err());
    assert!(foreign.exists(), "a file that is not an owned index is never deleted");
}

#[test]
fn cancelling_the_index_stops_without_waiting_and_is_not_reopened() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("mailbox.mailcraft");
    let bodies: Vec<(String, String)> =
        (0..200).map(|n| (format!("Message {n}"), format!("body text number {n} {}", "filler ".repeat(500)))).collect();
    let borrowed: Vec<(&str, &str)> = bodies.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    words_fixture(&path, &borrowed, "", "");
    let (mut session, _) = open(&path);
    session.view.search = "filler".into();
    session.poll_content_index();
    let started = std::time::Instant::now();
    session.cancel_content_index();
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "close does not wait for the build");
    session.poll_content_index();
    assert!(!session.content_index_busy(), "the index is not reopened while the window closes");
}

#[test]
fn declared_zip_expansion_is_refused_before_import() {
    use super::extract::zip_bomb;
    // One central-directory entry declaring 1 GiB from 1 KiB.
    let mut zip = b"PK\x03\x04".to_vec();
    zip.resize(64, 0);
    let directory = zip.len();
    let mut entry = vec![0u8; 46];
    entry[..4].copy_from_slice(b"PK\x01\x02");
    entry[20..24].copy_from_slice(&1024u32.to_le_bytes());
    entry[24..28].copy_from_slice(&(1u32 << 30).to_le_bytes());
    zip.extend(entry);
    let mut end = vec![0u8; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    end[10..12].copy_from_slice(&1u16.to_le_bytes());
    end[16..20].copy_from_slice(&u32::try_from(directory).unwrap().to_le_bytes());
    zip.extend(end);
    assert!(zip_bomb(&zip).is_some());
    assert!(zip_bomb(b"plain text").is_none());
}
