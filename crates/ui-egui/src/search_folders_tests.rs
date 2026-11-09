// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saved-query UI uses native metadata and preserves the mailbox during previews/cancellation.
use super::*;

#[test]
fn editor_prefills_owner_or_advanced_query_and_never_invents_a_scope_anchor() {
    let mut h = crate::people_view::testing::window("view.mail");
    let app = h.state_mut();
    app.execute("view.allInboxes", json!({})).unwrap();
    app.execute("view.search", json!({"query":"alias:studio@example.org"})).unwrap();
    let form = SearchForm::new(app, &json!({})).unwrap();
    assert_eq!(form.definition.query, "alias:studio@example.org");
    assert_eq!(form.definition.scope, Scope::Inboxes);
    assert!(!form.dirty());
    assert!(form.params().get("folder").is_none());
    let form = SearchForm::new(app, &json!({"query":"\"Café invoice, precisely!\" case:sensitive","scope":"everywhere"})).unwrap();
    assert_eq!(form.definition.scope, Scope::Everywhere);
    assert_eq!(form.definition.query, "\"Café invoice, precisely!\" case:sensitive");
    assert!(form.params().get("folder").is_none());
    assert!(SearchForm::new(app, &json!({"id":"not-an-id"})).is_none());
}

#[test]
fn saved_search_sidebar_editor_and_fixed_actions_fit_both_themes_without_mail_changes() {
    for theme in [crate::theme::Appearance::Light, crate::theme::Appearance::Dark] {
        let mut app = MailApp::new(mailcraft_engine::Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0)), crate::Services::default());
        app.autosave = false;
        app.ui.theme = theme;
        let id =
            app.execute("folder.newSearchFolder", json!({"name":"Invoices","query":"roadmap","scope":"inboxes"})).unwrap()["id"].as_u64().unwrap();
        app.execute("searchFolders.select", json!({"id":id})).unwrap();
        app.execute("ui.dialog", json!({"name":"searchFolder","id":id})).unwrap();
        let before = app.session.mailbox.clone();
        let mut h =
            egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(900.0, 700.0)).build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        h.run_steps(4);
        assert!(h.state().hit.rect("searchFolder", id).is_some());
        assert_eq!(h.state().title_stem(), "Invoices");
        for key in ["searchFolderSave", "searchFolderPreview", "searchFolderCancel"] {
            let rect = h.state().hit.rect(key, 0).unwrap();
            assert!(rect.left() >= 0.0 && rect.right() <= 900.0 && rect.top() >= 0.0 && rect.bottom() <= 700.0, "{key} remains reachable");
        }
        assert_eq!(h.state().session.mailbox, before);
        h.state_mut().execute("ui.closeDialog", json!({})).unwrap();
        assert_eq!(h.state().session.mailbox, before);
    }
}

#[test]
fn missing_saved_anchor_is_visible_and_editable_without_searching_another_account() {
    let mut h = crate::people_view::testing::window("view.mail");
    let app = h.state_mut();
    let anchor = app.session.view.folder;
    let id = app.execute("folder.newSearchFolder", json!({"name":"Anchored","query":"roadmap","scope":"mailbox","folder":anchor})).unwrap()["id"]
        .as_u64()
        .unwrap();
    app.session.mailbox.folders.retain(|folder| folder.id != anchor);
    app.session.mailbox_changed();
    let mut form = SearchForm::new(app, &json!({"id":id})).unwrap();
    assert!(!form.error.is_empty());
    assert_eq!(form.definition.folder, Some(anchor));
    let before = app.session.mailbox.clone();
    assert!(app.execute("searchFolders.preview", form.params()).is_err());
    assert_eq!(app.session.mailbox, before);
    form.definition.name = "Rebind this saved search".into();
    assert!(form.dirty());
    assert!(app.execute("searchFolders.edit", form.params()).is_err());
    assert_eq!(app.session.mailbox, before);
}
