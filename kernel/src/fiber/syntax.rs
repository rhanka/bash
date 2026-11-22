//! Full L3 bash lexer, multi-heredoc normalizer, and flat-arena parser.
use crate::mem::{Budget, KBytes, KString, KVec, MemError};
use core::fmt::{self, Write};

mod arith;
mod ast;
mod cond;
mod lexer;
mod parser;
mod word;

#[cfg(test)]
mod tests;

pub use ast::*;

pub(super) const TOKEN_CAP: usize = 100_000;
pub(super) const DEPTH_CAP: u32 = 200;
pub(super) const TOP_STMT_CAP: usize = 10_000;
pub(super) const STEP_CAP: usize = 1_000_000;

pub(super) fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub(super) fn ident_byte(b: u8) -> bool {
    ident_start(b) || b.is_ascii_digit()
}

pub(super) fn identifier(s: &[u8]) -> bool {
    s.first().is_some_and(|&b| ident_start(b)) && s.iter().all(|&b| ident_byte(b))
}

pub(super) fn utf16_len(bytes: &[u8]) -> u64 {
    bytes
        .utf8_chunks()
        .map(|c| c.valid().encode_utf16().count() as u64 + c.invalid().len() as u64)
        .sum()
}

pub(super) fn space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

fn trim_start(line: &[u8]) -> &[u8] {
    let valid = line.utf8_chunks().next().map_or("", |part| part.valid());
    &line[valid.len() - valid.trim_start_matches(space).len()..]
}

fn preceding_space(line: &[u8], at: usize) -> bool {
    let left = &line[at.saturating_sub(4)..at];
    let start = left.iter().rposition(|b| b & 0xc0 != 0x80);
    start
        .and_then(|i| core::str::from_utf8(&left[i..]).ok())
        .and_then(|s| s.chars().next())
        .is_some_and(space)
}

/// Remove leading whitespace only on lines that start outside a continued quote or queued heredoc.
/// Extends L2 to a FIFO queue so multiple heredocs on one line (`cat <<A <<B`) protect all bodies.
pub(super) fn normalize(source: &[u8], budget: &Budget) -> Result<KBytes, MemError> {
    let mut out = KBytes::with_capacity_in(source.len(), budget)?;
    let mut quote: u8 = 0;
    let mut heredocs: KVec<(Span, bool)> = KVec::new_in(budget);
    let mut head: usize = 0;
    let mut offset: usize = 0;

    for (line_no, line) in source.split(|&b| b == b'\n').enumerate() {
        let line_src_start = offset;
        offset += line.len() + 1;
        if line_no != 0 {
            out.try_push(b'\n')?;
        }
        if let Some(&(delim_span, strip_tabs)) = heredocs.get(head) {
            let mut closing = line;
            if strip_tabs {
                while let Some(rest) = closing.strip_prefix(b"\t") {
                    closing = rest;
                }
            }
            if closing == delim_span.get(source) {
                head += 1;
                out.try_extend_from_slice(trim_start(line))?;
            } else {
                out.try_extend_from_slice(line)?;
            }
            continue;
        }
        let outside = quote == 0;
        let trimmed = if outside { trim_start(line) } else { line };
        let trim_offset = line.len() - trimmed.len();
        out.try_extend_from_slice(trimmed)?;
        let mut i = 0;
        while i < trimmed.len() {
            let b = trimmed[i];
            match (quote, b) {
                (b'\'', b'\'') | (b'"', b'"') => quote = 0,
                (b'\'', _) => {}
                (_, b'\\') => i += 1,
                (0, b'\'' | b'"') => quote = b,
                (0, b'#') if i == 0 || preceding_space(trimmed, i) => break,
                (0, b'<') if outside && trimmed.get(i + 1) == Some(&b'<') => {
                    i += 2;
                    let tabs = trimmed.get(i) == Some(&b'-');
                    i += usize::from(tabs);
                    while trimmed.get(i).is_some_and(|b| matches!(b, b' ' | b'\t')) {
                        i += 1;
                    }
                    let q = trimmed.get(i).copied().filter(|b| matches!(b, b'\'' | b'"'));
                    i += usize::from(q.is_some());
                    let start = i;
                    while trimmed.get(i).is_some_and(|&b| ident_byte(b) || b == b'-') {
                        i += 1;
                    }
                    if i > start && q.is_none_or(|q| trimmed.get(i) == Some(&q)) {
                        let abs_start = line_src_start + trim_offset + start;
                        let abs_end = line_src_start + trim_offset + i;
                        heredocs.try_push((Span::new(abs_start, abs_end), tabs))?;
                        i += usize::from(q.is_some());
                    }
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
    }
    Ok(out)
}

#[derive(Debug)]
pub enum Error {
    Memory(MemError),
    Lexer {
        line: u32,
        message: KBytes,
    },
    Parser {
        line: u32,
        col: u32,
        message: KBytes,
    },
    Work,
}

impl From<MemError> for Error {
    fn from(error: MemError) -> Self {
        Self::Memory(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory(e) => write!(f, "{e:?}"),
            Self::Lexer { line, message } => {
                let msg = core::str::from_utf8(message).unwrap_or("lexer error");
                writeln!(f, "bash: line {line}: {msg}")
            }
            Self::Parser { line, col, message } => {
                let msg = core::str::from_utf8(message).unwrap_or("parse error");
                writeln!(f, "bash: syntax error: Parse error at {line}:{col}: {msg}")
            }
            Self::Work => write!(f, "work limit exceeded"),
        }
    }
}

pub(super) fn box_msg(budget: &Budget, args: fmt::Arguments<'_>) -> Result<KBytes, MemError> {
    let mut s = KString::new_in(budget);
    if s.write_fmt(args).is_err() {
        if let Some(e) = s.take_fmt_error() {
            return Err(e);
        }
    }
    Ok(s.into_bytes())
}

pub(super) fn lexer_err(line: u32, budget: &Budget, args: fmt::Arguments<'_>) -> Error {
    match box_msg(budget, args) {
        Ok(message) => Error::Lexer { line, message },
        Err(e) => Error::Memory(e),
    }
}

pub(super) fn parser_err(pos: Pos, budget: &Budget, args: fmt::Arguments<'_>) -> Error {
    match box_msg(budget, args) {
        Ok(message) => Error::Parser {
            line: pos.line,
            col: pos.col,
            message,
        },
        Err(e) => Error::Memory(e),
    }
}

impl Ast {
    /// Lexes and parses a normalized (or raw) script into a flat-arena `Ast`.
    pub fn parse(
        source: &[u8],
        max_heredoc_size: u64,
        budget: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Self, Error> {
        parser::parse_script(source, max_heredoc_size, budget, work)
    }
}
