// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Content files kept compressed at rest (`<name>.z`): written so when that is worth it, read
//! back as the very bytes their name says or not at all, swept and copied as plain ones are, and
//! plain ones written before compressed later ([`compress_existing`]).

use super::*;

/// `bytes` as a zlib stream, as a compressed content file holds them.
fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut out = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    out.write_all(bytes).unwrap();
    out.finish().unwrap()
}

/// The text of a message of `mb` long enough to be kept outside the mailbox file.
fn long_body(mb: &Mailbox) -> Vec<u8> {
    mb.message(first_with(mb, |m| m.body.text.len() > INLINE_MAX)).unwrap().body.text.as_bytes().to_vec()
}

#[test]
fn compressible_content_is_written_compressed_and_reads_back_the_same() {
    let tmp = Temp::new("compress-round-trip");
    let path = tmp.file();
    compress_under_test(&path);
    let mb = mailbox(12);
    save(&path, &mb).unwrap();
    let text = long_body(&mb);
    let file = blob_file(&path, &text);
    assert!(!file.exists() && packed(&file).is_file(), "a long text is kept compressed");
    assert!(fs::metadata(packed(&file)).unwrap().len() < text.len() as u64);
    // Noise doesn't shrink: kept as it is, and never in both forms.
    let shared = noise(7, 40_000);
    assert!(blob_file(&path, &shared).is_file() && !packed(&blob_file(&path, &shared)).exists());
    clear_cache();
    assert!(whole(&path) == mb);
    // Read as an export streams it, and as the word index reads it.
    let r = Ref::of(&text).to_model();
    let mut streamed = Vec::new();
    Content.open(&path, &r).unwrap().read_to_end(&mut streamed).unwrap();
    assert_eq!(streamed, text);
    assert_eq!(Reader::new(&path).read(&r), Some(text.clone()));
    // Saving the same content again writes nothing.
    assert!(!put(&blob_dir(&path), &Ref::of(&text), &text).unwrap());
    assert!(!file.exists());
}

#[test]
fn a_compressed_file_that_isnt_what_its_name_says_is_refused() {
    let tmp = Temp::new("compress-corrupt");
    let path = tmp.file();
    compress_under_test(&path);
    let text = long_text(3).into_bytes();
    let r = stash(&path, &Digest::of(&text), &text).unwrap();
    let file = blob_file(&path, &text);
    assert!(packed(&file).is_file() && !file.exists());
    assert_eq!(Content.fetch_fresh(&path, &r).unwrap().as_slice(), text.as_slice());
    // Other bytes of the same length.
    let mut other = text.clone();
    other[0] ^= 1;
    fs::write(packed(&file), zlib(&other)).unwrap();
    assert!(Content.fetch_fresh(&path, &r).is_err());
    assert_eq!(Reader::new(&path).read(&r), None);
    assert!(std::io::copy(&mut Content.open(&path, &r).unwrap(), &mut std::io::sink()).is_err());
    // Not a zlib stream at all.
    fs::write(packed(&file), &text).unwrap();
    assert!(Content.fetch_fresh(&path, &r).is_err());
}

#[test]
fn a_compressed_file_that_inflates_past_its_length_is_refused_without_inflating_it_all() {
    let tmp = Temp::new("compress-bomb");
    let path = tmp.file();
    compress_under_test(&path);
    let text = long_text(4).into_bytes();
    let r = stash(&path, &Digest::of(&text), &text).unwrap();
    let file = blob_file(&path, &text);
    // The right bytes, and then 64 MB of zeros (some 64 KB compressed): read no further than one
    // byte past the length promised (nothing is made room for past that), and refused.
    let mut out = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    out.write_all(&text).unwrap();
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..64 {
        out.write_all(&zeros).unwrap();
    }
    fs::write(packed(&file), out.finish().unwrap()).unwrap();
    assert!(Content.fetch_fresh(&path, &r).is_err());
    assert_eq!(Reader::new(&path).read(&r), None);
    assert!(std::io::copy(&mut Content.open(&path, &r).unwrap(), &mut std::io::sink()).is_err());
}

