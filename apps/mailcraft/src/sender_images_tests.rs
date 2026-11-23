// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a sender's picture is looked up. Nothing here goes to the network: a look-up finds its
//! picture already kept, or downloads with a stand-in.

use std::cell::RefCell;

use super::*;

/// The download of a look-up that must find what it wants kept.
fn no_network(url: &str) -> Result<Vec<u8>, String> {
    panic!("{url} was asked for")
}

const ADA: &str = "https://gravatar.com/avatar/cfe00dde46ef942601ffafb9e2825e802858b460e586b8305b344c1eb826357d?s=96&d=404";

#[test]
fn gravatar_gets_a_hash_and_favicon_im_the_domain() {
    // The hash is of the address in lower case, without the space round it.
    assert_eq!(
        addresses(" Ada@Example.ORG "),
        vec![
            ADA.to_string(),
            "https://favicon.im/example.org?larger=true&throw-error-on-404=true".into(),
            "https://www.google.com/s2/favicons?domain=example.org&sz=96".into()
        ]
    );
    // Nothing of the address but the domain reaches favicon.im.
    assert!(addresses("grace.hopper@shop.example").iter().all(|url| !url.contains("grace")));
}

#[test]
fn favicon_im_is_asked_for_the_site_not_the_mail_subdomain() {
    for (address, site) in [
        ("news@mail.nutrient.io", "nutrient.io"),
        ("noreply@email.steampowered.com", "steampowered.com"),
        ("a@x.y.shop.example", "shop.example"),
        ("a@u8f3a.spam.example", "spam.example"),
        ("a@news.bbc.co.uk", "bbc.co.uk"),
        ("a@bbc.co.uk", "bbc.co.uk"),
        ("a@shop.com.au", "shop.com.au"),
        ("a@mail.bmw.de", "bmw.de"),
    ] {
        assert_eq!(addresses(address).get(1), Some(&format!("https://favicon.im/{site}?larger=true&throw-error-on-404=true")), "{address}");
    }
}

#[test]
fn a_providers_domain_has_no_icon_of_the_sender() {
    for address in [
        "someone@gmail.com",
        "someone@outlook.com",
        "someone@hotmail.co.uk",
        "someone@yahoo.fr",
        "alerts@mail.gmail.com",
        "someone@proton.me",
        "someone@web.de",
        "someone@qq.com",
        "a1b2c3@privaterelay.appleid.com",
    ] {
        let urls = addresses(address);
        assert_eq!(urls.len(), 1, "{address}");
        assert!(urls[0].starts_with("https://gravatar.com/avatar/"), "{address}");
    }
}

#[test]
fn a_name_on_a_hosting_platform_does_not_wear_the_platforms_icon() {
    for address in [
        "it@contoso.onmicrosoft.com",
        "news@contoso.sharepoint.com",
        "app@shop.azurewebsites.net",
        "me@someone.github.io",
        "me@blog.blogspot.com",
        "billing@paypal-help.zendesk.com",
        "jira@acme.atlassian.net",
        "a@github.io",
        "a@co.uk",
    ] {
        assert_eq!(addresses(address).len(), 1, "{address}");
    }
}

#[test]
fn what_is_no_address_is_not_looked_up() {
    let long = format!("{}@example.org", "a".repeat(400));
    for junk in [
        "",
        "  ",
        "ada",
        "@example.org",
        "ada@",
        "ada@localhost",
        "ada@exa mple.org",
        "a da@example.org",
        "ada@example.org\r\nX: y",
        "ada@exam/ple.org",
        "ada@-",
        &long,
    ] {
        assert!(addresses(junk).is_empty(), "{junk:?}");
    }
}

