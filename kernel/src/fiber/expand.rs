//! Expansion over L3 flat-arena `WordId` / `WordPart`, keeping literal bytes sticky,
//! fuel-sliced, and offering IFS splitting only to unquoted parameter/arithmetic values.
use super::syntax::{
    identifier, utf16_len, ArithBinOp, ArithExpr, ArithId, ArithUnaryOp, Ast, ParamOp, Span,
    WordId, WordPart,
};
use super::{Diagnostic, Fail, Fiber};
use crate::limits::{I_ARRAY_ELEMENTS, I_STRING_LENGTH};
use crate::mem::{KBytes, KVec};
use crate::util::{bytes_in, parse_u64};
use core::fmt::Write;

pub(super) struct Expander<'a> {
    pub fiber: &'a Fiber,
    pub ast: &'a Ast,
}

#[derive(Clone, Copy)]
enum Value {
    Text(&'static [u8]),
    LastArg,
    Env(usize),
    Arg(usize),
    Number(i64),
    Positional(bool),
    Temp,
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

    fn is_set_and_nonempty(&self, name: &[u8], check_empty: bool) -> bool {
        let f = self.fiber;
        match name {
            b"?" | b"#" | b"$" | b"BASHPID" | b"PPID" | b"UID" | b"EUID" | b"!" | b"-"
            | b"BASH_VERSION" => true,
            b"_" => f.last_arg.as_deref().is_some_and(|v| !check_empty || !v.is_empty()),
            b"@" | b"*" => !f.params.is_empty(),
            b"" => false,
            _ if identifier(name) => f
                .env
                .get(name)
                .is_some_and(|e| !check_empty || !e.value.is_empty()),
            _ => match parse_u64(name) {
                Some(0) => true,
                Some(n) if n <= f.params.len() as u64 => {
                    !check_empty || !f.params[n as usize - 1].is_empty()
                }
                _ => false,
            },
        }
    }

    fn value_len(&self, name: &[u8]) -> Result<i64, Fail> {
        let f = self.fiber;
        if matches!(name, b"@" | b"*") {
            return Ok(f.params.len() as i64);
        }
        let val = self.value(name)?;
        let len = match val {
            Value::Text(b) => utf16_len(b),
            Value::LastArg => f.last_arg.as_deref().map_or(0, |v| utf16_len(v)),
            Value::Env(i) => utf16_len(&f.env.entry(i).expect("env stable").1.value),
            Value::Arg(i) => utf16_len(&f.params[i]),
            Value::Number(n) => number(n).bytes().len() as u64,
            Value::Positional(_) => f.params.len() as u64,
            Value::Temp => 0,
        };
        Ok(len as i64)
    }

    fn eval_arith(&self, id: ArithId) -> i64 {
        self.eval_arith_bounded(id, 0)
    }

    fn eval_arith_bounded(&self, id: ArithId, depth: u32) -> i64 {
        if depth > 256 {
            return 0;
        }
        match self.ast.arith_nodes[id.0 as usize] {
            ArithExpr::Number(n) => n,
            ArithExpr::NumberOverflow => 0,
            ArithExpr::SingleQuote { value, .. } => value,
            ArithExpr::Var { name, .. } => {
                let name = self.ast.bytes(name);
                let raw = self
                    .fiber
                    .env
                    .get(name)
                    .map_or(b"".as_slice(), |e| e.value.as_slice());
                parse_i64_trimmed(raw)
            }
            ArithExpr::SpecialVar { name } => match self.value(self.ast.bytes(name)) {
                Ok(Value::Number(n)) => n,
                Ok(Value::Arg(i)) => parse_i64_trimmed(&self.fiber.params[i]),
                Ok(Value::Text(b)) => parse_i64_trimmed(b),
                _ => 0,
            },
            ArithExpr::Group(inner) | ArithExpr::Nested(inner) => {
                self.eval_arith_bounded(inner, depth + 1)
            }
            ArithExpr::Unary { op, operand, .. } => {
                let v = self.eval_arith_bounded(operand, depth + 1);
                match op {
                    ArithUnaryOp::Pos => v,
                    ArithUnaryOp::Neg => v.wrapping_neg(),
                    ArithUnaryOp::LogNot => i64::from(v == 0),
                    ArithUnaryOp::BitNot => !v,
                    ArithUnaryOp::Inc => v.wrapping_add(1),
                    ArithUnaryOp::Dec => v.wrapping_sub(1),
                }
            }
            ArithExpr::Binary { op, left, right } => {
                let l = self.eval_arith_bounded(left, depth + 1);
                match op {
                    ArithBinOp::LogAnd => {
                        if l == 0 {
                            0
                        } else {
                            i64::from(self.eval_arith_bounded(right, depth + 1) != 0)
                        }
                    }
                    ArithBinOp::LogOr => {
                        if l != 0 {
                            1
                        } else {
                            i64::from(self.eval_arith_bounded(right, depth + 1) != 0)
                        }
                    }
                    _ => {
                        let r = self.eval_arith_bounded(right, depth + 1);
                        match op {
                            ArithBinOp::Add => l.wrapping_add(r),
                            ArithBinOp::Sub => l.wrapping_sub(r),
                            ArithBinOp::Mul => l.wrapping_mul(r),
                            ArithBinOp::Div => if r == 0 { 0 } else { l.wrapping_div(r) },
                            ArithBinOp::Mod => if r == 0 { 0 } else { l.wrapping_rem(r) },
                            ArithBinOp::Pow => {
                                if r < 0 { 0 } else { l.wrapping_pow(r as u32) }
                            }
                            ArithBinOp::Shl => l.wrapping_shl(r as u32),
                            ArithBinOp::Shr => l.wrapping_shr(r as u32),
                            ArithBinOp::BitAnd => l & r,
                            ArithBinOp::BitOr => l | r,
                            ArithBinOp::BitXor => l ^ r,
                            ArithBinOp::Lt => i64::from(l < r),
                            ArithBinOp::Le => i64::from(l <= r),
                            ArithBinOp::Gt => i64::from(l > r),
                            ArithBinOp::Ge => i64::from(l >= r),
                            ArithBinOp::Eq => i64::from(l == r),
                            ArithBinOp::Ne => i64::from(l != r),
                            ArithBinOp::Comma => r,
                            ArithBinOp::LogAnd | ArithBinOp::LogOr => unreachable!(),
                        }
                    }
                }
            }
            ArithExpr::Ternary {
                cond,
                consequent,
                alternate,
            } => {
                if self.eval_arith_bounded(cond, depth + 1) != 0 {
                    self.eval_arith_bounded(consequent, depth + 1)
                } else {
                    self.eval_arith_bounded(alternate, depth + 1)
                }
            }
            _ => 0,
        }
    }

    /// Eagerly expands a short default word (e.g. `${x:-default}`) into `KBytes`.
    fn expand_word_eager(&self, word: WordId, depth: u32) -> Result<KBytes, Fail> {
        let mut out = KBytes::new_in(&self.fiber.live);
        if depth > 16 {
            return Ok(out);
        }
        let cap = self.fiber.shell.lim.vals[I_STRING_LENGTH];
        let parts = &self.ast.parts[self.ast.words[word.0 as usize].parts.range()];
        for &part in parts {
            self.append_part_eager(part, &mut out, cap, depth)?;
        }
        Ok(out)
    }

    fn append_part_eager(
        &self,
        part: WordPart,
        out: &mut KBytes,
        cap: u64,
        depth: u32,
    ) -> Result<(), Fail> {
        let push_checked = |out: &mut KBytes, bytes: &[u8]| -> Result<(), Fail> {
            out.try_extend_from_slice(bytes)?;
            if utf16_len(out) > cap {
                return Err(Fail::StringLength);
            }
            Ok(())
        };
        match part {
            WordPart::Literal(s)
            | WordPart::SingleQuoted(s)
            | WordPart::AnsiCQuoted(s)
            | WordPart::Escaped(s)
            | WordPart::Glob { pattern: s } => push_checked(out, self.ast.bytes(s))?,
            WordPart::DoubleQuoted(inner) => {
                for &p in &self.ast.parts[inner.range()] {
                    self.append_part_eager(p, out, cap, depth + 1)?;
                }
            }
            WordPart::TildeExpand { user } => {
                push_checked(out, b"~")?;
                if let Some(u) = user {
                    push_checked(out, self.ast.bytes(u))?;
                }
            }
            WordPart::ArithExpand(id) => {
                let n = self.eval_arith(id);
                push_checked(out, number(n).bytes())?;
            }
            WordPart::ParamExpand(idx) => {
                let pe = self.ast.param_expands[idx as usize];
                let name = self.ast.bytes(pe.parameter);
                match pe.op {
                    Some(ParamOp::BadSubstitution { text }) => {
                        return Err(Fail::BadSubstitution(bytes_in(
                            self.ast.bytes(text),
                            &self.fiber.live,
                        )?));
                    }
                    Some(ParamOp::LengthSliceError) => {
                        return Err(Fail::BadSubstitution(bytes_in(name, &self.fiber.live)?));
                    }
                    Some(ParamOp::Length) => {
                        let n = self.value_len(name)?;
                        push_checked(out, number(n).bytes())?;
                    }
                    Some(ParamOp::DefaultValue { word, check_empty }) => {
                        if !self.is_set_and_nonempty(name, check_empty) {
                            let val = self.expand_word_eager(word, depth + 1)?;
                            push_checked(out, &val)?;
                        } else {
                            self.append_value_eager(self.value(name)?, out, cap)?;
                        }
                    }
                    _ => self.append_value_eager(self.value(name)?, out, cap)?,
                }
            }
            WordPart::BraceExpand(_)
            | WordPart::CommandSubst { .. }
            | WordPart::ProcessSubst { .. } => {}
        }
        Ok(())
    }

    fn append_value_eager(&self, val: Value, out: &mut KBytes, cap: u64) -> Result<(), Fail> {
        let slice = match val {
            Value::Text(b) => b,
            Value::LastArg => self
                .fiber
                .last_arg
                .as_deref()
                .map_or(b"".as_slice(), KBytes::as_slice),
            Value::Env(i) => &self.fiber.env.entry(i).expect("env stable").1.value,
            Value::Arg(i) => &self.fiber.params[i],
            Value::Number(n) => {
                let d = number(n);
                out.try_extend_from_slice(d.bytes())?;
                if utf16_len(out) > cap {
                    return Err(Fail::StringLength);
                }
                return Ok(());
            }
            Value::Positional(at) => {
                let joiner = self.joiner(at);
                for (i, p) in self.fiber.params.iter().enumerate() {
                    if i > 0 {
                        out.try_extend_from_slice(joiner)?;
                    }
                    out.try_extend_from_slice(p)?;
                    if utf16_len(out) > cap {
                        return Err(Fail::StringLength);
                    }
                }
                return Ok(());
            }
            Value::Temp => b"",
        };
        out.try_extend_from_slice(slice)?;
        if utf16_len(out) > cap {
            return Err(Fail::StringLength);
        }
        Ok(())
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

    pub fn splitting(&self, word: WordId) -> (bool, bool) {
        let parts = &self.ast.parts[self.ast.words[word.0 as usize].parts.range()];
        let unquoted = parts
            .iter()
            .any(|p| matches!(p, WordPart::ParamExpand(_) | WordPart::ArithExpand(_)));
        let positional = match parts {
            [WordPart::ParamExpand(idx)] => {
                let pe = self.ast.param_expands[*idx as usize];
                pe.op.is_none() && matches!(self.ast.bytes(pe.parameter), b"@" | b"*")
            }
            [WordPart::DoubleQuoted(inner)] => {
                let ip = &self.ast.parts[inner.range()];
                matches!(ip, [WordPart::ParamExpand(idx)] if {
                    let pe = self.ast.param_expands[*idx as usize];
                    pe.op.is_none() && matches!(self.ast.bytes(pe.parameter), b"@" | b"*")
                })
            }
            _ => false,
        };
        (!self.ifs().is_empty() && unquoted, positional)
    }

    pub fn has_quoted_part(&self, word: WordId) -> bool {
        let parts = &self.ast.parts[self.ast.words[word.0 as usize].parts.range()];
        parts.iter().any(|p| {
            matches!(
                p,
                WordPart::SingleQuoted(_)
                    | WordPart::AnsiCQuoted(_)
                    | WordPart::DoubleQuoted(_)
                    | WordPart::Escaped(_)
            )
        })
    }

    pub fn is_all_unquoted_param(&self, word: WordId) -> bool {
        let parts = &self.ast.parts[self.ast.words[word.0 as usize].parts.range()];
        !parts.is_empty() && parts.iter().all(|p| matches!(p, WordPart::ParamExpand(_)))
    }
}

fn parse_i64_trimmed(bytes: &[u8]) -> i64 {
    let mut s = bytes;
    while let Some(&b) = s.first() {
        if b.is_ascii_whitespace() {
            s = &s[1..];
        } else {
            break;
        }
    }
    while let Some(&b) = s.last() {
        if b.is_ascii_whitespace() {
            s = &s[..s.len() - 1];
        } else {
            break;
        }
    }
    if s.is_empty() {
        return 0;
    }
    let (neg, digits) = if let Some(r) = s.strip_prefix(b"-") {
        (true, r)
    } else if let Some(r) = s.strip_prefix(b"+") {
        (false, r)
    } else {
        (false, s)
    };
    let mut acc: i64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return 0;
        }
        acc = acc.wrapping_mul(10).wrapping_add((b - b'0') as i64);
    }
    if neg {
        acc.wrapping_neg()
    } else {
        acc
    }
}

