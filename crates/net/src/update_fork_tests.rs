// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The packaged fork checks its own releases and excludes upstream download assets.
use crate::fake::{FakeHttp, json_answer};
use serde_json::json;

#[test]
fn packaged_repository_and_release_assets_remain_bound_to_this_fork() {
    assert_eq!(crate::update::REPOSITORY, "https://github.com/Darkaxt/mailcraft");
    let tag = "v99.0.0";
    let root = "https://github.com/Darkaxt/mailcraft";
    let answer = json!({"tag_name":tag,"prerelease":false,"draft":false,"html_url":format!("{root}/releases/tag/{tag}"),
    "assets":[
        {"name":"mailcraft-99.0.0-windows-x64.msi","size":123,"browser_download_url":format!("{root}/releases/download/{tag}/mailcraft-99.0.0-windows-x64.msi")},
        {"name":"upstream.msi","size":123,"browser_download_url":format!("https://github.com/UCS-Care-Group/mailcraft/releases/download/{tag}/upstream.msi")}
    ]});
    let server = FakeHttp::answering(json_answer("200 OK", &answer.to_string()));
    let release = crate::update::latest_at(&server.url(""), crate::update::REPOSITORY, false).unwrap();
    assert_eq!(release.assets.len(), 1);
    assert_eq!(release.assets[0].name, "mailcraft-99.0.0-windows-x64.msi");
    assert!(!crate::update_download::release_keys().is_empty());
    for upstream in ["Mu5RF5oJpFhDGfa5wpgznC39tDC8vbUbyNkiAMdluPQ=", "XV6WRgW06/mGhZCHhFVKMIEZDmLevQduZPLSj/UW5zI="] {
        assert!(!crate::update::RELEASE_KEYS.contains(&upstream), "the fork owns its release trust root");
    }
}
