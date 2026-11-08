// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Presentation-only HTML formatting, streamed without a DOM or resource access.
//! Quotes/comments and raw-text elements remain literal; stored mail is untouched.

use std::io::{self, BufReader, Read, Write};
use std::iter::Peekable;

type Input<R> = Peekable<std::io::Bytes<BufReader<R>>>;
const TAG_BUFFER: usize = 4096;
const STACK: usize = 256;

pub(crate) fn pretty(input: impl Read, out: &mut dyn Write) -> io::Result<()> {
    let mut input = BufReader::with_capacity(64 << 10, input).bytes().peekable();
    let mut format = Format { out, line: 0, line_start: true, line_content: false, depth: 0, openings: Vec::new(), raw: None };
    while let Some(byte) = input.next().transpose()? {
        if let Some(raw) = format.raw {
            if byte != b'<' || peek(&mut input)? != Some(b'/') {
                format.emit(&[byte])?;
                continue;
            }
            let mut prefix = vec![b'<'];
            prefix.push(input.next().transpose()?.unwrap_or_default());
            let mut matched = true;
            for expected in raw.bytes() {
                match input.next().transpose()? {
                    Some(byte) => {
                        prefix.push(byte);
                        if !byte.eq_ignore_ascii_case(&expected) {
                            matched = false;
                            break;
                        }
                    }
                    None => {
                        matched = false;
                        break;
                    }
                }
            }
            let boundary = peek(&mut input)?.is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'));
            if matched && boundary {
                let tag = format.tag(&mut input, prefix, true)?;
                if tag.closed {
                    format.raw = None;
                }
            } else {
                format.emit(&prefix)?;
            }
            continue;
        }
        if byte != b'<' || !peek(&mut input)?.is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'/' | b'!' | b'?')) {
            format.emit(&[byte])?;
            continue;
        }
        let mut prefix = vec![b'<'];
        if peek(&mut input)? == Some(b'/') {
            prefix.push(input.next().transpose()?.unwrap_or_default());
            if !peek(&mut input)?.is_some_and(|byte| byte.is_ascii_alphabetic()) {
                format.emit(&prefix)?;
                continue;
            }
        }
        let tag = format.tag(&mut input, prefix, false)?;
        if tag.closed && !tag.closing && !tag.self_closing() {
            format.raw = match tag.name.as_slice() {
                b"script" => Some("script"),
                b"style" => Some("style"),
                b"pre" => Some("pre"),
                b"textarea" => Some("textarea"),
                _ => None,
            };
        }
    }
    Ok(())
}

fn peek<R: Read>(input: &mut Input<R>) -> io::Result<Option<u8>> {
    match input.peek() {
        Some(Ok(byte)) => Ok(Some(*byte)),
        Some(Err(_)) => input.next().transpose(),
        None => Ok(None),
    }
}

struct Format<'a> {
    out: &'a mut dyn Write,
    line: u64,
    line_start: bool,
    line_content: bool,
    depth: u32,
    openings: Vec<u64>,
    raw: Option<&'static str>,
}

