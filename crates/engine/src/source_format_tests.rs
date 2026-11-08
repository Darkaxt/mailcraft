// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::io::{self, Cursor, Write};

fn pretty(input: &str) -> String {
    let mut out = Vec::new();
    super::source_format::pretty(Cursor::new(input.as_bytes()), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn source_formatting_indents_blocks_without_rewriting_inline_text_or_quoted_angles() {
    let source = "<div><p title=\"1 > 0 and < 2\">Hello <b>café</b>.</p><p>Next</p></div>";
    assert_eq!(pretty(source), "<div>\n  <p title=\"1 > 0 and < 2\">Hello <b>café</b>.</p>\n  <p>Next</p>\n</div>");
}

#[test]
fn raw_text_and_comments_remain_literal_and_malformed_markup_is_not_discarded() {
    let source = "<div><script>if(a<b){x=\"<p>\";}\n</script><!-- a <b> > --><pre> one\n  two <b>raw</b></pre><textarea><x>\n</textarea></div>";
    let shown = pretty(source);
    for literal in ["<script>if(a<b){x=\"<p>\";}\n</script>", "<!-- a <b> > -->", "<pre> one\n  two <b>raw</b></pre>", "<textarea><x>\n</textarea>"] {
        assert!(shown.contains(literal), "{shown}");
    }
    let malformed = "<div data-x=\"unterminated > <p>still data";
    assert_eq!(pretty(malformed), malformed);
}

struct Bounded(u64);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(bytes.len() <= 4096);
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn long_unterminated_attributes_and_deep_markup_keep_a_bounded_working_buffer() {
    let source = format!("{}<p title=\"{}", "<div>".repeat(10_000), "x".repeat(1 << 20));
    let mut out = Bounded(0);
    super::source_format::pretty(Cursor::new(source.as_bytes()), &mut out).unwrap();
    assert!(out.0 >= source.len() as u64 && out.0 < source.len() as u64 * 4);
}
