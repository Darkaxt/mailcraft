// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two-stage free-text search. Stage 1 turns a term into the vocabulary
//! tokens that contain its folded pieces (an FTS5 candidate expression);
//! stage 2 verifies a candidate unit's stored text with the active options
//! and scores the best occurrence. Mail text is untrusted data: patterns and
//! text are bounded, and nothing here allocates per byte beyond one unit.

use mailcraft_model::search::text::{fold_words, word_edge};
use mailcraft_model::words;

use crate::{ScoreField, SearchOptions};

/// Most vocabulary tokens one piece of a term expands to. Past it the term is
/// reported as too broad: tokens starting with the piece are still found by
/// an FTS5 prefix query, and the most frequent containing tokens up to this
/// cap are added to it.
pub const EXPANSION_CAP: usize = 2000;
/// A matched word or regex match is reported up to this many characters.
const MATCHED_CHARS: usize = 64;
/// Compiled regular-expression size allowances (untrusted patterns).
const REGEX_SIZE: usize = 1 << 20;
const REGEX_DFA_SIZE: usize = 4 << 20;

/// The best occurrence of one term in one text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub score: u8,
    pub matched: String,
    pub extra: u32,
}

/// One term's best hit in one document, with the part it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BestHit {
    pub score: u8,
    pub matched: String,
    pub extra: u32,
    pub field: ScoreField,
}

impl BestHit {
    /// Whether `self` is a better hit than `other`: a higher score, then a
    /// better-placed part (subject, people, body, attachment).
    pub fn better_than(&self, other: &BestHit) -> bool {
        (self.score, std::cmp::Reverse(self.field)) > (other.score, std::cmp::Reverse(other.field))
    }
}

/// The part a link's field bits stand for, in score order.
pub fn field_of(fields: u8) -> ScoreField {
    if fields & words::SUBJECT != 0 {
        ScoreField::Subject
    } else if fields & (words::FROM | words::TO | words::CC) != 0 {
        ScoreField::People
    } else if fields & (words::ATTACHMENT | words::ATTACHMENT_CONTENT) != 0 {
        ScoreField::Attachment
    } else {
        ScoreField::Body
    }
}

pub enum Matcher {
    Text { needle: Vec<char>, fallback: Vec<usize>, match_case: bool, whole_word: bool },
    Regex(regex::Regex),
}

/// `ch` as compared: accents dropped, and lower case unless `match_case`.
fn fold_char(ch: char, match_case: bool, out: &mut Vec<char>) {
    if ch.is_ascii() {
        out.push(if match_case { ch } else { ch.to_ascii_lowercase() });
        return;
    }
    let mut buffer = [0u8; 4];
    let folded = fold_words(ch.encode_utf8(&mut buffer));
    if match_case && ch.is_uppercase() {
        for folded in folded.chars() {
            out.extend(folded.to_uppercase());
        }
    } else {
        out.extend(folded.chars());
    }
}

/// The folded characters of `chars`, each with the index of the original
/// character it came from (a mark folds to nothing, `ß` to two).
fn fold_all(chars: &[char], match_case: bool) -> (Vec<char>, Vec<usize>) {
    let mut folded = Vec::with_capacity(chars.len());
    let mut origin = Vec::with_capacity(chars.len());
    let mut scratch = Vec::with_capacity(4);
    for (index, ch) in chars.iter().enumerate() {
        scratch.clear();
        fold_char(*ch, match_case, &mut scratch);
        for folded_char in &scratch {
            folded.push(*folded_char);
            origin.push(index);
        }
    }
    (folded, origin)
}

fn truncated(chars: &[char]) -> String {
    chars.iter().take(MATCHED_CHARS).collect()
}

/// 100 × `term` ÷ `word`, rounded half up, at most 100.
pub fn score(term: usize, word: usize) -> u8 {
    let word = word.max(term).max(1);
    let value = (term.saturating_mul(200).saturating_add(word)) / word.saturating_mul(2);
    u8::try_from(value.min(100)).unwrap_or(100)
}

impl Matcher {
    /// `source` is the term's text; `case_sensitive` the query's own
    /// `case:sensitive`, which matchCase also turns on.
    pub fn new(source: &str, case_sensitive: bool, options: &SearchOptions) -> Result<Self, String> {
        let match_case = options.match_case || case_sensitive;
        if options.regex {
            let regex = regex::RegexBuilder::new(source)
                .case_insensitive(!match_case)
                .size_limit(REGEX_SIZE)
                .dfa_size_limit(REGEX_DFA_SIZE)
                .build()
                .map_err(|why| format!("the regular expression is not valid: {why}"))?;
            return Ok(Matcher::Regex(regex));
        }
        let chars: Vec<char> = source.chars().collect();
        let (needle, _) = fold_all(&chars, match_case);
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
        Ok(Matcher::Text { needle, fallback, match_case, whole_word: options.whole_words() })
    }

