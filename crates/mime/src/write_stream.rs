// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded RFC 5322/MIME export from retained or in-memory content readers.

use std::io::{self, BufRead, BufReader, Read, Write};

use mailcraft_model::Message;

use crate::{WriteOptions, header::write_field, media, transfer, write};

/// The existing message part that an export reads, never a caller's file path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentPart {
    Text,
    Html,
    Attachment(usize),
    /// The complete canonical signed/encrypted MIME retained when it arrived.
    Original,
}

type Fields = Vec<(&'static str, String)>;

#[derive(Clone, Copy)]
enum Encoding {
    Base64,
    SevenBit,
    Binary,
}

enum Body {
    Leaf { fields: Fields, part: ContentPart, encoding: Encoding, text: bool },
    Multipart { fields: Fields, boundary: String, children: Vec<Body> },
}

impl Body {
    fn fields(&self) -> &Fields {
        match self {
            Self::Leaf { fields, .. } | Self::Multipart { fields, .. } => fields,
        }
    }
    fn binary(&self) -> bool {
        match self {
            Self::Leaf { encoding, .. } => matches!(encoding, Encoding::Binary),
            Self::Multipart { children, .. } => children.iter().any(Self::binary),
        }
    }
}

/// Write one complete reconstructed message without assembling encoded bodies
/// or attachments in memory. The caller supplies a fresh 128-bit hexadecimal
/// boundary nonce and readers that verify retained content through EOF. An error
/// leaves an incomplete output which the caller must not publish as a full file.
pub fn write_to<'a>(
    message: &Message,
    opts: &WriteOptions,
    nonce: &str,
    open: &mut dyn FnMut(ContentPart) -> io::Result<Box<dyn Read + 'a>>,
    out: &mut dyn Write,
) -> io::Result<()> {
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a fresh 128-bit boundary nonce is required"));
    }
    if message.original.is_some() && !message.draft {
        return write_original(message, opts, open, out);
    }
    let mut counter = 0u32;
    let calendar = calendar_form(message, open)?;
    let has_html = message.body.html.is_some() || message.kept.as_ref().is_some_and(|kept| kept.html.is_some());
    let has_text = !message.body.text.is_empty() || message.kept.as_ref().and_then(|kept| kept.text).is_some_and(|part| part.len > 0);
    let text = |kind, part| Body::Leaf {
        fields: vec![("Content-Type", format!("text/{kind}; charset=utf-8")), ("Content-Transfer-Encoding", "base64".into())],
        part,
        encoding: Encoding::Base64,
        text: true,
    };
    let mut inline = Vec::new();
    let mut files = Vec::new();
    for (index, attachment) in message.attachments.iter().enumerate() {
        if calendar.as_ref().is_some_and(|(part, _)| *part == index) {
            continue;
        }
        let mut fields = write::attachment_fields(attachment);
        let encoding = match media::safe_media_type(&attachment.mime).as_str() {
            // RFC2046 section5.2.1 forbids Base64 for attached messages.
            "message/rfc822" => Encoding::Binary,
            // RFC3464 section2.1 requires readable 7bit delivery-status parts.
            "message/delivery-status" => Encoding::SevenBit,
            _ => Encoding::Base64,
        };
        for (name, value) in &mut fields {
            if *name == "Content-Transfer-Encoding" {
                *value = match encoding {
                    Encoding::Base64 => "base64",
                    Encoding::SevenBit => "7bit",
                    Encoding::Binary => "binary",
                }
                .into();
            }
        }
        let leaf = Body::Leaf { fields, part: ContentPart::Attachment(index), encoding, text: false };
        if has_html && attachment.inline {
            inline.push(leaf);
        } else {
            files.push(leaf);
        }
    }
    let mut forms = Vec::new();
    if !has_html || has_text {
        forms.push(text("plain", ContentPart::Text));
    }
    if has_html {
        let html = text("html", ContentPart::Html);
        let rich = if inline.is_empty() {
            html
        } else {
            let mut related = vec![html];
            related.extend(inline);
            multipart("related", "; type=\"text/html\"", related, nonce, &mut counter)
        };
        forms.push(rich);
    }
    if let Some((index, method)) = calendar {
        forms.push(Body::Leaf {
            fields: vec![
                ("Content-Type", format!("text/calendar; method={method}")),
                ("Content-Transfer-Encoding", "base64".into()),
                ("Content-Disposition", "inline".into()),
            ],
            part: ContentPart::Attachment(index),
            encoding: Encoding::Base64,
            text: false,
        });
    }
    let body = if forms.len() == 1 { forms.remove(0) } else { multipart("alternative", "", forms, nonce, &mut counter) };
    let body = if files.is_empty() {
        body
    } else {
        let mut mixed = vec![body];
        mixed.extend(files);
        multipart("mixed", "", mixed, nonce, &mut counter)
    };
    let mut headers = write::headers(message, opts);
    write_field(&mut headers, "MIME-Version", "1.0");
    out.write_all(&headers)?;
    write_body(&body, open, out, &format!("=_mc_{nonce}_"))
}

