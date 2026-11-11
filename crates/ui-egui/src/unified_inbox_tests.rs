// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_engine::Scope;

#[test]
fn origin_tints_preserve_text_contrast_and_high_contrast_surfaces() {
    for dark in [false, true] {
        let tokens = Tokens::build(dark, crate::theme::Accent::default(), false);
        for color in mailcraft_model::ACCOUNT_ROW_COLORS.into_iter().chain([Rgb(0, 0, 0), Rgb(255, 0, 255)]) {
            let tint = row_tint(&tokens, color).expect("ordinary account rows must have a readable origin tint");
            assert_ne!(tint, tokens.surface, "origin must remain visible");
            assert!(contrast(tint, tokens.surface) > 1.02 || color == Rgb(0, 0, 0), "a full-row tint is clearly visible");
            for ink in [tokens.text, tokens.text_dim, tokens.overdue] {
                assert!(contrast(ink, tint) >= 4.5);
            }
        }
        let high = Tokens::build(dark, crate::theme::Accent::default(), true);
        assert_eq!(row_tint(&high, Rgb(225, 233, 252)), None);
    }
}

#[test]
fn combined_navigation_search_and_origin_color_controls_are_wired_in_both_themes() {
    for theme in [crate::theme::Appearance::Light, crate::theme::Appearance::Dark] {
        let mut h = crate::people_view::testing::window("view.mail");
        h.state_mut().ui.theme = theme;
        h.state_mut().execute("view.allInboxes", json!({})).unwrap();
        h.run_steps(3);
        assert!(h.state().hit.rect("allInboxes", 0).is_some());
        assert_eq!(h.state().title_stem(), tl!("All inboxes"));
        assert!(h.state().search_box_shown);
        assert_eq!(h.state().session.view.scope, Scope::Inboxes);
    }
}

#[test]
fn custom_color_text_is_kept_per_colour_in_effect() {
    let scope = egui::Id::unique("menu");
    let old = mailcraft_model::ACCOUNT_ROW_COLORS[0];
    let new = mailcraft_model::ACCOUNT_ROW_COLORS[1];
    let ctx = egui::Context::default();
    ctx.data_mut(|data| data.insert_temp(custom_color_edit_id(scope, 1, None, old), "#123456".to_string()));
    // A swatch or Reset changed the colour: the next open starts from the new colour, not the old text.
    assert_eq!(ctx.data_mut(|data| data.get_temp::<String>(custom_color_edit_id(scope, 1, None, new))), None);
    assert_ne!(custom_color_edit_id(scope, 1, Some("a@example.org"), old), custom_color_edit_id(scope, 1, None, old));
}

#[test]
fn origin_colors_default_to_a_stripe_and_are_kept_with_the_settings() {
    let ui = crate::UiState::default();
    assert_eq!(ui.origin_colors, OriginColors::Stripe);
    let saved =
        serde_json::to_value(crate::UiState { origin_colors: OriginColors::FullRow, unified_filing_account: Some(7), ..Default::default() }).unwrap();
    assert_eq!(saved["originColors"], "fullRow");
    let restored: crate::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!((restored.origin_colors, restored.unified_filing_account), (OriginColors::FullRow, Some(7)));
}

#[test]
fn the_unified_mailbox_root_holds_inbox_filing_tree_and_search_folders_and_folds() {
    let mut s = mailcraft_engine::Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().kind = AccountKind::Microsoft;
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let company = s.mailbox.add_folder(account, Some(inbox), "Company").unwrap();
    let search = s.run("folder.newSearchFolder", &json!({"name":"Invoices","query":"invoice","scope":"inboxes"})).unwrap()["id"].as_u64().unwrap();
    let mut h = crate::people_view::testing::window_on(s.mailbox, "view.mail");
    h.run_steps(3);
    let root = h.state().hit.rect("unifiedFiling", 0).expect("the root");
    let plus = h.state().hit.rect("searchFolderNew", 0).expect("its + button");
    let all = h.state().hit.rect("allInboxes", 0).expect("its Inbox");
    let filed = h.state().hit.rect("unifiedFolder", company).expect("the filing tree under it");
    let saved = h.state().hit.rect("searchFolder", search).expect("the search folders under it");
    assert!(root.max.x <= plus.min.x && (root.center().y - plus.center().y).abs() < 4.0, "the + beside the root's name");
    assert!(root.max.y <= all.min.y + 0.5 && all.max.y <= filed.min.y + 0.5 && filed.max.y <= saved.min.y + 0.5);
    let heading = h.state().hit.rect("folderHeading", account).expect("the account's own heading");
    assert!(saved.max.y <= heading.min.y + 0.5, "the root is above the accounts");
    // A filing-tree row opens the filing view for that folder.
    h.state_mut().execute("view.filing", json!({"account": account, "folder": company})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().session.view.filing, Some(account));
    assert!(h.state().hit.rect("folderHeading", account).is_some(), "accounts stay listed in a filing view");
    // Folded like an account, and kept with the settings.
    h.state_mut().execute("ui.toggleFolder", json!({"id": UNIFIED_ROOT})).unwrap();
    h.run_steps(2);
    assert!(h.state().hit.rect("allInboxes", 0).is_none() && h.state().hit.rect("unifiedFolder", company).is_none());
    let restored: crate::UiState = serde_json::from_value(serde_json::to_value(h.state().prefs()).unwrap()).unwrap();
    assert!(restored.collapsed_folders.contains(&UNIFIED_ROOT));
}

#[test]
fn the_filing_account_follows_the_view_then_the_owners_pick_then_the_first() {
    let mut s = mailcraft_engine::Session::empty();
    let mut ids = Vec::new();
    for email in ["one@example.org", "two@example.org"] {
        let id = s.run("account.add", &json!({"email": email})).unwrap()["id"].as_u64().unwrap();
        s.mailbox.account_mut(id).unwrap().kind = AccountKind::Microsoft;
        ids.push(id);
    }
    let mut app = MailApp::new(s, crate::Services::default());
    app.autosave = false;
    let accounts = filing_accounts(&app);
    assert_eq!(accounts.len(), 2);
    assert_eq!(filing_account(&app, &accounts), Some(ids[0]));
    pick_filing_account(&mut app, ids[1]);
    assert_eq!(filing_account(&app, &accounts), Some(ids[1]));
    app.execute("view.filing", json!({"account": ids[0]})).unwrap();
    assert_eq!(filing_account(&app, &accounts), Some(ids[0]));
    pick_filing_account(&mut app, ids[1]);
    assert_eq!(app.session.view.filing, Some(ids[1]), "a pick shows its tree at once in a filing view");
}
