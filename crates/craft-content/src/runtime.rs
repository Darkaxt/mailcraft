// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Integrity-bound native worker payloads. Paths come only from the selected
//! product package and an exact caller-owned scratch root, never from an email.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{self, Operation, Rendered, Reply, Request, ResultValue};
use crate::{
    Batch, Cursor, VERSION,
    process::{Cancellation, ManagedProcess},
};

type Result<T> = std::result::Result<T, String>;

/// Generous wall clock for one native core response (open, text batch, page
/// render or close). A hung core is ended and reported as a resourceLimit gap
/// instead of blocking the single content writer.
pub const RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payload {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Core {
    pub id: String,
    pub executable: PathBuf,
    pub source: String,
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ocr {
    pub executable: PathBuf,
    pub version: String,
    pub languages: Vec<String>,
    pub models: Vec<Payload>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub payloads: Vec<Payload>,
    pub cores: Vec<Core>,
    pub ocr: Ocr,
}

pub struct Runtime {
    pub manifest: Manifest,
    pub revision: String,
    root: PathBuf,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn regular(path: &Path) -> Result<File> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|why| why.to_string())?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
        return Err("a Craft payload or input is not a regular, non-reparse file".into());
    }
    let file = File::open(path).map_err(|why| why.to_string())?;
    if !file.metadata().map_err(|why| why.to_string())?.is_file() {
        return Err("a Craft input changed from a regular file".into());
    }
    Ok(file)
}

fn member(root: &Path, relative: &Path) -> Result<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    if relative.as_os_str().is_empty() || relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err("Craft runtime payload names must be relative package members".into());
    }
    let mut target = root.to_path_buf();
    for part in relative.components() {
        target.push(part.as_os_str());
        let metadata = std::fs::symlink_metadata(&target).map_err(|why| why.to_string())?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("a Craft runtime payload path contains a reparse point".into());
        }
    }
    Ok(target)
}

pub fn verify(path: &Path, expected: &str, bytes: u64) -> Result<()> {
    let mut file = regular(path)?;
    if file.metadata().map_err(|why| why.to_string())?.len() != bytes {
        return Err("the Craft payload length differs from its selected manifest".into());
    }
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 32 * 1024];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|why| why.to_string())?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or("Craft payload size overflow")?;
        if total > bytes {
            return Err("the Craft payload grew while being verified".into());
        }
        hash.update(buffer.get(..count).ok_or("invalid Craft payload read")?);
    }
    if total != bytes || hex(hash.finish().as_ref()) != expected {
        return Err("the Craft payload SHA-256 differs from its selected manifest".into());
    }
    Ok(())
}

impl Runtime {
    pub fn open(root: &Path) -> Result<Self> {
        use std::os::windows::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(root).map_err(|why| why.to_string())?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err("the selected Craft runtime root must be a non-reparse directory".into());
        }
        let root = root.canonicalize().map_err(|why| why.to_string())?;
        let mut file = regular(&root.join("runtime.json"))?;
        if file.metadata().map_err(|why| why.to_string())?.len() > 1024 * 1024 {
            return Err("the Craft runtime manifest is oversized".into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|why| why.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("the Craft runtime manifest grew while being read".into());
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|why| why.to_string())?;
        if manifest.version != VERSION || manifest.ocr.languages.is_empty() {
            return Err("the Craft runtime has another version or no required extraction/OCR payload".into());
        }
        let revision = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
        for payload in manifest.payloads.iter().chain(&manifest.ocr.models) {
            verify(&member(&root, &payload.path)?, &payload.sha256, payload.bytes)?;
        }
        for executable in manifest.cores.iter().map(|core| &core.executable).chain(std::iter::once(&manifest.ocr.executable)) {
            if !manifest.payloads.iter().any(|payload| &payload.path == executable) {
                return Err("a Craft executable is absent from the verified payload set".into());
            }
        }
        if manifest.ocr.languages.iter().any(|language| {
            language.is_empty()
                || !language.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !manifest.ocr.models.iter().any(|model| model.path == Path::new(&format!("tessdata/{language}.traineddata")))
        }) {
            return Err("an OCR language has no matching verified model".into());
        }
        Ok(Self { manifest, revision, root })
    }