/// Retained protected MIME already has canonical line endings (or binary parts
/// whose bytes must never change). Only the top-level headers are reconstructed;
/// the signed/encrypted body is copied unchanged and the reader validates EOF.
fn write_original<'a>(
    message: &Message,
    opts: &WriteOptions,
    open: &mut dyn FnMut(ContentPart) -> io::Result<Box<dyn Read + 'a>>,
    out: &mut dyn Write,
) -> io::Result<()> {
    const MAX_HEADERS: usize = 4 << 20;
    const CHUNK: usize = 64 << 10;
    let original = open(ContentPart::Original)?;
    let mut original = BufReader::with_capacity(CHUNK, original.take(mailcraft_model::MAX_ORIGINAL as u64 + 1));
    let mut headers = Vec::new();
    loop {
        let start = headers.len();
        let room = MAX_HEADERS.saturating_sub(start).saturating_add(1);
        let count = Read::by_ref(&mut original).take(room as u64).read_until(b'\n', &mut headers)?;
        if headers.len() > MAX_HEADERS || count == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the retained original has an incomplete or oversized MIME header"));
        }
        if matches!(headers.get(start..), Some(b"\r\n" | b"\n")) {
            break;
        }
    }
    let (_, unexpected_body) = crate::header::split(&headers, true);
    if !unexpected_body.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the retained original has a malformed MIME header"));
    }
    let (fields, _) = crate::protected::content_of(&headers)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the retained original's MIME fields cannot be written safely"))?;
    for chunk in write::headers(message, opts).chunks(CHUNK) {
        out.write_all(chunk)?;
    }
    for chunk in fields.chunks(CHUNK) {
        out.write_all(chunk)?;
    }
    out.write_all(b"\r\n")?;
    let body_bytes = io::copy(&mut original, out)?;
    if (headers.len() as u64).saturating_add(body_bytes) > mailcraft_model::MAX_ORIGINAL as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the retained original exceeds its allowed size"));
    }
    Ok(())
}

fn multipart(kind: &str, params: &str, children: Vec<Body>, nonce: &str, counter: &mut u32) -> Body {
    let boundary = format!("=_mc_{nonce}_{counter}");
    *counter = counter.saturating_add(1);
    let mut fields = vec![("Content-Type", format!("multipart/{kind}{params}; boundary=\"{boundary}\""))];
    if children.iter().any(Body::binary) {
        fields.push(("Content-Transfer-Encoding", "binary".into()));
    }
    Body::Multipart { fields, boundary, children }
}

fn calendar_form<'a>(message: &Message, open: &mut dyn FnMut(ContentPart) -> io::Result<Box<dyn Read + 'a>>) -> io::Result<Option<(usize, String)>> {
    for (index, part) in message.attachments.iter().enumerate() {
        if !part.inline || !part.content_id.is_empty() || media::safe_media_type(&part.mime) != "text/calendar" {
            continue;
        }
        let input = open(ContentPart::Attachment(index))?;
        let mut reader = BufReader::new(input.take(64 << 10));
        let mut line = Vec::new();
        for _ in 0..100 {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if let Ok(line) = std::str::from_utf8(&line)
                && let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("METHOD")
            {
                let method = value.trim();
                if !method.is_empty() && method.len() <= 16 && method.bytes().all(|b| b.is_ascii_alphabetic()) {
                    return Ok(Some((index, method.to_ascii_uppercase())));
                }
            }
        }
    }
    Ok(None)
}