fn number(n: i64) -> Diagnostic {
    let mut text = Diagnostic::new();
    let _ = write!(text, "{n}");
    text
}

pub(crate) const BYTE_CHUNK: usize = 256;

fn chunk_end(bytes: &[u8]) -> usize {
    if bytes.len() <= BYTE_CHUNK {
        return bytes.len();
    }
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
    dq: Option<(Span, usize)>,
    param: usize,
    offset: usize,
    join: bool,
    prefix_done: bool,
    value: Option<Value>,
    temp: Option<KBytes>,
    lookup_left: Option<usize>,
}

impl Cursor {
    fn next(
        &mut self,
        ex: &Expander<'_>,
        word: WordId,
        prefix: &[u8],
        fields: bool,
        split: bool,
        mut visit: impl FnMut(Piece<'_>) -> Result<usize, Fail>,
    ) -> Result<bool, Fail> {
        let outer_parts = &ex.ast.parts[ex.ast.words[word.0 as usize].parts.range()];
        loop {
            if !self.prefix_done {
                self.prefix_done = self.visit(prefix, false, false, false, &mut visit)?;
                return Ok(true);
            }

            let (part, quoted) = if let Some((span, idx)) = self.dq {
                let inner = &ex.ast.parts[span.range()];
                if let Some(&p) = inner.get(idx) {
                    (p, true)
                } else {
                    self.dq = None;
                    self.part += 1;
                    continue;
                }
            } else if let Some(&p) = outer_parts.get(self.part) {
                (p, false)
            } else {
                return Ok(false);
            };

            let eligible = split && !quoted;
            let mut complete_part = true;

            let done = match part {
                WordPart::DoubleQuoted(inner) => {
                    if inner.is_empty() {
                        self.visit(b"", true, false, false, &mut visit)?
                    } else {
                        self.dq = Some((inner, 0));
                        continue;
                    }
                }
                WordPart::Literal(s) | WordPart::Glob { pattern: s } => {
                    self.visit(ex.ast.bytes(s), quoted, false, false, &mut visit)?
                }
                WordPart::SingleQuoted(s)
                | WordPart::AnsiCQuoted(s)
                | WordPart::Escaped(s) => {
                    self.visit(ex.ast.bytes(s), true, false, false, &mut visit)?
                }
                WordPart::TildeExpand { user } => {
                    if self.temp.is_none() {
                        let mut buf = KBytes::new_in(&ex.fiber.live);
                        buf.try_push(b'~')?;
                        if let Some(u) = user {
                            buf.try_extend_from_slice(ex.ast.bytes(u))?;
                        }
                        self.temp = Some(buf);
                    }
                    let temp = self.temp.take().unwrap();
                    let ok = self.visit(&temp, quoted, false, false, &mut visit)?;
                    if !ok {
                        self.temp = Some(temp);
                    }
                    ok
                }
                WordPart::ArithExpand(id) => {
                    let n = ex.eval_arith(id);
                    self.visit(number(n).bytes(), quoted, eligible, false, &mut visit)?
                }
                WordPart::ParamExpand(idx) => {
                    let pe = ex.ast.param_expands[idx as usize];
                    let name = ex.ast.bytes(pe.parameter);
                    let resolved = match self.value {
                        Some(v) => v,
                        None => {
                            let left = self.lookup_left.get_or_insert_with(|| {
                                Fiber::name_work(name.len(), ex.fiber.env.len(), 1)
                            });
                            if !ex.fiber.pay_bytes(left) {
                                return Ok(true);
                            }
                            self.lookup_left = None;
                            let val = match pe.op {
                                Some(ParamOp::BadSubstitution { text }) => {
                                    return Err(Fail::BadSubstitution(bytes_in(
                                        ex.ast.bytes(text),
                                        &ex.fiber.live,
                                    )?));
                                }
                                Some(ParamOp::LengthSliceError) => {
                                    return Err(Fail::BadSubstitution(bytes_in(
                                        name,
                                        &ex.fiber.live,
                                    )?));
                                }
                                Some(ParamOp::Length) => Value::Number(ex.value_len(name)?),
                                Some(ParamOp::DefaultValue { word: dw, check_empty }) => {
                                    if !ex.is_set_and_nonempty(name, check_empty) {
                                        self.temp = Some(ex.expand_word_eager(dw, 0)?);
                                        Value::Temp
                                    } else {
                                        ex.value(name)?
                                    }
                                }
                                _ => ex.value(name)?,
                            };
                            *self.value.insert(val)
                        }
                    };

                    match resolved {
                        Value::Text(bytes) => {
                            self.visit(bytes, quoted, eligible, false, &mut visit)?
                        }
                        Value::Temp => {
                            let temp = self.temp.take().unwrap();
                            let ok = self.visit(&temp, quoted, eligible, false, &mut visit)?;
                            if !ok {
                                self.temp = Some(temp);
                            }
                            ok
                        }
                        Value::LastArg => {
                            let last_arg = ex
                                .fiber
                                .last_arg
                                .as_deref()
                                .map_or(b"".as_slice(), KBytes::as_slice);
                            self.visit(last_arg, quoted, eligible, false, &mut visit)?
                        }
                        Value::Env(i) => self.visit(
                            &ex.fiber
                                .env
                                .entry(i)
                                .expect("environment is stable during expansion")
                                .1
                                .value,
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
                                self.advance_part();
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
                    }
                }
                WordPart::BraceExpand(_)
                | WordPart::CommandSubst { .. }
                | WordPart::ProcessSubst { .. } => true,
            };

            if complete_part && done {
                self.advance_part();
                self.value = None;
                self.temp = None;
            }
            return Ok(true);
        }
    }

    fn advance_part(&mut self) {
        if let Some((_, idx)) = &mut self.dq {
            *idx += 1;
        } else {
            self.part += 1;
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
        word: WordId,
        prefix: &[u8],
    ) -> Result<Option<KBytes>, Fail> {
        let cap = ex.fiber.shell.lim.vals[I_STRING_LENGTH];
        while ex.fiber.tick() {
            if self
                .cursor
                .next(ex, word, prefix, false, false, |piece| {
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

    pub fn prepare(&mut self, ex: &Expander<'_>) -> Result<bool, Fail> {
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
                            let mut bits =
                                KBytes::with_capacity_in(0x110000 / 8, &ex.fiber.live)?;
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

    pub fn contains(&self, c: char) -> bool {
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
    pub fn new(ex: &Expander<'_>, word: WordId, split: bool) -> Self {
        let parts = &ex.ast.parts[ex.ast.words[word.0 as usize].parts.range()];
        let is_param_at_or_star = |idx: u32, quoted: bool| -> bool {
            let pe = ex.ast.param_expands[idx as usize];
            if pe.op.is_some() {
                return false;
            }
            let name = ex.ast.bytes(pe.parameter);
            name == b"@" || (!quoted && split && name == b"*")
        };
        let separate = parts.iter().any(|p| match *p {
            WordPart::ParamExpand(idx) => is_param_at_or_star(idx, false),
            WordPart::DoubleQuoted(inner) => ex.ast.parts[inner.range()]
                .iter()
                .any(|ip| matches!(*ip, WordPart::ParamExpand(idx) if is_param_at_or_star(idx, true))),
            _ => false,
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
        word: WordId,
        delims: &mut Delimiters,
        args: &mut KVec<KBytes>,
    ) -> Result<bool, Fail> {
        let cap = ex.fiber.shell.lim.vals[I_ARRAY_ELEMENTS];
        match self {
            Self::Scalar(scalar) => {
                let Some(value) = scalar.run(ex, word, b"")? else {
                    return Ok(false);
                };
                if !value.is_empty() || ex.has_quoted_part(word) {
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
                    let more = cursor.next(ex, word, b"", true, fields.split, |piece| {
                        fields.add(ex, args, delims, piece)
                    })?;
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
    pub fn new(ex: &Expander<'_>, split: bool) -> Self {
        Self {
            cursor: Cursor::default(),
            value: KBytes::new_in(&ex.fiber.live),
            present: false,
            pending: false,
            non_ws: false,
            start: None,
            split,
            units: 0,
        }
    }

    /// Expands positional parameters (`$@`) with IFS splitting (matching `just-bash@3.6.0`
    /// bare `for p; do ...; done`).
    pub fn expand_positional(
        &mut self,
        ex: &Expander<'_>,
        delims: &mut Delimiters,
        items: &mut KVec<KBytes>,
    ) -> Result<bool, Fail> {
        let cap = ex.fiber.shell.lim.vals[I_ARRAY_ELEMENTS];
        let do_split = !ex.ifs().is_empty();
        self.split = do_split;
        if do_split && !delims.prepare(ex)? {
            return Ok(false);
        }
        self.start.get_or_insert(items.len());
        while ex.fiber.tick() {
            let Some(arg) = ex.fiber.params.get(self.cursor.param) else {
                if self.present {
                    self.flush(ex, items)?;
                }
                if items.len() as u64 > cap {
                    return Err(Fail::Elements);
                }
                return Ok(true);
            };
            let rest = &arg[self.cursor.offset..];
            let separate = self.cursor.param > 0 && self.cursor.offset == 0;
            let consumed = self.add(
                ex,
                items,
                delims,
                Piece {
                    bytes: rest,
                    quoted: false,
                    split: do_split,
                    separate,
                },
            )?;
            self.cursor.offset += consumed;
            if self.cursor.offset >= arg.len() {
                self.cursor.offset = 0;
                self.cursor.param += 1;
            }
        }
        Ok(false)
    }

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