    pub fn core(&self, extension: &str) -> Option<&Core> {
        self.manifest.cores.iter().find(|core| core.extensions.iter().any(|allowed| allowed == extension))
    }

    pub fn start(&self, core: &Core, scratch: &Path, cancellation: &Cancellation) -> Result<Client> {
        let payload = self
            .manifest
            .payloads
            .iter()
            .find(|payload| payload.path == core.executable)
            .ok_or("the requested core is not a selected package executable")?;
        let executable = member(&self.root, &payload.path)?;
        verify(&executable, &payload.sha256, payload.bytes)?;
        let stderr = tempfile::tempfile_in(scratch).map_err(|why| why.to_string())?;
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr.try_clone().map_err(|why| why.to_string())?)
            .current_dir(scratch)
            .env("TEMP", scratch)
            .env("TMP", scratch)
            .env("OMP_THREAD_LIMIT", "1")
            .env("OMP_NUM_THREADS", "1")
            .env("RAYON_NUM_THREADS", "1");
        let mut process = ManagedProcess::spawn(command, cancellation)?;
        let stdin = process.stdin().ok_or("the owned Craft worker has no input pipe")?;
        let stdout = process.stdout().ok_or("the owned Craft worker has no output pipe")?;
        Ok(Client { process, stdin: Some(stdin), stdout: BufReader::new(stdout), stderr, next: 0, failed: false, closed: false })
    }

    pub(crate) fn ocr_executable(&self) -> Result<PathBuf> {
        let payload = self
            .manifest
            .payloads
            .iter()
            .find(|payload| payload.path == self.manifest.ocr.executable)
            .ok_or("the OCR executable is not selected in this package")?;
        let executable = member(&self.root, &payload.path)?;
        verify(&executable, &payload.sha256, payload.bytes)?;
        Ok(executable)
    }

    pub(crate) fn tessdata(&self) -> Result<PathBuf> {
        external_directory(&self.root.join("tessdata"))
    }
}

