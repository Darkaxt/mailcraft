// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use egui_kittest::kittest::Queryable as _;
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
fn all_mail_sits_under_the_unified_inbox_and_lists_every_folder_with_its_name() {
    let mut h = crate::people_view::testing::window("view.mail");
    h.run_steps(3);
    let inbox = h.state().hit.rect("allInboxes", 0).expect("the unified Inbox");
    let all = h.state().hit.rect("allMail", 0).expect("All mail under it");
    assert!(inbox.max.y <= all.min.y + 0.5);
    h.state_mut().execute("view.allMail", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().title_stem(), tl!("All mail"));
    let view = &h.state().session.view;
    assert!(view.scope == Scope::Inboxes && view.all_mail);
    h.state_mut().execute("view.allInboxes", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().title_stem(), tl!("All inboxes"));
    assert!(!h.state().session.view.all_mail);
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

/// A favourite wears its star in the unified tree as in its account's, and not under Favorites,
/// where every row is one (v0.12.21).
#[test]
fn a_favourite_wears_its_star_in_the_unified_tree() {
    let mut s = mailcraft_engine::Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().kind = AccountKind::Microsoft;
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let company = s.mailbox.add_folder(account, Some(inbox), "Company").unwrap();
    let other = s.mailbox.add_folder(account, Some(inbox), "Other").unwrap();
    s.run("folder.favorite", &json!({"id": company, "value": true})).unwrap();
    let mut h = crate::people_view::testing::window_on(s.mailbox, "view.mail");
    h.state_mut().ui.folder_tabs = true;
    h.state_mut().execute("ui.folderTab", json!({"tab": "unified"})).unwrap();
    h.run_steps(3);
    let row = h.state().hit.rect("unifiedFolder", company).expect("its unified row");
    let star = h.state().hit.rect("folderStar", company).expect("its star");
    assert!(row.contains(star.center()), "{star:?} in {row:?}");
    assert!(h.state().hit.rect("folderStar", other).is_none(), "not a favourite: no star");
    // The filing Inbox, shown as "Folders", wears none even when it is a favourite (v0.12.24).
    h.state_mut().session.run("folder.favorite", &json!({"id": inbox, "value": true})).unwrap();
    h.run_steps(3);
    assert!(h.state().hit.rect("unifiedFolder", inbox).is_some() && h.state().hit.rect("folderStar", inbox).is_none());
    h.state_mut().execute("ui.folderTab", json!({"tab": "favorites"})).unwrap();
    h.run_steps(3);
    assert!(h.state().hit.rect("folder", company).is_some() && h.state().hit.rect("folderStar", company).is_none());
    // A click on the star takes the folder out of Favorites (v0.12.24).
    h.state_mut().execute("ui.folderTab", json!({"tab": "unified"})).unwrap();
    h.run_steps(3);
    let star = h.state().hit.rect("folderStar", company).unwrap().center();
    h.event(egui::Event::PointerMoved(star));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: star, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    assert!(!h.state().session.mailbox.folder(company).unwrap().favorite, "the star's click unstars it");
    assert!(h.state().hit.rect("folderStar", company).is_none());
}

/// The unified tree's Folders is its own row, not the filing account's Inbox (v0.12.24): no star,
/// and its right-click menu makes a folder and is the unified mailbox's, not that Inbox's.
#[test]
fn the_unified_folders_row_is_not_the_filing_accounts_inbox() {
    let mut s = mailcraft_engine::Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().kind = AccountKind::Microsoft;
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    s.mailbox.add_folder(account, Some(inbox), "Company").unwrap();
    s.run("folder.favorite", &json!({"id": inbox, "value": true})).unwrap();
    let mut h = crate::people_view::testing::window_on(s.mailbox, "view.mail");
    h.state_mut().ui.folder_tabs = true;
    h.state_mut().execute("ui.folderTab", json!({"tab": "unified"})).unwrap();
    h.run_steps(3);
    let at = h.state().hit.rect("unifiedFolder", inbox).expect("the Folders row").center();
    assert!(h.state().hit.rect("folderStar", inbox).is_none(), "its Inbox is a favourite, Folders wears no star");
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert!(h.query_by_label("New Folder…").is_some() && h.query_by_label("New search folder…").is_some());
    for theirs in ["Rename Folder…", "Delete Folder", "Tag", "Remove from Favorites", "Properties…"] {
        assert!(h.query_by_label(theirs).is_none(), "{theirs} is the Inbox's, not Folders'");
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // A click lists what every account has not filed yet, as before.
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert_eq!(h.state().session.view.filing, Some(account));
}

#[test]
fn a_folder_folded_in_the_unified_tree_stays_open_in_its_accounts_own_tree() {
    let mut s = mailcraft_engine::Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().kind = AccountKind::Microsoft;
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let company = s.mailbox.add_folder(account, Some(inbox), "Company").unwrap();
    let team = s.mailbox.add_folder(account, Some(company), "Team").unwrap();
    let mut h = crate::people_view::testing::window_on(s.mailbox, "view.mail");
    h.run_steps(3);
    assert!(h.state().hit.rect("unifiedFolder", team).is_some() && h.state().hit.rect("folder", team).is_some());
    h.state_mut().execute("ui.toggleFolder", json!({"id": company, "unified": true})).unwrap();
    h.run_steps(2);
    assert!(h.state().hit.rect("unifiedFolder", team).is_none(), "folded in the unified tree");
    assert!(h.state().hit.rect("folder", team).is_some(), "still open in the account's own tree");
    assert!(!h.state().ui.collapsed_folders.contains(&company) && h.state().ui.collapsed_unified.contains(&company));
    let restored: crate::UiState = serde_json::from_value(serde_json::to_value(h.state().prefs()).unwrap()).unwrap();
    assert!(restored.collapsed_unified.contains(&company));
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