    /// The best occurrence in `text`, a stored unit whose first and last
    /// characters are at a source word boundary when `starts_word`/`ends_word`.
    pub fn best(&self, text: &str, starts_word: bool, ends_word: bool) -> Option<Hit> {
        match self {
            Matcher::Regex(regex) => {
                regex.find(text).map(|found| Hit { score: 100, matched: found.as_str().chars().take(MATCHED_CHARS).collect(), extra: 0 })
            }
            Matcher::Text { needle, fallback, match_case, whole_word } => {
                if needle.is_empty() {
                    return None;
                }
                let chars: Vec<char> = text.chars().collect();
                let (folded, origin) = fold_all(&chars, *match_case);
                let length = needle.len();
                let mut best: Option<Hit> = None;
                let mut matched = 0usize;
                for (end, ch) in folded.iter().enumerate() {
                    while matched > 0 && needle.get(matched) != Some(ch) {
                        matched = fallback.get(matched - 1).copied().unwrap_or(0);
                    }
                    if needle.get(matched) != Some(ch) {
                        continue;
                    }
                    matched += 1;
                    if matched < length {
                        continue;
                    }
                    matched = fallback.last().copied().unwrap_or(0);
                    let start = end + 1 - length;
                    let (Some(&first), Some(&last)) = (origin.get(start), origin.get(end)) else { continue };
                    let after = last + 1;
                    let edge = |at: usize| chars.get(at).is_some_and(|ch| word_edge(*ch));
                    let hit = if *whole_word {
                        let before_ok = if first == 0 { starts_word || !edge(0) } else { !edge(first - 1) || !edge(first) };
                        let after_ok =
                            if after >= chars.len() { ends_word || !edge(chars.len().saturating_sub(1)) } else { !edge(after) || !edge(last) };
                        if !(before_ok && after_ok) {
                            continue;
                        }
                        Hit { score: 100, matched: truncated(chars.get(first..after).unwrap_or_default()), extra: 0 }
                    } else {
                        let mut word_start = first;
                        while word_start > 0 && edge(word_start - 1) {
                            word_start -= 1;
                        }
                        let mut word_end = after;
                        while word_end < chars.len() && edge(word_end) {
                            word_end += 1;
                        }
                        let word = origin.partition_point(|&o| o < word_end).saturating_sub(origin.partition_point(|&o| o < word_start));
                        let word = word.max(length);
                        Hit {
                            score: score(length, word),
                            matched: truncated(chars.get(word_start..word_end).unwrap_or_default()),
                            extra: u32::try_from(word - length).unwrap_or(u32::MAX),
                        }
                    };
                    if best.as_ref().is_none_or(|known| hit.score > known.score) {
                        let perfect = hit.score == 100;
                        best = Some(hit);
                        if perfect {
                            break;
                        }
                    }
                }
                best
            }
        }
    }
}

/// The folded word pieces stage 1 looks up in the vocabulary: every run of
/// letters and digits of the term, or for a regular expression the literal
/// runs it requires. Empty: no usable literal (a bounded scan answers it).
pub fn pieces(source: &str, regex: bool) -> Vec<String> {
    let runs = if regex { regex_literals(source) } else { vec![source.to_owned()] };
    let mut out = Vec::new();
    for run in runs {
        for piece in fold_words(&run).split(|ch: char| !ch.is_alphanumeric()) {
            if !piece.is_empty() && !out.iter().any(|known: &String| known == piece) {
                out.push(piece.to_owned());
            }
        }
    }
    out
}

/// Conservative required literal runs of a regular expression: letters and
/// digits outside classes, escapes and optional elements. Any alternation
/// (`|`) or optional group gives none, so a candidate restriction can never
/// exclude a real match.
pub fn regex_literals(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    if chars.contains(&'|') {
        return Vec::new();
    }
    // An optional group (`)?`, `)*`, `){0`) may hold the only literal: none.
    for (index, ch) in chars.iter().enumerate() {
        if *ch == ')' && matches!(chars.get(index + 1), Some('?' | '*' | '{')) {
            return Vec::new();
        }
    }
    let mut runs = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    let finish = |current: &mut String, runs: &mut Vec<String>| {
        if !current.is_empty() {
            runs.push(std::mem::take(current));
        }
    };
    while let Some(&ch) = chars.get(index) {
        match ch {
            '\\' => {
                finish(&mut current, &mut runs);
                // `\p{..}`/`\x{..}` and the like: skip the braces too.
                index += 2;
                if chars.get(index) == Some(&'{') {
                    while chars.get(index).is_some_and(|ch| *ch != '}') {
                        index += 1;
                    }
                    index += 1;
                }
                continue;
            }
            '[' => {
                finish(&mut current, &mut runs);
                index += 1;
                if chars.get(index) == Some(&']') {
                    index += 1;
                }
                while let Some(&inside) = chars.get(index) {
                    index += if inside == '\\' { 2 } else { 1 };
                    if inside == ']' {
                        break;
                    }
                }
                continue;
            }
            '(' => {
                finish(&mut current, &mut runs);
                index += 1;
                // Flags and group names (`(?i)`, `(?P<name>`) are not text.
                if chars.get(index) == Some(&'?') {
                    while chars.get(index).is_some_and(|ch| !matches!(ch, ':' | ')' | '>')) {
                        index += 1;
                    }
                    index += 1;
                }
                continue;
            }
            '?' | '*' | '{' => {
                // The character before is optional: it is not required text.
                current.pop();
                finish(&mut current, &mut runs);
                if ch == '{' {
                    while chars.get(index).is_some_and(|ch| *ch != '}') {
                        index += 1;
                    }
                }
            }
            ch if ch.is_alphanumeric() => current.push(ch),
            _ => finish(&mut current, &mut runs),
        }
        index += 1;
    }
    finish(&mut current, &mut runs);
    runs
}

/// One FTS5 MATCH group for a piece: its expansion tokens ORed, and when the
/// expansion was capped, a prefix query for the piece itself. `None` when
/// nothing in the vocabulary can contain the piece.
pub fn group(piece: &str, tokens: &[String], too_broad: bool) -> Option<String> {
    let quote = |text: &str| format!("\"{}\"", text.replace('"', "\"\""));
    let mut alternatives: Vec<String> = tokens.iter().map(|token| quote(token)).collect();
    if too_broad {
        alternatives.push(format!("{} *", quote(piece)));
    }
    if alternatives.is_empty() { None } else { Some(format!("({})", alternatives.join(" OR "))) }
}

#[cfg(test)]
#[path = "matcher_tests.rs"]
mod tests;
