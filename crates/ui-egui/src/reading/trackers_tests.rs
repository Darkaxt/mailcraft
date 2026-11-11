// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tracking pictures are told from pictures by strong signals only.

use super::trackers::{has_token, images, known, scan, unseen};

#[test]
fn the_open_pixels_of_mail_services_are_known() {
    for url in [
        "https://us21.list-manage.com/track/open.php?u=4f1c2a9e8d7b6a5c3e2f1a0b9&id=7c3e1a2b4d&e=9a8b7c6d5e",
        "https://u1234567.ct.sendgrid.net/wf/open?upn=u001.Zb4nQ3xT9kLmP2rV8sW1yA7cD5eF6gH0jK",
        "https://email.news.shop.example/wf/open?upn=u001.Zb4nQ3xT9kLmP2rV8sW1yA7cD5eF6gH0jK",
        "https://mandrillapp.com/track/open.php?u=30451234&id=5c1f9e2d7a8b4c3d",
        "https://ea.pstmrk.it/open?m=v3_1.abc&r=def",
        "https://track.customeriomail.com/e/o/eyJlbWFpbF9pZCI6IkRtVnVBUVkifQ==",
        "https://trk.klaviyomail.com/open/01HABCDEF",
        "https://click.e.brand.example.cl.exct.net/open.aspx?ffcb10-fec8",
        "https://r20.rs6.net/on.jsp?a=1102345678&r=3&c=abc",
        "https://brand.cmail19.com/t/d-o-abcdef-l/o.gif",
        "https://shop.activehosted.com/lt.php?nl=1&c=2&m=3&s=4&l=open",
        "https://1a2b3c4d.r.us-east-1.awstrack.me/I0/0100018b1234abcd/xyz=",
        "https://www.google-analytics.com/collect?v=1&tid=UA-1&cid=555&t=event&ec=email&ea=open",
        "https://www.facebook.com/tr?id=1234567890&ev=PageView&noscript=1",
        "//t.hubspotemail.net/e2t/to/VWxyz",
        "https://spgo.io/q/AbC123",
    ] {
        assert!(known(url), "{url}");
    }
}

#[test]
fn the_picture_hosts_of_the_same_services_are_not() {
    for url in [
        "https://mcusercontent.com/4f1c2a9e8d7b6a5c3e2f1a0b9/images/7c3e1a2b-banner.png",
        "https://f.hubspotusercontent40.net/hubfs/123456/logo.png",
        "https://gallery.mailchimp.com/abc/images/hero.jpg",
        "https://www.facebook.com/images/fb_icon_325x325.png",
        "https://www.trackingshoes.example/track/open-house.jpg",
        "https://list-manage.com.evil.example/track/open.php",
        "mailto:someone@example.org",
        "cid:logo@example.org",
    ] {
        assert!(!known(url), "{url}");
    }
}

#[test]
fn tiny_hidden_and_off_page_pictures_are_unseen() {
    let html = r#"
        <img src="https://a.example/p1.gif" width="1" height="1">
        <img src="https://a.example/p3.gif" width=3 height=3 alt="">
        <img src="https://a.example/s.gif" style="width:2px;height:2px">
        <img src="https://a.example/n.gif" style="display: none !important">
        <img src="https://a.example/v.gif" style="visibility:hidden">
        <img src="https://a.example/o.gif" style="opacity:0">
        <img src="https://a.example/x.gif" style="position:absolute;left:-9999px;top:0">
        <img src="https://a.example/m.gif" style="max-height:0;overflow:hidden">
        <img src="https://a.example/i.gif" STYLE='margin-left:-5000px'>
    "#;
    let all = images(html);
    assert_eq!(all.len(), 9);
    for img in &all {
        assert!(unseen(img), "{img:?}");
    }
}

#[test]
fn ordinary_pictures_are_seen() {
    let html = r#"
        <img src="https://cdn.shop.example/hero.jpg" width="600" height="300" alt="Autumn sale">
        <img src="https://cdn.shop.example/icon.png" width="24" height="24">
        <img src="https://cdn.shop.example/spacer.png" style="width:100%">
        <img src="https://cdn.shop.example/rel.png" style="position:relative;left:-10px">
        <img src="https://cdn.shop.example/op.png" style="opacity:0.9">
    "#;
    for img in images(html) {
        assert!(!unseen(&img), "{img:?}");
    }
}

