//! Expansion keeps literal bytes sticky and offers splitting only to unquoted parameter values.
use super::syntax::{identifier, utf16_len, Ast, PartKind, Word};
use super::{Diagnostic, Fail, Fiber};
use crate::limits::{I_ARRAY_ELEMENTS, I_STRING_LENGTH};
use crate::mem::{KBytes, KVec};
use crate::util::{bytes_in, parse_u64};
use core::fmt::Write;

pub(super) struct Expander<'a> {
    pub fiber: &'a Fiber,
    pub ast: &'a Ast,
    pub source: &'a [u8],
}

#[derive(Clone, Copy)]
enum Value {
    Text(&'static [u8]),
    LastArg,
    Env(usize),
    Arg(usize),
    Number(i64),
    Positional(bool),
}

impl<'a> Expander<'a> {
    fn value(&self, name: &[u8]) -> Result<Value, Fail> {
        let f = self.fiber;
        let info = f.shell.process_info;
        let value = match name {
            b"?" => Value::Number(f.last_exit as i64),
            b"#" => Value::Number(f.params.len() as i64),
            b"$" | b"BASHPID" => Value::Number(info.pid as i64),
            b"PPID" => Value::Number(info.ppid as i64),
            b"UID" | b"EUID" => Value::Number(info.uid as i64),
            b"!" => Value::Text(b"0"),
            b"-" => Value::Text(b"hBs"),
            b"_" => Value::LastArg,
            b"BASH_VERSION" => Value::Text(b"5.1.0(1)-release"),
            b"@" => Value::Positional(true),
            b"*" => Value::Positional(false),
            b"" => Value::Text(b""),
            _ if identifier(name) => f
                .env
                .position(name)
                .map_or(Value::Text(b""), Value::Env),
            _ => {
                if !name[0].is_ascii_alphanumeric() && !b"_?$!#@*-".contains(&name[0]) {
                    return Err(Fail::BadSubstitution(bytes_in(name, &f.live)?));
                }
                match parse_u64(name) {
                    Some(0) => Value::Text(b"bash"),
                    Some(n) if n <= f.params.len() as u64 => Value::Arg(n as usize - 1),
                    _ => Value::Text(b""),
                }
            }
        };
        Ok(value)
    }

    fn ifs(&self) -> &'a [u8] {
        self.fiber
            .env
            .get(b"IFS")
            .map_or(b" \t\n".as_slice(), |e| &e.value)
    }

    fn joiner(&self, at: bool) -> &'a [u8] {
        if at {
            return b" ";
        }
        let ifs = self.ifs();
        let size = ifs[..ifs.len().min(4)].utf8_chunks().next().map_or(0, |p| {
            p.valid()
                .chars()
                .next()
                .map_or_else(|| usize::from(!p.invalid().is_empty()), char::len_utf8)
        });
        &ifs[..size]
    }

    pub fn splitting(&self, word: Word) -> (bool, bool) {
        let parts = &self.ast.parts[word.parts.range()];
        let unquoted = parts.iter().any(|p| p.kind == PartKind::Param);
        let positional = parts.len() == 1
            && matches!(parts[0].kind, PartKind::Param | PartKind::QuotedParam)
            && matches!(parts[0].text.get(self.source), b"@" | b"*");
        (!self.ifs().is_empty() && unquoted, positional)
    }
}

fn number(n: i64) -> Diagnostic {
    let mut text = Diagnostic::new();
    let _ = write!(text, "{n}");
    text
}

// Each cursor points into a stable word, positional argument or environment value.
pub(crate) const BYTE_CHUNK: usize = 256;

fn chunk_end(bytes: &[u8]) -> usize {
    if bytes.len() <= BYTE_CHUNK {
        return bytes.len();
    }
    // Keep UTF-8 scalars together. Invalid bytes remain literal.
    let mut end = BYTE_CHUNK;
    while end > BYTE_CHUNK - 4 && bytes[end] & 0xc0 == 0x80 {
        end -= 1;
    }
    if end == BYTE_CHUNK - 4 {
        BYTE_CHUNK
    } else {
        end
    }
}

struct Piece<'a> {
    bytes: &'a [u8],
    quoted: bool,
    split: bool,
    separate: bool,
}

#[derive(Default)]
struct Cursor {
    part: usize,
    param: usize,
    offset: usize,
    join: bool,
    prefix_done: bool,
    value: Option<Value>,
    lookup_left: Option<usize>,
}

