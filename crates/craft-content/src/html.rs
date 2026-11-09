// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! HTML text for indexing, including hidden text beyond a renderer's preview
//! allowance. Tokenization neither builds a DOM nor executes scripts or CSS.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

#[derive(Default)]
struct State {
    text: RefCell<String>,
    raw: Cell<bool>,
    errors: Cell<u64>,
}

struct Sink(Rc<State>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        match token {
            Token::CharacterTokens(text) if !self.0.raw.get() => self.0.text.borrow_mut().push_str(&text),
            Token::NullCharacterToken if !self.0.raw.get() => self.0.text.borrow_mut().push('\u{fffd}'),
            Token::ParseError(_) => self.0.errors.set(self.0.errors.get().saturating_add(1)),
            Token::TagToken(tag) => {
                let name = tag.name.as_ref();
                if matches!(name, "script" | "style") {
                    self.0.raw.set(tag.kind == TagKind::StartTag);
                    if tag.kind == TagKind::StartTag {
                        return TokenSinkResult::RawData(if name == "script" { RawKind::ScriptData } else { RawKind::Rawtext });
                    }
                }
                if !self.0.raw.get() {
                    let mut text = self.0.text.borrow_mut();
                    if matches!(
                        name,
                        "p" | "div"
                            | "br"
                            | "hr"
                            | "tr"
                            | "td"
                            | "th"
                            | "li"
                            | "ul"
                            | "ol"
                            | "table"
                            | "section"
                            | "article"
                            | "header"
                            | "footer"
                            | "blockquote"
                            | "pre"
                            | "h1"
                            | "h2"
                            | "h3"
                            | "h4"
                            | "h5"
                            | "h6"
                    ) {
                        text.push('\n');
                    }
                    if name == "img" && tag.kind == TagKind::StartTag {
                        for attr in tag.attrs.iter().filter(|attr| &*attr.name.local == "alt") {
                            text.push_str(&attr.value);
                        }
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

pub struct HtmlText {
    tokenizer: Tokenizer<Sink>,
    queue: BufferQueue,
    state: Rc<State>,
}

impl Default for HtmlText {
    fn default() -> Self {
        let state = Rc::new(State::default());
        Self { tokenizer: Tokenizer::new(Sink(state.clone()), TokenizerOpts::default()), queue: BufferQueue::default(), state }
    }
}

impl HtmlText {
    pub fn feed(&self, text: &str) -> String {
        self.queue.push_back(StrTendril::from_slice(text));
        let _ = self.tokenizer.feed(&self.queue);
        std::mem::take(&mut *self.state.text.borrow_mut())
    }

    pub fn finish(self) -> (String, u64) {
        self.tokenizer.end();
        let text = std::mem::take(&mut *self.state.text.borrow_mut());
        (text, self.state.errors.get())
    }
}
