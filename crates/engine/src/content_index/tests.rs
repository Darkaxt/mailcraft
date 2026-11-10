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
