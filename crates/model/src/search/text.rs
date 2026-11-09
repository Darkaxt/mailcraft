// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exact source verification after persistent full-text candidate lookup.
//! Quoted text retains punctuation, accents and spacing; only requested case
//! folding changes its comparison. Word searches keep the existing accent and
//! prefix behavior without silently shortening words to 32 characters.

use std::collections::VecDeque;

use caseless::Caseless;

use crate::words;

/// A query bound, never a limit on document content or result counts.
pub const MAX_QUERY_BYTES: usize = 16 * 1024;
pub const MAX_QUERY_WORDS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextTerm {
    pub fields: u8,
    pub source: String,
    pub literal: bool,
    pub case_sensitive: bool,
    wanted: Vec<String>,
    needle: Vec<char>,
    fallback: Vec<usize>,
}

/// Full Unicode default case folding, without removing punctuation or accents.
/// Index callers pass bounded source segments; matching itself is streaming.
pub fn fold_literal(text: &str) -> String {
    text.chars().default_case_fold().collect()
}

/// The existing word-search accent folding, applied to complete source text.
pub fn fold_words(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars().default_case_fold().filter(|ch| !words::is_mark(*ch)) {
        if let Some(plain) = words::plain(ch) {
            result.push_str(plain);
        } else {
            result.push(ch);
        }
    }
    result
}

impl TextTerm {
    pub fn new(fields: u8, source: String, literal: bool, case_sensitive: bool) -> Result<Self, &'static str> {
        if source.len() > MAX_QUERY_BYTES {
            return Err("search text is longer than 16384 UTF-8 bytes");
        }
        let mut wanted = Vec::new();
        if !literal {
            // The input is already query-bounded. This bound never discards a
            // source word: it only bounds the prefix needed for verification.
            each_prefix(&source, case_sensitive, MAX_QUERY_BYTES, true, |word| {
                wanted.push(word.to_owned());
                false
            });
            if wanted.len() > MAX_QUERY_WORDS {
                return Err("a search term contains more than 64 words");
            }
        }
        let needle: Vec<char> = if case_sensitive { source.chars().collect() } else { source.chars().default_case_fold().collect() };
        let mut fallback = vec![0; needle.len()];
        let mut matched = 0;
        for (index, &ch) in needle.iter().enumerate().skip(1) {
            while matched > 0 && needle.get(matched) != Some(&ch) {
                matched = fallback.get(matched - 1).copied().unwrap_or(0);
            }
            if needle.get(matched) == Some(&ch) {
                matched += 1;
            }
            if let Some(slot) = fallback.get_mut(index) {
                *slot = matched;
            }
        }
        Ok(Self { fields, source, literal, case_sensitive, wanted, needle, fallback })
    }

    pub fn is_empty(&self) -> bool {
        if self.literal { self.needle.is_empty() } else { self.wanted.is_empty() }
    }

    pub fn word_count(&self) -> usize {
        self.wanted.len()
    }

    /// Bounded state for successive fragments of one source text unit. The
    /// caller removes extraction overlap and ends the unit explicitly, so long
    /// punctuation runs and words across segments cannot hide a phrase.
    pub fn stream(&self) -> TextStream<'_> {
        TextStream { term: self, prefix: String::new(), prefix_chars: 0, recent: VecDeque::with_capacity(self.wanted.len()), matched: false }
    }

    /// An indexed candidate restriction; a short value requires the engine's
    /// supported exact scan because FTS5 trigrams contain at least 3 characters.
    pub fn candidate(&self) -> (&'static str, String) {
        if self.literal {
            if self.case_sensitive { ("raw", self.source.clone()) } else { ("folded", fold_literal(&self.source)) }
        } else {
            (if self.case_sensitive { "raw" } else { "lexical" }, self.wanted.first().cloned().unwrap_or_default())
        }
    }

    pub fn matches(&self, text: &str) -> bool {
        self.matches_segment(text, true, true)
    }

    /// `starts_word` describes the original boundary of an overlapping native
    /// segment, so a fragment inside a long word cannot invent a new prefix.
    pub fn matches_segment(&self, text: &str, starts_word: bool, ends_word: bool) -> bool {
        if self.literal {
            if self.case_sensitive {
                self.literal_in(text.chars(), starts_word, ends_word)
            } else {
                self.literal_in(text.chars().default_case_fold(), starts_word, ends_word)
            }
        } else if self.wanted.is_empty() {
            true
        } else {
            let prefix = self.wanted.iter().map(|word| word.chars().count()).max().unwrap_or(0);
            let mut recent = VecDeque::with_capacity(self.wanted.len());
            each_prefix(text, self.case_sensitive, prefix, starts_word, |word| {
                if recent.len() == self.wanted.len() {
                    recent.pop_front();
                }
                recent.push_back(word.to_owned());
                recent.len() == self.wanted.len() && recent.iter().zip(&self.wanted).all(|(actual, wanted)| actual.starts_with(wanted))
            })
        }
    }

    fn literal_in(&self, text: impl Iterator<Item = char>, starts_word: bool, ends_word: bool) -> bool {
        if self.needle.is_empty() {
            return true;
        }
        let mut matched = 0;
        let mut previous = VecDeque::with_capacity(self.needle.len().saturating_add(1));
        let first_word = self.needle.first().is_some_and(|ch| word_edge(*ch));
        let last_word = self.needle.last().is_some_and(|ch| word_edge(*ch));
        let mut pending = false;
        for ch in text {
            if pending && (!last_word || !word_edge(ch)) {
                return true;
            }
            pending = false;
            if previous.len() > self.needle.len() {
                previous.pop_front();
            }
            previous.push_back(ch);
            while matched > 0 && self.needle.get(matched) != Some(&ch) {
                matched = self.fallback.get(matched - 1).copied().unwrap_or(0);
            }
            if self.needle.get(matched) == Some(&ch) {
                matched += 1;
                if matched == self.needle.len() {
                    let before = if previous.len() > self.needle.len() { previous.front().is_none_or(|ch| !word_edge(*ch)) } else { starts_word };
                    if !first_word || before {
                        if !last_word {
                            return true;
                        }
                        pending = true;
                    }
                    matched = self.fallback.last().copied().unwrap_or(0);
                }
            }
        }
        pending && ends_word
    }
}