// Tesseract appends forward-slash model names through the C runtime. Windows
// verbatim paths disable slash conversion, so pass the equivalent ordinary
// spelling at this external interface, after proving it names the same directory.
fn external_directory(path: &Path) -> Result<PathBuf> {
    use std::path::Prefix;
    let mut components = path.components();
    let mut ordinary = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
            Prefix::VerbatimUNC(server, share) => {
                let mut result = PathBuf::from(r"\\");
                result.push(server);
                result.push(share);
                result
            }
            Prefix::Disk(_) | Prefix::UNC(_, _) => path.to_path_buf(),
            _ => return Err("the OCR model directory has an unsupported Windows namespace".into()),
        },
        _ => return Err("the OCR model directory is not an absolute Windows path".into()),
    };
    if matches!(path.components().next(), Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim()) {
        for component in components {
            if !matches!(component, Component::RootDir) {
                ordinary.push(component.as_os_str());
            } else if !ordinary.has_root() {
                ordinary.push(r"\");
            }
        }
    }
    if ordinary.canonicalize().map_err(|why| why.to_string())? != path.canonicalize().map_err(|why| why.to_string())? {
        return Err("the external OCR model path does not name its verified directory".into());
    }
    Ok(ordinary)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

pub struct Client {
    process: ManagedProcess,
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    stderr: File,
    next: u64,
    failed: bool,
    closed: bool,
}

impl Client {
    fn call(&mut self, operation: Operation) -> Result<ResultValue> {
        if self.failed || self.closed {
            return Err("this Craft worker has ended or has an unresolved operation".into());
        }
        self.next = self.next.checked_add(1).ok_or("Craft request identity overflow")?;
        let request = Request { version: VERSION, id: self.next, operation };
        self.process.deadline(Some(RESPONSE_DEADLINE));
        let result = protocol::write(self.stdin.as_mut().ok_or("the Craft worker input is closed")?, &request, protocol::MAX_REQUEST)
            .and_then(|_| protocol::read::<Reply>(&mut self.stdout, protocol::MAX_REPLY))
            .and_then(|reply| reply.ok_or_else(|| "the Craft worker exited without its original response".into()))
            .and_then(|reply| reply.check(request.id));
        self.process.deadline(None);
        let result = match result {
            Err(why) if self.process.expired() => Err(format!(
                "resourceLimit: the native core did not answer within its {}-second response deadline and was ended; {why}",
                RESPONSE_DEADLINE.as_secs()
            )),
            result => result,
        };
        if result.is_err() {
            self.failed = true;
            self.process.abort();
            self.stdin.take();
            let exit = self.process.wait();
            let diagnostics = self.diagnostics().unwrap_or_else(|why| format!("native diagnostics unavailable: {why}"));
            return result.map_err(|why| {
                format!("{why}; original worker PID {}; exit observed: {}; diagnostics: {diagnostics}", self.process.id(), exit.is_ok())
            });
        }
        result
    }

    pub fn open(&mut self, input: &Path, sha256: &str, extension: &str, scratch: &Path) -> Result<Value> {
        match self.call(Operation::Open { input: input.into(), sha256: sha256.into(), extension: extension.into(), scratch: scratch.into() })? {
            ResultValue::Opened(info) => Ok(info),
            _ => Err("the native worker did not acknowledge document opening".into()),
        }
    }

    pub fn extract(&mut self, cursor: Cursor) -> Result<Batch> {
        match self.call(Operation::Extract { cursor, limit: crate::MAX_BATCH })? {
            ResultValue::Text(batch) => {
                batch.check()?;
                Ok(batch)
            }
            _ => Err("the native worker did not return a located text batch".into()),
        }
    }

    pub fn render(&mut self, part: u32, page: u32) -> Result<Rendered> {
        match self.call(Operation::Render { part, page, width: 3000 })? {
            ResultValue::Rendered(rendered) => Ok(rendered),
            _ => Err("the native worker did not return a rendered page".into()),
        }
    }

    /// Close the document and confirm the tree's exit. Native stderr warnings
    /// after a successful close are returned as a note, not a failure.
    pub fn finish(mut self) -> Result<String> {
        if !matches!(self.call(Operation::Close)?, ResultValue::Closed) {
            return Err("the Craft worker did not acknowledge document close".into());
        }
        self.closed = true;
        self.stdin.take();
        self.process.deadline(Some(RESPONSE_DEADLINE));
        let status = self.process.wait()?;
        if !status.success() {
            if self.process.expired() {
                return Err(format!(
                    "resourceLimit: the Craft worker did not exit within its {}-second deadline after close",
                    RESPONSE_DEADLINE.as_secs()
                ));
            }
            return Err("the Craft worker failed while closing its original process tree".into());
        }
        Ok(self.diagnostics()?.trim().to_owned())
    }

    fn diagnostics(&mut self) -> Result<String> {
        self.stderr.seek(SeekFrom::Start(0)).map_err(|why| why.to_string())?;
        let mut text = String::new();
        Read::by_ref(&mut self.stderr).take(8193).read_to_string(&mut text).map_err(|why| why.to_string())?;
        if text.len() > 8192 {
            return Err("native diagnostics exceed the explicit reporting allowance".into());
        }
        Ok(text)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // EOF is the worker's normal shutdown protocol. A cancelled/malformed
        // transaction already aborted its exact owned job in call().
        self.stdin.take();
        let _ = self.process.wait();
    }
}