impl Cursor {
    // The visitor returns how many bytes it consumed. It may stop at a delimiter.
    fn next(
        &mut self,
        ex: &Expander<'_>,
        word: Word,
        from: u32,
        prefix: &[u8],
        fields: bool,
        split: bool,
        mut visit: impl FnMut(Piece<'_>) -> Result<usize, Fail>,
    ) -> Result<bool, Fail> {
        let parts = &ex.ast.parts[word.parts.range()];
        loop {
            if !self.prefix_done {
                self.prefix_done = self.visit(prefix, false, false, false, &mut visit)?;
                return Ok(true);
            }
            let Some(part) = parts.get(self.part) else {
                return Ok(false);
            };
            if part.text.end < from || (part.text.end == from && part.kind == PartKind::Literal) {
                self.part += 1;
                continue;
            }
            let text = &ex.source[part.text.start.max(from) as usize..part.text.end as usize];
            let quoted = matches!(part.kind, PartKind::Quoted | PartKind::QuotedParam);
            let eligible = split && !quoted;
            let mut complete_part = true;
            let done = match part.kind {
                PartKind::Param | PartKind::QuotedParam => match match self.value {
                    Some(value) => value,
                    None => {
                        let left = self.lookup_left.get_or_insert_with(|| {
                            Fiber::name_work(text.len(), ex.fiber.env.len(), 1)
                        });
                        if !ex.fiber.pay_bytes(left) {
                            return Ok(true);
                        }
                        self.lookup_left = None;
                        *self.value.insert(ex.value(text)?)
                    }
                } {
                    Value::Text(bytes) => self.visit(bytes, quoted, eligible, false, &mut visit)?,
                    Value::LastArg => {
                        let last_arg = ex
                            .fiber
                            .last_arg
                            .as_deref()
                            .map_or(b"".as_slice(), KBytes::as_slice);
                        self.visit(last_arg, quoted, eligible, false, &mut visit)?
                    }
                    Value::Env(i) => self.visit(
                        &ex.fiber.env.entry(i).expect("environment is stable during expansion").1.value,
                        quoted,
                        eligible,
                        false,
                        &mut visit,
                    )?,
                    Value::Arg(i) => {
                        self.visit(&ex.fiber.params[i], quoted, eligible, false, &mut visit)?
                    }
                    Value::Number(n) => {
                        self.visit(number(n).bytes(), quoted, eligible, false, &mut visit)?
                    }
                    Value::Positional(at) => {
                        let separate = fields && (at || eligible);
                        let Some(arg) = ex.fiber.params.get(self.param) else {
                            let keep_empty = fields && !separate && self.param == 0 && quoted;
                            self.part += 1;
                            self.value = None;
                            self.param = 0;
                            self.join = false;
                            if keep_empty {
                                self.visit(b"", true, false, false, &mut visit)?;
                                return Ok(true);
                            }
                            continue;
                        };
                        complete_part = false;
                        if self.join && !separate {
                            if self.visit(ex.joiner(at), false, false, false, &mut visit)? {
                                self.join = false;
                            }
                        } else if self.visit(
                            arg,
                            quoted,
                            eligible,
                            separate && self.param > 0,
                            &mut visit,
                        )? {
                            self.param += 1;
                            self.join = true;
                        }
                        true
                    }
                },
                _ => self.visit(text, quoted, false, false, &mut visit)?,
            };
            if complete_part && done {
                self.part += 1;
                self.value = None;
            }
            return Ok(true);
        }
    }

    fn visit(
        &mut self,
        bytes: &[u8],
        quoted: bool,
        split: bool,
        separate: bool,
        visit: &mut impl FnMut(Piece<'_>) -> Result<usize, Fail>,
    ) -> Result<bool, Fail> {
        let rest = &bytes[self.offset..];
        self.offset += visit(Piece {
            bytes: rest,
            quoted,
            split,
            separate: separate && self.offset == 0,
        })?;
        let done = self.offset == bytes.len();
        if done {
            self.offset = 0;
        }
        Ok(done)
    }
}

pub(super) struct Scalar {
    cursor: Cursor,
    phase: ScalarPhase,
    bytes: usize,
    units: u64,
    out: KBytes,
}

#[derive(PartialEq, Eq)]
enum ScalarPhase {
    Size,
    Check,
    Copy,
}

impl Scalar {
    pub fn new(ex: &Expander<'_>) -> Self {
        Self {
            cursor: Cursor::default(),
            phase: ScalarPhase::Size,
            bytes: 0,
            units: 0,
            out: KBytes::new_in(&ex.fiber.live),
        }
    }

    pub fn run(
        &mut self,
        ex: &Expander<'_>,
        word: Word,
        from: u32,
        prefix: &[u8],
    ) -> Result<Option<KBytes>, Fail> {
        let cap = ex.fiber.shell.lim.vals[I_STRING_LENGTH];
        while ex.fiber.tick() {
            if self
                .cursor
                .next(ex, word, from, prefix, false, false, |piece| {
                    if self.phase == ScalarPhase::Size {
                        self.bytes = self
                            .bytes
                            .checked_add(piece.bytes.len())
                            .ok_or(Fail::StringLength)?;
                        return Ok(piece.bytes.len());
                    }
                    let chunk = &piece.bytes[..chunk_end(piece.bytes)];
                    if self.phase == ScalarPhase::Check {
                        self.units += utf16_len(chunk);
                        if self.units > cap {
                            return Err(Fail::StringLength);
                        }
                    } else {
                        self.out.try_extend_from_slice(chunk)?;
                    }
                    Ok(chunk.len())
                })?
            {
                continue;
            }
            if self.phase == ScalarPhase::Copy {
                return Ok(Some(core::mem::replace(
                    &mut self.out,
                    KBytes::new_in(&ex.fiber.live),
                )));
            }
            self.cursor = Cursor::default();
            if self.phase == ScalarPhase::Size && self.bytes as u64 > cap {
                self.phase = ScalarPhase::Check;
            } else {
                self.phase = ScalarPhase::Copy;
                self.out.try_reserve_exact(self.bytes)?;
            }
        }
        Ok(None)
    }
}

pub(super) struct Delimiters {
    ascii: [bool; 128],
    small_unicode: [char; 8],
    small_len: usize,
    unicode: Option<KBytes>,
    offset: usize,
}

impl Delimiters {
    pub fn new() -> Self {
        Self {
            ascii: [false; 128],
            small_unicode: ['\0'; 8],
            small_len: 0,
            unicode: None,
            offset: 0,
        }
    }

    fn prepare(&mut self, ex: &Expander<'_>) -> Result<bool, Fail> {
        let ifs = ex.ifs();
        while self.offset < ifs.len() {
            if !ex.fiber.tick() {
                return Ok(false);
            }
            let rest = &ifs[self.offset..];
            let chunk = &rest[..chunk_end(rest)];
            for c in chunk.utf8_chunks().flat_map(|p| p.valid().chars()) {
                let cp = c as usize;
                if c.is_ascii() {
                    self.ascii[cp] = true;
                } else {
                    if self.unicode.is_none() {
                        if self.small_unicode[..self.small_len].contains(&c) {
                            continue;
                        }
                        if let Some(slot) = self.small_unicode.get_mut(self.small_len) {
                            *slot = c;
                            self.small_len += 1;
                            continue;
                        }
                    }
                    let bits = match &mut self.unicode {
                        Some(bits) => bits,
                        slot @ None => {
                            // One bit per code point. Allocate only when IFS contains non-ASCII.
                            let mut bits = KBytes::with_capacity_in(0x110000 / 8, &ex.fiber.live)?;
                            bits.try_resize(0x110000 / 8, 0)?;
                            for &scalar in &self.small_unicode {
                                let cp = scalar as usize;
                                bits[cp / 8] |= 1 << (cp % 8);
                            }
                            slot.insert(bits)
                        }
                    };
                    bits[cp / 8] |= 1 << (cp % 8);
                }
            }
            self.offset += chunk.len();
        }
        Ok(true)
    }

    fn contains(&self, c: char) -> bool {
        let cp = c as usize;
        if c.is_ascii() {
            self.ascii[cp]
        } else if let Some(bits) = &self.unicode {
            bits[cp / 8] & (1 << (cp % 8)) != 0
        } else {
            self.small_unicode[..self.small_len].contains(&c)
        }
    }
}

pub(super) enum Argument {
    Scalar(Scalar),
    Fields(Fields),
}

impl Argument {
    pub fn new(ex: &Expander<'_>, word: Word, split: bool) -> Self {
        let separate = ex.ast.parts[word.parts.range()].iter().any(|p| {
            matches!(p.kind, PartKind::Param | PartKind::QuotedParam)
                && (p.text.get(ex.source) == b"@"
                    || (split && p.kind == PartKind::Param && p.text.get(ex.source) == b"*"))
        });
        if split || separate {
            Self::Fields(Fields {
                cursor: Cursor::default(),
                value: KBytes::new_in(&ex.fiber.live),
                present: false,
                pending: false,
                non_ws: false,
                start: None,
                split,
                units: 0,
            })
        } else {
            Self::Scalar(Scalar::new(ex))
        }
    }

    pub fn run(
        &mut self,
        ex: &Expander<'_>,
        word: Word,
        delims: &mut Delimiters,
        args: &mut KVec<KBytes>,
    ) -> Result<bool, Fail> {
        let cap = ex.fiber.shell.lim.vals[I_ARRAY_ELEMENTS];
        match self {
            Self::Scalar(scalar) => {
                let Some(value) = scalar.run(ex, word, word.text.start, b"")? else {
                    return Ok(false);
                };
                if !value.is_empty()
                    || ex.ast.parts[word.parts.range()]
                        .iter()
                        .any(|p| matches!(p.kind, PartKind::Quoted | PartKind::QuotedParam))
                {
                    if args.len() as u64 >= cap {
                        return Err(Fail::Arguments);
                    }
                    args.try_push(value)?;
                }
                Ok(true)
            }
            Self::Fields(fields) => {
                if fields.split && !delims.prepare(ex)? {
                    return Ok(false);
                }
                fields.start.get_or_insert(args.len());
                while ex.fiber.tick() {
                    let mut cursor = core::mem::take(&mut fields.cursor);
                    let more = cursor.next(
                        ex,
                        word,
                        word.text.start,
                        b"",
                        true,
                        fields.split,
                        |piece| fields.add(ex, args, delims, piece),
                    )?;
                    fields.cursor = cursor;
                    if !more {
                        if fields.present {
                            fields.flush(ex, args)?;
                        }
                        if args.len() as u64 > cap {
                            return Err(Fail::Arguments);
                        }
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

pub(super) struct Fields {
    cursor: Cursor,
    value: KBytes,
    present: bool,
    pending: bool,
    non_ws: bool,
    start: Option<usize>,
    split: bool,
    units: u64,
}

impl Fields {
    fn flush(&mut self, ex: &Expander<'_>, args: &mut KVec<KBytes>) -> Result<(), Fail> {
        if (args.len() - self.start.unwrap_or(args.len())) as u64
            >= ex.fiber.shell.lim.vals[I_ARRAY_ELEMENTS]
        {
            return Err(Fail::Elements);
        }
        if !self.split && self.units > ex.fiber.shell.lim.vals[I_STRING_LENGTH] {
            return Err(Fail::StringLength);
        }
        args.try_push(core::mem::replace(
            &mut self.value,
            KBytes::new_in(&ex.fiber.live),
        ))?;
        self.units = 0;
        self.present = false;
        self.pending = false;
        Ok(())
    }

    fn sticky(
        &mut self,
        ex: &Expander<'_>,
        args: &mut KVec<KBytes>,
        bytes: &[u8],
        quoted: bool,
    ) -> Result<(), Fail> {
        if bytes.is_empty() && !quoted {
            return Ok(());
        }
        if self.pending && self.present {
            self.flush(ex, args)?;
        }
        self.pending = false;
        self.non_ws = false;
        self.value.try_extend_from_slice(bytes)?;
        if !self.split {
            self.units += utf16_len(bytes);
        }
        self.present = true;
        Ok(())
    }

    fn add(
        &mut self,
        ex: &Expander<'_>,
        args: &mut KVec<KBytes>,
        delims: &Delimiters,
        piece: Piece<'_>,
    ) -> Result<usize, Fail> {
        if piece.separate {
            if self.present {
                self.flush(ex, args)?;
            }
            self.pending = false;
            self.non_ws = false;
        }
        let bytes = &piece.bytes[..chunk_end(piece.bytes)];
        if !piece.split {
            self.sticky(ex, args, bytes, piece.quoted)?;
            return Ok(bytes.len());
        }
        // Stop after each delimiter, so every flushed field consumes fuel.
        let mut offset = 0;
        for chunk in bytes.utf8_chunks() {
            if let Some((i, c)) = chunk
                .valid()
                .char_indices()
                .find(|&(_, c)| delims.contains(c))
            {
                self.sticky(ex, args, &bytes[..offset + i], false)?;
                if matches!(c, ' ' | '\t' | '\n') {
                    self.pending = self.present && !self.non_ws;
                } else {
                    self.flush(ex, args)?;
                    self.non_ws = true;
                }
                return Ok(offset + i + c.len_utf8());
            }
            offset += chunk.valid().len() + chunk.invalid().len();
        }
        self.sticky(ex, args, bytes, false)?;
        Ok(bytes.len())
    }
}