/// Streaming ordinary-word verification. Literal matching uses the bounded
/// overlapping source segments and never normalizes their whitespace.
pub struct TextStream<'a> {
    term: &'a TextTerm,
    prefix: String,
    prefix_chars: usize,
    recent: VecDeque<String>,
    matched: bool,
}

impl TextStream<'_> {
    pub fn push(&mut self, text: &str) -> bool {
        let max = self.term.wanted.iter().map(|word| word.chars().count()).max().unwrap_or(0);
        for original in text.chars() {
            if words::stands_alone(original) {
                self.end_word();
                self.prefix.push(original);
                self.prefix_chars = 1;
                self.end_word();
            } else if original.is_alphanumeric() || words::is_mark(original) {
                if self.term.case_sensitive {
                    if self.prefix_chars < max {
                        self.prefix.push(original);
                        self.prefix_chars += 1;
                    }
                } else {
                    for folded in std::iter::once(original).default_case_fold().filter(|ch| !words::is_mark(*ch)) {
                        if let Some(plain) = words::plain(folded) {
                            for ch in plain.chars().take(max.saturating_sub(self.prefix_chars)) {
                                self.prefix.push(ch);
                                self.prefix_chars += 1;
                            }
                        } else if self.prefix_chars < max {
                            self.prefix.push(folded);
                            self.prefix_chars += 1;
                        }
                    }
                }
            } else {
                self.end_word();
            }
            if self.matched {
                return true;
            }
        }
        self.matched
    }

    pub fn finish(mut self) -> bool {
        self.end_word();
        self.matched
    }

    fn end_word(&mut self) {
        if self.prefix.is_empty() {
            return;
        }
        if self.recent.len() == self.term.wanted.len() {
            self.recent.pop_front();
        }
        self.recent.push_back(std::mem::take(&mut self.prefix));
        self.prefix_chars = 0;
        self.matched |= self.recent.len() == self.term.wanted.len()
            && self.recent.iter().zip(&self.term.wanted).all(|(actual, wanted)| actual.starts_with(wanted));
    }
}

pub fn word_edge(ch: char) -> bool {
    !words::stands_alone(ch) && (ch.is_alphanumeric() || words::is_mark(ch))
}

/// Read only as many characters of each complete source word as the query can
/// distinguish. Remaining characters still consume input and never start a word.
fn each_prefix(text: &str, sensitive: bool, max: usize, starts_word: bool, mut accept: impl FnMut(&str) -> bool) -> bool {
    let mut prefix = String::new();
    let mut count = 0;
    let mut skip = !starts_word;
    for original in text.chars() {
        if words::stands_alone(original) {
            if !skip && !prefix.is_empty() && accept(&prefix) {
                return true;
            }
            prefix.clear();
            prefix.push(original);
            if accept(&prefix) {
                return true;
            }
            prefix.clear();
            count = 0;
            skip = false;
        } else if original.is_alphanumeric() || words::is_mark(original) {
            if count < max && !skip {
                if sensitive {
                    prefix.push(original);
                    count += 1;
                } else {
                    for folded in std::iter::once(original).default_case_fold().filter(|ch| !words::is_mark(*ch)) {
                        if let Some(plain) = words::plain(folded) {
                            for ch in plain.chars().take(max.saturating_sub(count)) {
                                prefix.push(ch);
                                count += 1;
                            }
                        } else if count < max {
                            prefix.push(folded);
                            count += 1;
                        }
                    }
                }
            }
        } else {
            if !skip && !prefix.is_empty() && accept(&prefix) {
                return true;
            }
            prefix.clear();
            count = 0;
            skip = false;
        }
    }
    !skip && !prefix.is_empty() && accept(&prefix)
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
