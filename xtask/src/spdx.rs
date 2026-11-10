// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask spdx [--fix]`: every Rust source file starts with its copyright and licence
//! (REUSE-style SPDX lines), so a file copied out of the repository still says whose it is and
//! under what terms. `--fix` adds the lines to the files that lack them.

use std::path::Path;
use std::process::Command;

pub const HEADER: &str = "// SPDX-FileCopyrightText: MailCraft contributors\n// SPDX-License-Identifier: MIT OR Apache-2.0\n";

// These seven imported adapters retain their actual MIT copyright/licence.
// Their exact source and original hashes are recorded in native/craft-content/
// sources.json. No directory or source file is exempt from the header check.
fn expected_header(path: &str) -> &'static str {
    if path.starts_with("vendor/winit/") {
        return "// SPDX-FileCopyrightText: The winit contributors\n// SPDX-License-Identifier: Apache-2.0\n";
    }
    if path.starts_with("vendor/cosca/") {
        return "// SPDX-FileCopyrightText: Anna Zhukova\n// SPDX-License-Identifier: Apache-2.0\n";
    }
    match path {
        "native/craft-content/adapters/gridcraft/engine.rs" => {
            "// SPDX-FileCopyrightText: 2026 Protagonist Labs, Pathweft and MailCraft contributors\n// SPDX-License-Identifier: MIT\n"
        }
        "native/craft-content/adapters/wordcraft/engine.rs"
        | "native/craft-content/adapters/deckcraft/engine.rs"
        | "native/craft-content/adapters/pdfcraft/engine.rs"
        | "native/craft-content/adapters/photocraft/engine.rs"
        | "native/craft-content/adapters/vectorcraft/engine.rs"
        | "native/craft-content/adapters/figcraft/engine.rs" => {
            "// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors\n// SPDX-License-Identifier: MIT\n"
        }
        _ => HEADER,
    }
}

/// Whether `text` starts with the exact required lines, whatever its line endings (a Windows
/// checkout has CRLF).
fn has_header(text: &str, expected: &str) -> bool {
    let mut lines = text.lines();
    expected.lines().all(|want| lines.next() == Some(want))
}

/// Rust files lacking their exact first-party or preserved imported SPDX header.
pub fn missing(files: &[String], read: impl Fn(&str) -> Option<String>) -> Vec<&str> {
    files
        .iter()
        .filter(|f| f.ends_with(".rs"))
        .filter(|f| read(f).is_some_and(|text| !has_header(&text, expected_header(f))))
        .map(String::as_str)
        .collect()
}

pub fn run(root: &Path, fix: bool) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "*.rs"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    let files: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_owned).collect();
    let read = |f: &str| std::fs::read_to_string(root.join(f)).ok();
    let miss = missing(&files, read);
    if miss.is_empty() {
        println!("spdx: all {} Rust files carry their copyright and licence", files.len());
        return Ok(());
    }
    if !fix {
        return Err(format!("{} Rust file(s) lack the SPDX lines (run `cargo xtask spdx --fix`):\n  {}", miss.len(), miss.join("\n  ")));
    }
    for f in &miss {
        let path = root.join(f);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{f}: {e}"))?;
        std::fs::write(&path, format!("{}{text}", expected_header(f))).map_err(|e| format!("{f}: {e}"))?;
    }
    println!("spdx: added the lines to {} file(s)", miss.len());
    Ok(())
}

#[cfg(test)]
#[path = "spdx_import_tests.rs"]
mod import_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_rust_files_without_the_lines() {
        let files = vec!["a.rs".to_string(), "b.rs".into(), "c.md".into(), "gone.rs".into(), "crlf.rs".into()];
        let read = |f: &str| match f {
            "a.rs" => Some(format!("{HEADER}fn a() {{}}\n")),
            "b.rs" => Some("//! b\n".into()),
            "crlf.rs" => Some(HEADER.replace('\n', "\r\n")),
            "c.md" => Some("# c\n".into()),
            _ => None,
        };
        assert_eq!(missing(&files, read), vec!["b.rs"]);
    }
}