fn write_body<'a>(
    body: &Body,
    open: &mut dyn FnMut(ContentPart) -> io::Result<Box<dyn Read + 'a>>,
    out: &mut dyn Write,
    forbidden: &str,
) -> io::Result<()> {
    let mut headers = Vec::new();
    for (name, value) in body.fields() {
        write_field(&mut headers, name, value);
    }
    out.write_all(&headers)?;
    out.write_all(b"\r\n")?;
    match body {
        Body::Multipart { boundary, children, .. } => {
            for child in children {
                write!(out, "--{boundary}\r\n")?;
                write_body(child, open, out, forbidden)?;
                out.write_all(b"\r\n")?;
            }
            write!(out, "--{boundary}--\r\n")
        }
        Body::Leaf { part, encoding, text, .. } => {
            let reader = open(*part)?;
            match encoding {
                Encoding::SevenBit => copy_raw(reader, out, forbidden.as_bytes(), true),
                Encoding::Binary => copy_raw(reader, out, forbidden.as_bytes(), false),
                Encoding::Base64 if *text => base64_lines(CanonicalText::new(reader), out),
                Encoding::Base64 => base64_lines(reader, out),
            }
        }
    }
}

fn base64_lines(mut reader: impl Read, out: &mut dyn Write) -> io::Result<()> {
    let mut block = [0u8; 57]; // RFC 2045:57 bytes encode to76 columns.
    loop {
        let mut used = 0;
        while used < block.len() {
            match reader.read(&mut block[used..]) {
                Ok(0) => break,
                Ok(count) => used += count,
                Err(why) if why.kind() == io::ErrorKind::Interrupted => continue,
                Err(why) => return Err(why),
            }
        }
        if used == 0 {
            return Ok(());
        }
        out.write_all(&transfer::base64_lines(&block[..used]))?;
        out.write_all(b"\r\n")?;
    }
}

fn copy_raw(mut reader: impl Read, out: &mut dyn Write, forbidden: &[u8], seven_bit: bool) -> io::Result<()> {
    let mut block = [0u8; 64 << 10];
    let mut overlap = Vec::new();
    let mut line_bytes = 0usize;
    let mut carriage_return = false;
    loop {
        let count = reader.read(&mut block)?;
        if count == 0 {
            if seven_bit && carriage_return {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a 7bit MIME part ends with a bare carriage return"));
            }
            return Ok(());
        }
        if seven_bit {
            for &byte in &block[..count] {
                if carriage_return {
                    if byte != b'\n' {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "a 7bit MIME part has a bare carriage return"));
                    }
                    carriage_return = false;
                    line_bytes = 0;
                    continue;
                }
                match byte {
                    b'\r' => carriage_return = true,
                    0 | b'\n' | 0x80..=0xff => {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "a MIME part is not valid 7bit content"));
                    }
                    _ => {
                        line_bytes += 1;
                        if line_bytes > 998 {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, "a 7bit MIME part has a line longer than 998 bytes"));
                        }
                    }
                }
            }
        }
        overlap.extend_from_slice(&block[..count]);
        if overlap.windows(forbidden.len()).any(|bytes| bytes == forbidden) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a raw MIME part conflicts with the export boundary"));
        }
        out.write_all(&block[..count])?;
        let keep = forbidden.len().saturating_sub(1).min(overlap.len());
        overlap.drain(..overlap.len() - keep);
    }
}

struct CanonicalText<R> {
    input: BufReader<R>,
    skip_lf: bool,
    pending_lf: bool,
}

impl<R: Read> CanonicalText<R> {
    fn new(input: R) -> Self {
        Self { input: BufReader::new(input), skip_lf: false, pending_lf: false }
    }
}

impl<R: Read> Read for CanonicalText<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut used = 0;
        while used < out.len() {
            if self.pending_lf {
                self.pending_lf = false;
                out[used] = b'\n';
                used += 1;
                continue;
            }
            let mut byte = [0u8; 1];
            if self.input.read(&mut byte)? == 0 {
                break;
            }
            let byte = byte[0];
            let skipped = self.skip_lf && byte == b'\n';
            self.skip_lf = false;
            if skipped {
                continue;
            }
            if byte == b'\r' {
                self.skip_lf = true;
                self.pending_lf = true;
                out[used] = b'\r';
            } else if byte == b'\n' {
                out[used] = b'\r';
                self.pending_lf = true;
            } else {
                out[used] = byte;
            }
            used += 1;
        }
        Ok(used)
    }
}