#[test]
fn a_sweep_removes_compressed_files_nothing_names_and_keeps_those_named() {
    let tmp = Temp::new("compress-sweep");
    let path = tmp.file();
    compress_under_test(&path);
    let mb = mailbox(12);
    save(&path, &mb).unwrap();
    let named = packed(&blob_file(&path, &long_body(&mb)));
    assert!(named.is_file());
    // Content nobody names, and what a write of a compressed file cut short left.
    let stray_bytes = long_text(777).into_bytes();
    let stray = packed(&blob_file(&path, &stray_bytes));
    fs::create_dir_all(stray.parent().unwrap()).unwrap();
    fs::write(&stray, zlib(&stray_bytes)).unwrap();
    let hex = Digest::of(&stray_bytes).hex();
    let half = stray.with_file_name(format!("{hex}.z.12-ab-3.tmp"));
    fs::write(&half, b"half").unwrap();
    for file in [&named, &stray, &half] {
        age(file, Duration::from_secs(3600));
    }
    let swept = sweep(&path, GRACE).unwrap();
    assert_eq!(swept.removed, 2, "{swept:?}");
    assert!(!stray.exists() && !half.exists() && named.is_file());
    clear_cache();
    assert!(whole(&path) == mb);
}

#[test]
fn plain_content_files_are_compressed_later_and_the_mailbox_reads_the_same() {
    let tmp = Temp::new("compress-existing");
    let path = tmp.file();
    let mb = mailbox(12);
    // Written plain (as before this form).
    save(&path, &mb).unwrap();
    let text = long_body(&mb);
    let file = blob_file(&path, &text);
    assert!(file.is_file() && !packed(&file).exists());
    let first = compress_existing(&path, 1);
    assert!(first.files <= 1 && first.left > 0, "{first:?}");
    let rest = compress_existing(&path, 100_000);
    assert!(rest.files > 0 && rest.bytes_after < rest.bytes_before && rest.left == 0, "{rest:?}");
    assert!(!file.exists() && packed(&file).is_file());
    // Noise isn't worth it: left as it is, and not looked at again.
    let shared = noise(7, 40_000);
    assert!(blob_file(&path, &shared).is_file() && !packed(&blob_file(&path, &shared)).exists());
    assert_eq!(compress_existing(&path, 100_000), CompressReport::default());
    clear_cache();
    let mut opened = open(&path).unwrap();
    assert_eq!(opened.unavailable, 0);
    load(&path, &mut opened);
    assert!(opened.mailbox == mb);
    // And the session's command says the same of a folder with nothing left to do.
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    let said = s.run("file.compressContent", &json!({})).unwrap();
    assert_eq!((said["files"].as_u64(), said["left"].as_u64()), (Some(0), Some(0)));
}

#[test]
fn a_plain_file_that_isnt_what_its_name_says_is_never_compressed() {
    let tmp = Temp::new("compress-existing-damaged");
    let path = tmp.file();
    let mb = mailbox(12);
    save(&path, &mb).unwrap();
    let text = long_body(&mb);
    let file = blob_file(&path, &text);
    let mut damaged = text.clone();
    damaged[0] ^= 1;
    fs::write(&file, &damaged).unwrap();
    compress_existing(&path, 100_000);
    assert!(file.is_file() && !packed(&file).exists());
    assert_eq!(fs::read(&file).unwrap(), damaged);
}

#[test]
fn save_as_copies_a_compressed_file_as_it_is() {
    let tmp = Temp::new("compress-copy");
    let path = tmp.file();
    compress_under_test(&path);
    let mb = mailbox(12);
    save(&path, &mb).unwrap();
    clear_cache();
    // Opened with no room for bodies: they stay on disk, and are copied, not read.
    let opened = with_budget(1, || open(&path).unwrap());
    let other = tmp.0.join("copy.mailcraft");
    assert!(copy_content(&path, &other, &opened.mailbox).unwrap() > 0);
    let text = long_body(&mb);
    let copied = packed(&blob_file(&other, &text));
    assert!(copied.is_file() && !blob_file(&other, &text).exists());
    assert_eq!(fs::read(&copied).unwrap(), fs::read(packed(&blob_file(&path, &text))).unwrap());
    save(&other, &opened.mailbox).unwrap();
    clear_cache();
    assert!(whole(&other) == mb);
}
