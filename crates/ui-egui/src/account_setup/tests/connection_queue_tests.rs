// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A newly connected account must keep its first download while another runs.
//! Accounts, credentials and mail hosts are synthetic; no real mailbox is used.

use super::*;

fn connected(app: &mut MailApp, address: &str) -> Id {
    let id =
        app.execute("account.add", json!({"email":address,"incoming":{"host":"imap.example.org","security":"tls"}})).unwrap()["id"].as_u64().unwrap();
    app.passwords.insert(id, "synthetic-password".into());
    id
}

#[test]
fn onboarding_waits_for_the_current_download_then_runs_the_new_account_once() {
    use mailcraft_engine::cmd::sync_progress::RunState;
    let mut app = app(false);
    let first = connected(&mut app, "first@example.org");
    let second = connected(&mut app, "second@example.org");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    app.services.sync = Some(Box::new(move |request: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        recorded.lock().unwrap().push((request.account.id, tx));
        rx
    }));
    app.execute("ui.sync", json!({"account":first})).unwrap();
    requests.lock().unwrap()[0].1.send(SyncResult { account: first, partial: true, ..Default::default() }).unwrap();
    app.poll_sync();
    let mut wizard = AccountSetup::new(&app, &json!({"email":"second@example.org"}));
    wizard.finish(&mut app, second);

    assert_eq!(wizard.outcome, Outcome::Queued);
    assert_eq!(app.sync_queue.iter().copied().collect::<Vec<_>>(), vec![(second, false)]);
    assert!(!app.services.sync_cancel.load(Ordering::Relaxed));
    assert_eq!(requests.lock().unwrap().len(), 1, "the first download is still the sole native job");
    let progress = &app.session.sync_progress.accounts;
    assert_eq!(progress.iter().find(|r| r.account == first).unwrap().state, RunState::Running);
    assert_eq!(progress.iter().find(|r| r.account == second).unwrap().state, RunState::Waiting);

    // Repeated completion notification retains one queue entry and the same job.
    wizard.finish(&mut app, second);
    assert_eq!(app.sync_queue.len(), 1);
    requests.lock().unwrap()[0].1.send(SyncResult { account: first, ..Default::default() }).unwrap();
    app.poll_sync();
    // The current job can finish while setup is still open. A third account
    // must join the existing queue, rather than jumping in front of the second.
    let third = connected(&mut app, "third@example.org");
    let mut next = AccountSetup::new(&app, &json!({"email": "third@example.org"}));
    next.finish(&mut app, third);
    assert_eq!(app.sync_queue.iter().copied().collect::<Vec<_>>(), vec![(second, false), (third, false)]);
    app.auto_sync();
    assert_eq!(requests.lock().unwrap().iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![first, second]);
    requests.lock().unwrap()[1].1.send(SyncResult { account: second, ..Default::default() }).unwrap();
    app.poll_sync();
    app.auto_sync();
    assert_eq!(requests.lock().unwrap().iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![first, second, third]);
    assert!(app.sync_queue.is_empty());
}

#[test]
fn onboarding_during_a_graph_job_keeps_the_existing_progress_and_queues_the_account() {
    use mailcraft_engine::cmd::sync_progress::RunState;
    use mailcraft_model::sync::SyncPhase;
    let mut app = app(false);
    syncs(&mut app);
    let first = connected(&mut app, "first@example.org");
    let second = connected(&mut app, "second@example.org");
    let (_held, rx) = mpsc::channel();
    app.graph_rx = Some((first, false, false, rx));
    app.session.sync_progress.begin(&[first]);
    app.session.sync_progress.phase(first, SyncPhase::NewMail);
    let original = app.session.sync_progress.accounts[0].clone();
    let mut wizard = AccountSetup::new(&app, &json!({"email":"second@example.org"}));
    wizard.finish(&mut app, second);

    assert_eq!(wizard.outcome, Outcome::Queued);
    assert_eq!(app.session.sync_progress.accounts[0], original);
    assert_eq!(app.session.sync_progress.accounts[1].state, RunState::Waiting);
    assert_eq!(app.sync_queue.pop_front(), Some((second, false)));
    assert!(app.graph_rx.is_some());
}

#[test]
fn renewed_google_sign_in_keeps_its_download_queued_behind_another_account() {
    for quiet in [false, true] {
        let mut app = app(false);
        let first = connected(&mut app, "first@example.org");
        let requested = syncs(&mut app);
        app.execute("ui.sync", json!({"account":first})).unwrap();
        let second = app
            .execute(
                "account.add",
                json!({"email":"second@example.org","auth":"oauth2","provider":"google",
                   "clientId":"synthetic.apps.googleusercontent.com",
                   "incoming":{"host":"imap.example.org","security":"tls"}}),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        sign_ins(&mut app, Some(Ok(signed_in_as("second@example.org"))));
        let oauth = app.session.mailbox.account(second).unwrap().oauth.clone().unwrap();
        app.begin_sign_in(second, oauth, "second@example.org", "synthetic-refresh".into(), quiet, false).unwrap();
        app.poll_sign_in();

        assert!(!app.signing_in());
        assert!(app.tokens.contains_key(&second));
        assert_eq!(requested.lock().unwrap().len(), 1);
        assert_eq!(app.sync_queue.iter().copied().collect::<Vec<_>>(), vec![(second, quiet)]);
        assert!(!app.services.sync_cancel.load(Ordering::Relaxed));
    }
}

#[test]
fn queued_google_setup_uses_the_existing_completion_page_in_both_themes() {
    for theme in ["light", "dark"] {
        let mut app = app(false);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.services.google_client_id = "synthetic.apps.googleusercontent.com".into();
        app.services.google_client_secret = "synthetic-public-desktop-registration".into();
        let first = connected(&mut app, "first@example.org");
        let requests = syncs(&mut app);
        app.execute("ui.sync", json!({"account": first})).unwrap();
        sign_ins(&mut app, Some(Ok(signed_in_as("second@example.org"))));
        lookups(&mut app, Some(Ok(vec!["aspmx.l.google.com"])));
        let mut h = open(app, json!({"email": "second@example.org"}));
        click(&mut h, "Connect");
        click(&mut h, "Sign in with Google");
        h.run_steps(8);

        assert_eq!(page(&h), Some(Page::Done));
        assert!(says(&h, "Its mail is waiting to download."), "{theme}");
        assert!(!says(&h, "Your mail is downloading now"), "{theme}");
        assert_eq!(requests.lock().unwrap().len(), 1);
        let second = h.state().session.mailbox.accounts.iter().find(|a| a.email == "second@example.org").unwrap().id;
        assert_eq!(h.state().sync_queue.iter().copied().collect::<Vec<_>>(), vec![(second, false)]);
    }
}