impl Format<'_> {
    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        for byte in bytes {
            if *byte == b'\n' {
                self.line = self.line.saturating_add(1);
                self.line_start = true;
                self.line_content = false;
            } else {
                self.line_start = false;
                self.line_content |= !byte.is_ascii_whitespace();
            }
        }
        Ok(())
    }

    fn begin_tag(&mut self, tag: &Tag, preserve: bool) -> io::Result<u64> {
        let depth = if tag.closing { self.depth.saturating_sub(1) } else { self.depth };
        let opening = usize::try_from(depth).ok().and_then(|depth| self.openings.get(depth)).copied();
        if !preserve && tag.block() && (!tag.closing || opening != Some(self.line)) && self.line_content {
            self.emit(b"\n")?;
        }
        if !preserve && self.line_start {
            let spaces = [b' '; 32];
            let count = (depth.min(16) * 2) as usize;
            self.emit(spaces.get(..count).unwrap_or_default())?;
        }
        Ok(self.line)
    }

    fn tag<R: Read>(&mut self, input: &mut Input<R>, prefix: Vec<u8>, preserve: bool) -> io::Result<Tag> {
        let mut tag = Tag::new(prefix.starts_with(b"</"));
        let mut buffer = Vec::with_capacity(TAG_BUFFER);
        for byte in prefix {
            tag.consume(byte);
            buffer.push(byte);
        }
        let mut opening = None;
        loop {
            let next = input.next().transpose()?;
            if let Some(byte) = next {
                tag.closed = tag.consume(byte);
                buffer.push(byte);
            }
            if buffer.len() >= TAG_BUFFER || next.is_none() || tag.closed {
                if opening.is_none() {
                    opening = Some(self.begin_tag(&tag, preserve)?);
                }
                self.emit(&buffer)?;
                buffer.clear();
            }
            if next.is_none() || tag.closed {
                break;
            }
        }
        if tag.closed && !tag.name.is_empty() {
            if tag.closing {
                self.depth = self.depth.saturating_sub(1);
                if (self.depth as usize) < STACK {
                    self.openings.pop();
                }
            } else if !tag.self_closing() && !tag.void() {
                if (self.depth as usize) < STACK {
                    self.openings.push(opening.unwrap_or(self.line));
                }
                self.depth = self.depth.saturating_add(1);
            }
        }
        Ok(tag)
    }
}

struct Tag {
    name: Vec<u8>,
    name_done: bool,
    closing: bool,
    closed: bool,
    count: usize,
    prefix: Vec<u8>,
    last_three: Vec<u8>,
    comment: bool,
    quote: Option<u8>,
    last: u8,
}

impl Tag {
    fn new(closing: bool) -> Self {
        Self {
            name: Vec::with_capacity(64),
            name_done: false,
            closing,
            closed: false,
            count: 0,
            prefix: Vec::with_capacity(4),
            last_three: Vec::with_capacity(3),
            comment: false,
            quote: None,
            last: 0,
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.prefix.len() < 4 {
            self.prefix.push(byte);
            self.comment = self.prefix == b"<!--";
        }
        if self.count >= if self.closing { 2 } else { 1 } && !self.name_done {
            if (byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')) && self.name.len() < 64 {
                self.name.push(byte.to_ascii_lowercase());
            } else {
                self.name_done = true;
            }
        }
        self.count = self.count.saturating_add(1);
        if self.comment {
            self.last_three.push(byte);
            if self.last_three.len() > 3 {
                self.last_three.remove(0);
            }
            return self.last_three == b"-->";
        }
        if let Some(quote) = self.quote {
            if byte == quote {
                self.quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            self.quote = Some(byte);
        } else if byte == b'>' {
            return true;
        }
        if !byte.is_ascii_whitespace() {
            self.last = byte;
        }
        false
    }

    fn self_closing(&self) -> bool {
        self.last == b'/'
    }
    fn void(&self) -> bool {
        matches!(
            self.name.as_slice(),
            b"area" | b"base" | b"br" | b"col" | b"embed" | b"hr" | b"img" | b"input" | b"link" | b"meta" | b"param" | b"source" | b"track" | b"wbr"
        )
    }
    fn block(&self) -> bool {
        !matches!(
            self.name.as_slice(),
            b"a" | b"abbr"
                | b"b"
                | b"bdi"
                | b"bdo"
                | b"br"
                | b"cite"
                | b"code"
                | b"data"
                | b"del"
                | b"dfn"
                | b"em"
                | b"i"
                | b"img"
                | b"input"
                | b"ins"
                | b"kbd"
                | b"label"
                | b"q"
                | b"ruby"
                | b"s"
                | b"samp"
                | b"small"
                | b"span"
                | b"strong"
                | b"sub"
                | b"sup"
                | b"time"
                | b"u"
                | b"var"
                | b"wbr"
        )
    }
}
