// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[test]
fn external_ocr_directory_preserves_verified_unicode_identity() {
    let scratch = tempfile::tempdir().unwrap();
    let directory = scratch.path().join("OCR café 子");
    std::fs::create_dir(&directory).unwrap();
    let canonical = directory.canonicalize().unwrap();
    let external = super::external_directory(&canonical).unwrap();
    assert!(!external.as_os_str().to_string_lossy().starts_with(r"\\?\"));
    assert!(external.is_absolute());
    assert_eq!(external.canonicalize().unwrap(), canonical);
    assert_eq!(external.file_name(), directory.file_name());
}

#[test]
#[ignore = "finite native OCR diagnosis; explicit runtime, image and owned work paths required"]
fn diagnose_multilingual_ocr_order_on_verified_image() {
    use std::path::{Path, PathBuf};
    let root = PathBuf::from(std::env::var("MAILCRAFT_OCR_DIAGNOSTIC_RUNTIME").unwrap());
    let image = PathBuf::from(std::env::var("MAILCRAFT_OCR_DIAGNOSTIC_IMAGE").unwrap());
    let work = PathBuf::from(std::env::var("MAILCRAFT_OCR_DIAGNOSTIC_WORK").unwrap());
    assert!(root.is_absolute() && image.is_absolute() && work.is_absolute() && work.is_dir());
    let bytes = std::fs::read(&image).unwrap();
    let sha = super::hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    let mut runtime = super::Runtime::open(&root).unwrap();
    let mut results = Vec::new();
    for order in ["eng+spa+ell", "spa+ell+eng", "ell+spa+eng", "eng", "spa", "ell"] {
        let selected: Vec<String> = order.split('+').map(str::to_owned).collect();
        assert!(
            selected.iter().all(|language| runtime
                .manifest
                .ocr
                .models
                .iter()
                .any(|model| model.path == Path::new(&format!("tessdata/{language}.traineddata"))))
        );
        runtime.manifest.ocr.languages = selected;
        let mut records = Vec::new();
        let report = crate::ocr::recognize(
            &runtime,
            &image,
            &sha,
            bytes.len() as u64,
            &work,
            serde_json::json!({"diagnostic":"language-order","imageSha256":sha}),
            &crate::process::Cancellation::default(),
            |record| {
                records.push(record);
                Ok(())
            },
        )
        .unwrap();
        results.push(serde_json::json!({"order":order,"report":report,"records":records}));
    }
    std::fs::write(work.join("language-orders.json"), serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    println!("{}", serde_json::to_string(&results).unwrap());
}