#[test]
fn a_token_is_random_looking_text_not_a_hashed_file_name() {
    assert!(has_token("https://t.news.example/o/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD"));
    assert!(has_token("https://t.news.example/open?r=9f8E7d6C5b4A3z2Y1x0W9v8U7t"));
    // A CDN names its pictures by their hash: that is a file, not a recipient.
    assert!(!has_token("https://cdn.example/assets/3f9a1c7e5b2d4f6a8c0e1b3d5f7a9c2e.png"));
    assert!(!has_token("https://images.ctfassets.net/space/asset/hero-banner-autumn.jpg"));
    // Words and dates are not tokens.
    assert!(!has_token("https://shop.example/newsletter/2026-11-11-autumn-collection-preview"));
    assert!(!has_token("https://shop.example/p.png?utm_source=newsletter&utm_campaign=autumn"));
}

#[test]
fn a_token_alone_never_makes_a_tracker() {
    // Sole from its host and last, but with a description: a picture.
    let described = r#"<p>Hi</p><img src="https://img.example/v/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD" alt="Your order">"#;
    assert!(scan(described).is_empty());
    // With a size meant to be seen.
    let sized = r#"<p>Hi</p><img src="https://img.example/v/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD" width="320" height="200">"#;
    assert!(scan(sized).is_empty());
    // Not last.
    let early = r#"<img src="https://img.example/v/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD"><img src="https://cdn.example/logo.png" width="120">"#;
    assert!(scan(early).is_empty());
    // Not the only one from its host.
    let many = r#"<img src="https://img.example/a.png" width="120"><img src="https://img.example/v/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD">"#;
    assert!(scan(many).is_empty());
}

#[test]
fn a_token_with_the_shape_of_a_pixel_does() {
    let html = r#"<table><tr><td><img src="https://cdn.shop.example/hero.jpg" width="600" alt="Sale"></td></tr></table>
        <p>Unsubscribe</p><img src="https://o.shop-mail.example/v/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD" border="0">"#;
    assert_eq!(scan(html), ["https://o.shop-mail.example/v/a8F3kL9qZ2xV7mN4pR1sT6uW0yB5cD"]);
}

#[test]
fn a_realistic_newsletter_has_its_trackers_found_and_its_pictures_left() {
    let html = r#"<html><body>
        <img src="https://cdn.shop.example/i/3f9a1c7e5b2d4f6a8c0e1b3d5f7a9c2e.png" width="600" height="200" alt="Autumn">
        <img src="https://cdn.shop.example/i/7d2b9e4a1c6f8b3d5e0a2c4f6b8d1e3a.jpg" alt="">
        <a href="https://shop.example"><img src="https://cdn.shop.example/logo.svg" height="40"></a>
        <img src="https://us21.list-manage.com/track/open.php?u=4f1c2a&amp;id=7c3e1a&amp;e=9a8b7c" height="1" width="1">
        <img src="https://www.google-analytics.com/collect?v=1&amp;tid=UA-1&amp;t=event">
    </body></html>"#;
    assert_eq!(
        scan(html),
        ["https://us21.list-manage.com/track/open.php?u=4f1c2a&id=7c3e1a&e=9a8b7c", "https://www.google-analytics.com/collect?v=1&tid=UA-1&t=event"]
    );
}

#[test]
fn each_tracker_is_listed_once_and_only_web_pictures_are_looked_at() {
    let pixel = r#"<img src="https://u1.ct.sendgrid.net/wf/open?upn=abc" width=1 height=1>"#;
    let html = format!("{pixel}{pixel}<img src=\"cid:logo\" width=1 height=1><img src=\"data:image/gif;base64,R0lGOD\" width=1>");
    assert_eq!(scan(&html).len(), 1);
}

#[test]
fn broken_markup_is_read_without_panicking() {
    for html in [
        "<img",
        "<img src=",
        "<img src=\"https://a.example/x",
        "<IMG\tSRC=https://a.example/y.gif WIDTH=1>",
        "<imgx src=a>",
        "ü<img src='https://ü.example/ü' width=1>",
    ] {
        let _ = scan(html);
    }
    assert_eq!(scan("<IMG\tSRC=https://a.example/y.gif WIDTH=1>"), ["https://a.example/y.gif"]);
}
