// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn origin_tints_preserve_text_contrast_and_high_contrast_surfaces() {
    for dark in [false, true] {
        let tokens = Tokens::build(dark, crate::theme::Accent::default(), false);
        for color in mailcraft_model::ACCOUNT_ROW_COLORS.into_iter().chain([Rgb(0, 0, 0), Rgb(255, 0, 255)]) {
            let tint = row_tint(&tokens, color).expect("ordinary account rows must have a readable origin tint");
            assert_ne!(tint, tokens.surface, "origin must remain visible");
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