#[test]
fn a_kept_picture_is_not_asked_for_again() {
    let dir = std::env::temp_dir().join(format!("mailcraft-sender-images-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[7; 64]].concat();
    assert!(cache.put(ADA, &png));
    let alive = Arc::new(());
    assert_eq!(look_up("ada@example.org", Some(&cache), &Arc::downgrade(&alive), no_network), Ok(png));
    assert!(look_up("not an address", Some(&cache), &Arc::downgrade(&alive), no_network).is_err());
    // Once the window lets go (sender images turned off), nothing more is asked, kept or not.
    let gone = Arc::downgrade(&alive);
    drop(alive);
    assert!(look_up("ada@example.org", Some(&cache), &gone, no_network).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_kept_site_icon_is_found_before_gravatar_is_asked_again() {
    let dir = std::env::temp_dir().join(format!("mailcraft-sender-icons-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[9; 64]].concat();
    let icon = addresses("ada@example.org").remove(1);
    assert!(cache.put(&icon, &png));
    // Gravatar had nothing last time: its address is not kept, and it is not asked again.
    let alive = Arc::new(());
    assert_eq!(look_up("ada@example.org", Some(&cache), &Arc::downgrade(&alive), no_network), Ok(png));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_sites_icon_is_kept_for_a_sender_and_never_for_a_message() {
    let dir = std::env::temp_dir().join(format!("mailcraft-sender-ico-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let ico = b"\0\0\x01\0\x01\0rest".to_vec();
    let icon = addresses("ada@example.org").remove(1);
    assert!(!cache.put(&icon, &ico), "a message's pictures are never an ICO");
    assert!(cache.put_as(&icon, &ico, looks_like_a_picture));
    let alive = Arc::new(());
    assert_eq!(look_up("ada@example.org", Some(&cache), &Arc::downgrade(&alive), no_network), Ok(ico));
    // Read back for a message's picture, it is none.
    assert_eq!(cache.get(&icon), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn turning_it_off_stops_before_the_next_request() {
    let dir = std::env::temp_dir().join(format!("mailcraft-sender-stop-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[5; 64]].concat();
    let (alive, asked) = (RefCell::new(Some(Arc::new(()))), RefCell::new(Vec::new()));
    let wanted = alive.borrow().as_ref().map(Arc::downgrade).unwrap();
    // Turned off while Gravatar is being asked: that request goes on, and nothing more is asked.
    let fetch = |url: &str| {
        asked.borrow_mut().push(url.to_string());
        alive.borrow_mut().take();
        Err::<Vec<u8>, _>("404")
    };
    assert!(look_up("ada@example.org", Some(&cache), &wanted, fetch).is_err());
    assert_eq!(asked.borrow().len(), 1);
    // And a picture that comes once it was turned off is not kept.
    let again = Arc::new(());
    let wanted = Arc::downgrade(&again);
    let held = RefCell::new(Some(again));
    let fetch = |_: &str| {
        held.borrow_mut().take();
        Ok::<_, String>(png.clone())
    };
    assert!(look_up("ada@example.org", Some(&cache), &wanted, fetch).is_err());
    assert_eq!(cache.usage().0, 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_home_pages_icons_are_read_largest_first_and_never_an_svg() {
    let page = br#"<html><head>
        <LINK rel="shortcut icon" href="/favicon.ico">
        <link rel='icon' type='image/png' sizes='32x32' href='/img/icon-32.png?v=2&amp;x=1'>
        <link rel=apple-touch-icon href=//cdn.example.org/touch.png>
        <link rel="icon" href="/mask.svg">
        <link rel="mask-icon" href="/pin.png">
        <link rel="stylesheet" href="/site.css">
        <link rel="icon" sizes="512x512" href="http://example.org/huge.png">
        <link rel="icon" href="data:image/png;base64,AAAA">
        <link rel="icon" href="javascript:alert(1)">
    </head></html>"#;
    assert_eq!(
        icon_links(page, "example.org"),
        [
            "https://cdn.example.org/touch.png",
            "https://example.org/img/icon-32.png?v=2&x=1",
            "https://example.org/favicon.ico",
            "https://example.org/huge.png",
        ]
    );
    assert!(icon_links(b"not a page at all", "example.org").is_empty());
    assert!(icon_links(b"<link rel=\"icon\" href=\"", "example.org").is_empty());
}

#[test]
fn a_folders_icon_is_the_sites_own_first_then_favicon_im_then_googles() {
    let dir = std::env::temp_dir().join(format!("mailcraft-folder-icons-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[5; 64]].concat();
    let alive = Arc::new(());
    let key = format!("{}coinbase.com", folder_sites::ICON_KEY);
    // The home page names an icon the site doesn't have: its /favicon.ico is next.
    let asked = RefCell::new(Vec::new());
    let fetch = |url: &str| {
        asked.borrow_mut().push(url.to_string());
        match url {
            "https://coinbase.com/" => Ok(b"<link rel=icon href=/gone.png>".to_vec()),
            "https://coinbase.com/favicon.ico" => Ok(png.clone()),
            _ => Err("404".to_string()),
        }
    };
    assert_eq!(look_up(&key, Some(&cache), &Arc::downgrade(&alive), fetch), Ok(png.clone()));
    assert_eq!(*asked.borrow(), ["https://coinbase.com/", "https://coinbase.com/gone.png", "https://coinbase.com/favicon.ico"]);
    // Kept: not asked again at the next start.
    assert_eq!(look_up(&key, Some(&cache), &Arc::downgrade(&alive), no_network), Ok(png.clone()));
    // A site with nothing of its own: favicon.im, then Google.
    let asked = RefCell::new(Vec::new());
    let fetch = |url: &str| {
        asked.borrow_mut().push(url.to_string());
        if url.starts_with("https://www.google.com/") { Ok(png.clone()) } else { Err("404".to_string()) }
    };
    let quora = format!("{}quora.com", folder_sites::ICON_KEY);
    assert_eq!(look_up(&quora, Some(&cache), &Arc::downgrade(&alive), fetch), Ok(png.clone()));
    assert_eq!(
        *asked.borrow(),
        [
            "https://quora.com/",
            "https://quora.com/favicon.ico",
            "https://favicon.im/quora.com?larger=true&throw-error-on-404=true",
            "https://www.google.com/s2/favicons?domain=quora.com&sz=96",
        ]
    );
    // Nobody is asked about a mail provider, a hosting platform's customer or what is no site.
    for junk in ["gmail.com", "contoso.onmicrosoft.com", "mail.coinbase.com", "co.uk", "not a site"] {
        assert!(look_up(&format!("{}{junk}", folder_sites::ICON_KEY), None, &Arc::downgrade(&alive), no_network).is_err(), "{junk}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
