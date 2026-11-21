//! Expansion keeps literal bytes sticky and offers splitting only to unquoted parameter values.
use super::syntax::{identifier, utf16_len, Ast, PartKind, Word};
use super::{Diagnostic, Fail, Fiber};
use crate::limits::{I_ARRAY_ELEMENTS, I_STRING_LENGTH};
use crate::mem::{Budget, KBytes, KVec};
use crate::util::{bytes_in, parse_u64};
use core::fmt::Write;

pub(super) struct Expander<'a> {
    pub fiber: &'a Fiber,
    pub ast: &'a Ast,
    pub source: &'a [u8],
}

enum Value<'a> {
    Text(&'a [u8]),
    Number(i64),
    Positional(bool),
}

impl<'a> Expander<'a> {
    fn value(&self, name: &'a [u8]) -> Result<Value<'a>, Fail> {
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
            b"_" => Value::Text(&f.last_arg),
            b"BASH_VERSION" => Value::Text(b"5.1.0(1)-release"),
            b"@" => Value::Positional(true),
            b"*" => Value::Positional(false),
            b"" => Value::Text(b""),
            _ if identifier(name) => {
                Value::Text(f.env.get(name).map_or(b"".as_slice(), |e| &e.value))
            }
            _ => {
                if !name[0].is_ascii_alphanumeric() && !b"_?$!#@*-".contains(&name[0]) {
                    return Err(Fail::BadSubstitution(bytes_in(name, &f.live)?));
                }
                match parse_u64(name) {
                    Some(0) => Value::Text(b"bash"),
                    Some(n) if n <= f.params.len() as u64 => Value::Text(&f.params[n as usize - 1]),
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
        let size = ifs.utf8_chunks().next().map_or(0, |p| {
            p.valid()
                .chars()
                .next()
                .map_or_else(|| usize::from(!p.invalid().is_empty()), char::len_utf8)
        });
        &ifs[..size]
    }

    fn visit_scalar(
        &self,
        word: Word,
        from: u32,
        mut visit: impl FnMut(&[u8]) -> Result<(), Fail>,
    ) -> Result<(), Fail> {
        for part in &self.ast.parts[word.parts.range()] {
            if part.text.end < from || (part.text.end == from && part.kind == PartKind::Literal) {
                continue;
            }
            let start = part.text.start.max(from) as usize;
            let text = &self.source[start..part.text.end as usize];
            if !matches!(part.kind, PartKind::Param | PartKind::QuotedParam) {
                visit(text)?;
                continue;
            }
            match self.value(text)? {
                Value::Text(bytes) => visit(bytes)?,
                Value::Number(n) => visit(&number(n).bytes())?,
                Value::Positional(at) => {
                    for (i, arg) in self.fiber.params.iter().enumerate() {
                        if i != 0 {
                            visit(self.joiner(at))?;
                        }
                        visit(arg)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn scalar(&self, word: Word, from: u32, prefix: &[u8]) -> Result<KBytes, Fail> {
        let cap = self.fiber.shell.lim.vals[I_STRING_LENGTH];
        let mut bytes = prefix.len();
        self.visit_scalar(word, from, |part| {
            bytes = bytes.checked_add(part.len()).ok_or(Fail::StringLength)?;
            Ok(())
        })?;
        if bytes as u64 > cap {
            let mut units = utf16_len(prefix);
            self.visit_scalar(word, from, |part| {
                units += utf16_len(part);
                if units > cap {
                    Err(Fail::StringLength)
                } else {
                    Ok(())
                }
            })?;
            if units > cap {
                return Err(Fail::StringLength);
            }
        }
        let mut out = KBytes::with_capacity_in(bytes, &self.fiber.live)?;
        out.try_extend_from_slice(prefix)?;
        self.visit_scalar(word, from, |part| {
            out.try_extend_from_slice(part).map_err(Fail::Mem)
        })?;
        Ok(out)
    }

    pub fn splitting(&self, word: Word) -> (bool, bool) {
        let parts = &self.ast.parts[word.parts.range()];
        let unquoted = parts.iter().any(|p| p.kind == PartKind::Param);
        let positional = parts.len() == 1
            && matches!(parts[0].kind, PartKind::Param | PartKind::QuotedParam)
            && matches!(parts[0].text.get(self.source), b"@" | b"*");
        (!self.ifs().is_empty() && unquoted, positional)
    }

    pub fn args(&self, word: Word, args: &mut KVec<KBytes>) -> Result<(), Fail> {
        let parts = &self.ast.parts[word.parts.range()];
        let (split, _) = self.splitting(word);
        let separate = parts.iter().any(|p| {
            matches!(p.kind, PartKind::Param | PartKind::QuotedParam)
                && p.text.get(self.source) == b"@"
        }) || (split
            && parts
                .iter()
                .any(|p| p.kind == PartKind::Param && p.text.get(self.source) == b"*"));
        let cap = self.fiber.shell.lim.vals[I_ARRAY_ELEMENTS];
        if !split && !separate {
            let value = self.scalar(word, word.text.start, b"")?;
            if !value.is_empty()
                || parts
                    .iter()
                    .any(|p| matches!(p.kind, PartKind::Quoted | PartKind::QuotedParam))
            {
                if args.len() as u64 >= cap {
                    return Err(Fail::Arguments);
                }
                args.try_push(value)?;
            }
            return Ok(());
        }

        let budget = &self.fiber.live;
        let mut fields = Fields {
            budget,
            value: KBytes::new_in(budget),
            out: KVec::new_in(budget),
            present: false,
            pending: false,
            non_ws: false,
            cap,
            string_cap: self.fiber.shell.lim.vals[I_STRING_LENGTH],
            split,
        };
        for part in parts {
            let text = part.text.get(self.source);
            let quoted = matches!(part.kind, PartKind::Quoted | PartKind::QuotedParam);
            if !matches!(part.kind, PartKind::Param | PartKind::QuotedParam) {
                fields.sticky(text, quoted)?;
                continue;
            }
            let eligible = split && !quoted;
            match self.value(text)? {
                Value::Text(bytes) => fields.add(bytes, eligible, quoted, self.ifs())?,
                Value::Number(n) => fields.add(number(n).bytes(), eligible, quoted, self.ifs())?,
                Value::Positional(at) if at || eligible => {
                    for (i, arg) in self.fiber.params.iter().enumerate() {
                        if i > 0 && fields.present {
                            fields.flush()?;
                        }
                        fields.pending = false;
                        fields.non_ws = false;
                        fields.add(arg, eligible, quoted, self.ifs())?;
                    }
                }
                Value::Positional(at) => {
                    for (i, arg) in self.fiber.params.iter().enumerate() {
                        if i > 0 {
                            fields.sticky(self.joiner(at), false)?;
                        }
                        fields.sticky(arg, quoted)?;
                    }
                    if quoted && self.fiber.params.is_empty() {
                        fields.sticky(b"", true)?;
                    }
                }
            }
        }
        if fields.present {
            fields.flush()?;
        }
        if args.len() as u64 + fields.out.len() as u64 > cap {
            return Err(Fail::Arguments);
        }
        // Move fields without copying their buffers, preserving their original order.
        args.try_reserve_exact(fields.out.len())?;
        for value in fields.out.as_mut_slice() {
            args.try_push(core::mem::replace(value, KBytes::new_in(budget)))?;
        }
        Ok(())
    }
}

fn number(n: i64) -> Diagnostic {
    let mut text = Diagnostic::new();
    let _ = write!(text, "{n}");
    text
}

struct Fields<'a> {
    budget: &'a Budget,
    value: KBytes,
    out: KVec<KBytes>,
    present: bool,
    pending: bool,
    non_ws: bool,
    cap: u64,
    string_cap: u64,
    split: bool,
}

impl Fields<'_> {
    fn flush(&mut self) -> Result<(), Fail> {
        if self.out.len() as u64 >= self.cap {
            return Err(Fail::Elements);
        }
        if !self.split && utf16_len(&self.value) > self.string_cap {
            return Err(Fail::StringLength);
        }
        self.out.try_push(core::mem::replace(
            &mut self.value,
            KBytes::new_in(self.budget),
        ))?;
        self.present = false;
        self.pending = false;
        Ok(())
    }

    fn sticky(&mut self, bytes: &[u8], quoted: bool) -> Result<(), Fail> {
        if bytes.is_empty() && !quoted {
            return Ok(());
        }
        if self.pending && self.present {
            self.flush()?;
        }
        self.pending = false;
        self.non_ws = false;
        self.value.try_extend_from_slice(bytes)?;
        self.present = true;
        Ok(())
    }

    fn add(&mut self, bytes: &[u8], eligible: bool, quoted: bool, ifs: &[u8]) -> Result<(), Fail> {
        if !eligible {
            return self.sticky(bytes, quoted);
        }
        for chunk in bytes.utf8_chunks() {
            let valid = chunk.valid();
            let mut start = 0;
            for (i, c) in valid.char_indices() {
                let end = i + c.len_utf8();
                if !ifs
                    .windows(c.len_utf8())
                    .any(|s| s == &valid.as_bytes()[i..end])
                {
                    continue;
                }
                self.sticky(&valid.as_bytes()[start..i], false)?;
                start = end;
                if matches!(c, ' ' | '\t' | '\n') {
                    self.pending = self.present && !self.non_ws;
                } else {
                    self.flush()?;
                    self.non_ws = true;
                }
            }
            self.sticky(&valid.as_bytes()[start..], false)?;
            self.sticky(chunk.invalid(), false)?;
        }
        Ok(())
    }
}
