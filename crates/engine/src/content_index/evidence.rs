// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit finite offline qualification evidence. The GUI never enables this
//! process-only sink. No environment variable or mailbox setting enables it.

use std::cell::{Cell, RefCell};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub struct Evidence {
    root: PathBuf,
    manifest: RefCell<File>,
    bytes: Cell<u64>,
}

impl Evidence {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        use std::os::windows::fs::MetadataExt;
        if !root.is_absolute() || root.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
            return Err("render evidence needs an exact absolute qualification directory".into());
        }
        for path in root.ancestors() {
            let metadata = std::fs::symlink_metadata(path).map_err(|why| why.to_string())?;
            if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
                return Err("the render-evidence directory must not contain a reparse binding".into());
            }
        }
        let manifest = OpenOptions::new().write(true).create_new(true).open(root.join("renders.jsonl")).map_err(|why| why.to_string())?;
        Ok(Self { root, manifest: RefCell::new(manifest), bytes: Cell::new(0) })
    }

    pub fn capture(&self, input: &Path, sha256: &str, bytes: u64, location: Value) -> Result<(), String> {
        let total = self.bytes.get().checked_add(bytes).ok_or("render evidence size overflow")?;
        if total > 64 * 1024 * 1024 {
            return Err("finite render evidence exceeds its explicit 64 MiB allowance".into());
        }
        craft_content::runtime::verify(input, sha256, bytes)?;
        let destination = self.root.join(format!("{sha256}.png"));
        if destination.exists() {
            craft_content::runtime::verify(&destination, sha256, bytes)?;
        } else {
            let mut temporary = tempfile::NamedTempFile::new_in(&self.root).map_err(|why| why.to_string())?;
            let mut source = File::open(input).map_err(|why| why.to_string())?;
            let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
            let mut buffer = [0u8; 32 * 1024];
            let mut copied = 0u64;
            loop {
                let count = source.read(&mut buffer).map_err(|why| why.to_string())?;
                if count == 0 {
                    break;
                }
                let chunk = buffer.get(..count).ok_or("invalid render evidence read")?;
                copied = copied.checked_add(count as u64).ok_or("render evidence size overflow")?;
                if copied > bytes {
                    return Err("render evidence source grew during capture".into());
                }
                hash.update(chunk);
                temporary.write_all(chunk).map_err(|why| why.to_string())?;
            }
            if copied != bytes || super::input::hex(hash.finish().as_ref()) != sha256 {
                return Err("the captured render differs from its original native identity".into());
            }
            temporary.as_file().sync_all().map_err(|why| why.to_string())?;
            temporary.persist_noclobber(destination).map_err(|why| why.to_string())?;
        }
        let mut manifest = self.manifest.try_borrow_mut().map_err(|why| why.to_string())?;
        serde_json::to_writer(&mut *manifest, &json!({"file":format!("{sha256}.png"),"sha256":sha256,"bytes":bytes,"location":location}))
            .map_err(|why| why.to_string())?;
        manifest.write_all(b"\n").and_then(|_| manifest.flush()).and_then(|_| manifest.sync_all()).map_err(|why| why.to_string())?;
        self.bytes.set(total);
        Ok(())
    }
}
