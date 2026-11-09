// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native acceptance sources for semantic dark colours, contrast, images and cache changes.

use super::{
    colors::{Paper, contrast, luminance},
    images::Pictures,
    layout::{ItemKind, Laid},
    view::{self, Doc},
};
use crate::{
    MailApp, Services,
    theme::{Accent, Appearance, ThemeChoice, Tokens},
};
use egui::Color32;
use mailcraft_html::{Env, Rgb};
use mailcraft_model::{Address, Attachment, Body, Message};

pub(super) fn scenarios() -> [(&'static str, &'static str); 6] {
    [
        (
            "U3 newsletter",
            "<body bgcolor='#fafafa'><table bgcolor='white' width='560'><tr><td style='color:#555;padding:18px'><h2 style='color:#143773'>Weekly dispatch</h2><p>Body copy and <a href='https://example.org'>a real link</a>.</p></td></tr></table></body>",
        ),
        (
            "U3 table",
            "<table border='1' style='border-color:#ccd'><tr bgcolor='#fff8d4'><th>Company</th><th>Amount</th></tr><tr bgcolor='#e8f2fa'><td style='color:#555'>StudioOffice</td><td>25.00</td></tr></table>",
        ),
        (
            "U3 inline colours",
            "<p style='color:#777;background:white'>Readable body <span style='background:#ffff00;color:#222'>yellow highlight</span> <span style='color:#1847b0'>blue</span> <span style='color:#b00000'>red</span></p>",
        ),
        (
            "U3 nested backgrounds",
            "<div style='background:#f4f4f4;padding:12px'><div style='background:#e9effb;padding:10px'><p style='background:#fff3c8;color:#999'>Nested card</p><p>Inherited text</p></div></div>",
        ),
        (
            "U3 image pixels",
            "<div style='background:white;padding:12px'><p>Picture colours must stay unchanged</p><img src='cid:colour-fixture' width='120' height='90' alt='Original colour sample'></div>",
        ),
        (
            "U3 author dark",
            "<style>body{background:white;color:#222}@media(prefers-color-scheme:dark){body{background:#121820;color:#e5e8ef}a{color:#90baff}}</style><p>Author-designed dark palette <a href='https://example.org'>link</a></p>",
        ),
    ]
}

pub(super) fn message(html: &str) -> Message {
    let mut message = Message {
        from: Address::new("Fixture", "sender@example.org"),
        to: vec![Address::new("Owner", "owner@example.org")],
        subject: "Native colour fixture".into(),
        body: Body { text: "Fixture text".into(), html: Some(html.into()) },
        ..Default::default()
    };
    message.attachments.push(Attachment {
        content_id: "colour-fixture".into(),
        inline: true,
        ..Attachment::new("colour.png", "image/png", super::tests::png(40, 30))
    });
    message
}

#[test]
fn role_aware_background_text_links_and_borders_remain_readable() {
    for dark in [false, true] {
        for high in [false, true] {
            let t = Tokens::build(dark, Accent::Blue, high);
            for background in [Rgb(255, 255, 255), Rgb(255, 255, 0), Rgb(255, 0, 0), Rgb(0, 180, 240), Rgb(120, 120, 120), Rgb(18, 24, 32)] {
                let paper = Paper::of(background, &t);
                if high {
                    assert_eq!(paper.fill, t.surface);
                }
                if dark && !high {
                    assert!(luminance(paper.fill) < 0.25, "no glaring background islands");
                }
                for asked in [None, Some(Rgb(255, 255, 255)), Some(Rgb(0, 0, 0)), Some(Rgb(119, 119, 119)), Some(Rgb(16, 40, 180))] {
                    for link in [false, true] {
                        assert!(contrast(paper.ink(&t, asked, link, false), paper.fill) >= if high { 7.0 } else { 4.5 });
                    }
                }
                assert!(contrast(paper.border(&t, Some(Rgb(190, 190, 190))), paper.fill) >= if high { 3.0 } else { 1.5 });
            }
        }
    }
    let t = Tokens { sender_dark: true, ..Tokens::dark() };
    let paper = Paper::of(Rgb(18, 24, 32), &t);
    assert_eq!(paper.fill, Color32::from_rgb(18, 24, 32));
    assert_eq!(paper.ink(&t, Some(Rgb(229, 232, 239)), false, false), Color32::from_rgb(229, 232, 239));
}

#[test]
fn native_layout_handles_newsletters_tables_highlights_images_and_author_dark_without_editing_source() {
    for dark in [false, true] {
        for high in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::theme::font_definitions());
            crate::theme::apply(
                &ctx,
                if dark { Appearance::Dark } else { Appearance::Light }.preference(None),
                &ThemeChoice { high_contrast: high, ..Default::default() },
            );
            ctx.run_ui(Default::default(), |ui| {
                for (name, html) in scenarios() {
                    let m = message(html);
                    let original = m.clone();
                    let doc = Doc::of_in(&m, false, Env::for_pane(600.0, dark));
                    let held = Pictures::of(ui.ctx());
                    let mut pictures = held.lock().unwrap();
                    let mut laid = Laid::new(600.0).sender_dark(doc.sender_dark).cut(doc.cuts.clone());
                    laid.advance(ui.ctx(), &doc.blocks, Some(&m), &mut pictures, usize::MAX);
                    assert!(laid.done, "{name}");
                    let mut texts = 0;
                    for item in &laid.items {
                        if let ItemKind::Text(text) = &item.kind {
                            texts += 1;
                            for section in &text.job.sections {
                                let paper = if section.format.background == Color32::TRANSPARENT { text.paper } else { section.format.background };
                                assert!(contrast(section.format.color, paper) >= if high { 7.0 } else { 4.5 }, "{name}");
                            }
                        }
                    }
                    assert!(texts > 0, "{name}");
                    if name == "U3 image pixels" {
                        let image = laid
                            .items
                            .iter()
                            .find_map(|i| match &i.kind {
                                ItemKind::Picture(image) => Some(image),
                                _ => None,
                            })
                            .unwrap();
                        assert_eq!(image.data, m.attachments[0].data, "same original encoded pixels in every theme");
                    }
                    assert_eq!(m, original, "rendering never changes source, search text, replies or attachments");
                }
            })
            .drop_without_applying_deltas();
        }
    }
}

#[test]
fn cached_body_rebuilds_when_high_contrast_or_accent_changes_without_reopening_mail() {
    let mut mailbox = mailcraft_model::Mailbox::with_local_account("Owner", "owner@example.org");
    let inbox = mailbox.inbox().unwrap();
    let mut m = message("<p>A body with <a href='https://example.org'>default link colour</a></p>");
    m.folder = inbox;
    let id = mailbox.add_message(m).unwrap();
    let mut app = MailApp::new(mailcraft_engine::Session::new(mailbox), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app.ui.theme = Appearance::Dark;
    app.execute("mail.select", serde_json::json!({"id":id})).unwrap();
    let original = app.session.mailbox.message(id).unwrap().clone();
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1200.0, 850.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    let mut begun = view::layouts_begun(&h.ctx);
    for (high, accent) in [(true, Accent::Blue), (true, Accent::Teal), (false, Accent::Teal)] {
        h.state_mut().ui.high_contrast = high;
        h.state_mut().ui.accent = accent;
        h.run_steps(3);
        let now = view::layouts_begun(&h.ctx);
        assert!(now > begun);
        begun = now;
        assert_eq!(h.state().session.mailbox.message(id).unwrap(), &original);
    }
}
