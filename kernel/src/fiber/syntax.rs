//! The L2 lexer and bounded recursive-descent parser. AST edges are indexes into flat tables.
use crate::mem::{Budget, KBytes, KVec, MemError};
use core::fmt;
use core::ops::Range;

const TOKEN_CAP: usize = 100_000;
const DEPTH_CAP: usize = 200;
const STEP_CAP: usize = 1_000_000;

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
    matches!(c, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' |
        '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
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

/// Remove leading whitespace only on lines that start outside a continued quote or heredoc.
pub(super) fn normalize(source: &[u8], budget: &Budget) -> Result<KBytes, MemError> {
    let mut out = KBytes::with_capacity_in(source.len(), budget)?;
    let mut quote = 0;
    let mut heredoc: Option<(&[u8], bool)> = None;
    for (line_no, line) in source.split(|&b| b == b'\n').enumerate() {
        if line_no != 0 {
            out.try_push(b'\n')?;
        }
        if let Some((delimiter, strip_tabs)) = heredoc {
            let mut closing = line;
            if strip_tabs {
                while let Some(rest) = closing.strip_prefix(b"\t") {
                    closing = rest;
                }
            }
            if closing == delimiter {
                heredoc = None;
                out.try_extend_from_slice(trim_start(line))?;
            } else {
                out.try_extend_from_slice(line)?;
            }
            continue;
        }
        let outside = quote == 0;
        let line = if outside { trim_start(line) } else { line };
        out.try_extend_from_slice(line)?;
        let mut i = 0;
        while i < line.len() {
            let b = line[i];
            match (quote, b) {
                (b'\'', b'\'') | (b'"', b'"') => quote = 0,
                (b'\'', _) => {}
                (_, b'\\') => i += 1,
                (0, b'\'' | b'"') => quote = b,
                (0, b'#') if i == 0 || preceding_space(line, i) => break,
                (0, b'<') if outside && line.get(i + 1) == Some(&b'<') => {
                    i += 2;
                    let tabs = line.get(i) == Some(&b'-');
                    i += usize::from(tabs);
                    while line.get(i).is_some_and(|b| matches!(b, b' ' | b'\t')) {
                        i += 1;
                    }
                    let q = line.get(i).copied().filter(|b| matches!(b, b'\'' | b'"'));
                    i += usize::from(q.is_some());
                    let start = i;
                    while line.get(i).is_some_and(|&b| ident_byte(b) || b == b'-') {
                        i += 1;
                    }
                    if i > start && q.is_none_or(|q| line.get(i) == Some(&q)) {
                        heredoc = Some((&line[start..i], tabs));
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

#[derive(Clone, Copy, Debug)]
pub(super) enum Error {
    Memory(MemError),
    Quote(u32, u8),
    Brace,
    Tokens(usize),
    Depth(u32, u32),
    Steps(u32, u32),
    Unexpected(u32, u32),
}

impl From<MemError> for Error {
    fn from(error: MemError) -> Self {
        Self::Memory(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Quote(line, quote) => writeln!(f, "bash: line {line}: unexpected EOF while looking for matching `{}'", quote as char),
            Self::Brace => writeln!(f, "bash: syntax error: Parse error at 0:0: unexpected EOF while looking for matching '}}'"),
            Self::Tokens(n) => writeln!(f, "bash: syntax error: Parse error at 1:1: Too many tokens: {n} exceeds limit of {TOKEN_CAP}"),
            Self::Depth(line, col) => writeln!(f, "bash: syntax error: Parse error at {line}:{col}: Maximum parser nesting depth exceeded ({DEPTH_CAP})"),
            Self::Steps(line, col) => writeln!(f, "bash: syntax error: Parse error at {line}:{col}: Maximum parse iterations exceeded (possible infinite loop)"),
            Self::Unexpected(line, col) => writeln!(f, "bash: syntax error: Parse error at {line}:{col}: unexpected token"),
            Self::Memory(e) => write!(f, "{e:?}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Newline,
    Semi,
    And,
    Or,
    Pipe,
    PipeErr,
    Open,
    Close,
    Unsupported,
    Eof,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PartKind {
    Literal,
    Quoted,
    Escaped,
    Param,
    QuotedParam,
}

#[derive(Clone, Copy)]
pub(super) struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    fn new(start: usize, end: usize) -> Self {
        Self {
            start: start as u32,
            end: end as u32,
        }
    }
    pub fn get(self, input: &[u8]) -> &[u8] {
        &input[self.start as usize..self.end as usize]
    }
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Clone, Copy)]
pub(super) struct Part {
    pub text: Span,
    pub kind: PartKind,
}

#[derive(Clone, Copy)]
pub(super) struct Word {
    pub text: Span,
    pub parts: Span,
}

#[derive(Clone, Copy)]
struct Token {
    kind: Kind,
    word: Word,
}

struct Lexer<'a> {
    src: &'a [u8],
    at: usize,
    count: usize,
    tokens: KVec<Token>,
    parts: KVec<Part>,
}

impl Lexer<'_> {
    fn lex(&mut self) -> Result<(), Error> {
        while self.at < self.src.len() {
            let start = self.at;
            let rest = &self.src[start..];
            let (kind, n) = match rest {
                [b' ' | b'\t', ..] => {
                    self.at += 1;
                    continue;
                }
                [b'\\', b'\n', ..] => {
                    self.at += 2;
                    continue;
                }
                [b'#', ..] => {
                    self.at += rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
                    continue;
                }
                [b'&', b'&', ..] => (Kind::And, 2),
                [b'|', b'|', ..] => (Kind::Or, 2),
                [b'|', b'&', ..] => (Kind::PipeErr, 2),
                [b'|', ..] => (Kind::Pipe, 1),
                [b';', b';' | b'&', ..] => (Kind::Unsupported, 2),
                [b';', ..] => (Kind::Semi, 1),
                [b'\n', ..] => (Kind::Newline, 1),
                [b'(', ..] => (Kind::Open, 1),
                [b')', ..] => (Kind::Close, 1),
                [b'&' | b'<' | b'>', ..] => (Kind::Unsupported, 1),
                _ => (Kind::Word, 0),
            };
            let part_start = self.parts.len();
            self.count += 1;
            if kind == Kind::Word {
                self.word()?;
            } else {
                self.at += n;
            }
            if self.count <= TOKEN_CAP {
                self.tokens.try_push(Token {
                    kind,
                    word: Word {
                        text: Span::new(start, self.at),
                        parts: Span::new(part_start, self.parts.len()),
                    },
                })?;
            }
        }
        if self.count > TOKEN_CAP {
            return Err(Error::Tokens(self.count));
        }
        Ok(())
    }

    fn part(&mut self, start: usize, end: usize, kind: PartKind) -> Result<(), Error> {
        if self.count <= TOKEN_CAP {
            self.parts.try_push(Part {
                text: Span::new(start, end),
                kind,
            })?;
        }
        Ok(())
    }

    fn word(&mut self) -> Result<(), Error> {
        while let Some(&b) = self.src.get(self.at) {
            if delimiter(b) {
                break;
            }
            match b {
                b'\'' => self.single()?,
                b'"' => self.double()?,
                b'$' if self.src.get(self.at + 1) == Some(&b'"') => {
                    self.at += 1;
                    self.double()?;
                }
                b'$' => self.param(false)?,
                b'\\' => self.escape(PartKind::Escaped)?,
                _ => {
                    let start = self.at;
                    self.at += 1;
                    while self.src.get(self.at).is_some_and(|&c| {
                        !delimiter(c) && !matches!(c, b'\'' | b'"' | b'$' | b'\\')
                    }) {
                        self.at += 1;
                    }
                    self.part(start, self.at, PartKind::Literal)?;
                }
            }
        }
        Ok(())
    }

    fn single(&mut self) -> Result<(), Error> {
        let start = self.at + 1;
        let end = self.src[start..]
            .iter()
            .position(|&b| b == b'\'')
            .ok_or_else(|| Error::Quote(location(self.src, self.at).0, b'\''))?
            + start;
        self.at = end + 1;
        self.part(start, end, PartKind::Quoted)
    }

    fn double(&mut self) -> Result<(), Error> {
        let origin = self.at;
        self.at += 1;
        let mut has_part = false;
        loop {
            match self.src.get(self.at).copied() {
                None => return Err(Error::Quote(location(self.src, origin).0, b'"')),
                Some(b'"') => {
                    if !has_part {
                        self.part(self.at, self.at, PartKind::Quoted)?;
                    }
                    self.at += 1;
                    return Ok(());
                }
                Some(b'$') => {
                    self.param(true)?;
                    has_part = true;
                }
                Some(b'\\')
                    if self
                        .src
                        .get(self.at + 1)
                        .is_some_and(|b| matches!(b, b'"' | b'\\' | b'$' | b'`' | b'\n')) =>
                {
                    has_part |= self.src[self.at + 1] != b'\n';
                    self.escape(PartKind::Quoted)?;
                }
                Some(_) => {
                    let start = self.at;
                    self.at += 1;
                    while self
                        .src
                        .get(self.at)
                        .is_some_and(|&c| !matches!(c, b'"' | b'$' | b'\\'))
                    {
                        self.at += 1;
                    }
                    self.part(start, self.at, PartKind::Quoted)?;
                    has_part = true;
                }
            }
        }
    }

    fn escape(&mut self, kind: PartKind) -> Result<(), Error> {
        let start = self.at;
        self.at += 1;
        match self.src.get(self.at) {
            Some(b'\n') => {
                self.at += 1;
                Ok(())
            }
            Some(_) => {
                self.at += 1;
                self.part(start + 1, self.at, kind)
            }
            None => self.part(start, self.at, kind),
        }
    }

    fn param(&mut self, quoted: bool) -> Result<(), Error> {
        let dollar = self.at;
        self.at += 1;
        let mut start = self.at;
        let kind = if quoted {
            PartKind::QuotedParam
        } else {
            PartKind::Param
        };
        match self.src.get(self.at).copied() {
            Some(b'{') => {
                start += 1;
                self.at = start;
                let mut depth = 1;
                for &b in &self.src[start..] {
                    if b == b'{' {
                        depth += 1;
                    }
                    if b == b'}' {
                        depth -= 1;
                    }
                    if depth == 0 {
                        break;
                    }
                    self.at += 1;
                }
                if depth != 0 {
                    return Err(Error::Brace);
                }
                self.at += 1;
                self.part(start, self.at - 1, kind)
            }
            Some(b) if ident_start(b) => {
                while self.src.get(self.at).is_some_and(|&b| ident_byte(b)) {
                    self.at += 1;
                }
                self.part(start, self.at, kind)
            }
            Some(b'?' | b'$' | b'!' | b'#' | b'@' | b'*' | b'-' | b'0'..=b'9') => {
                self.at += 1;
                self.part(start, self.at, kind)
            }
            _ => self.part(
                dollar,
                self.at,
                if quoted {
                    PartKind::Quoted
                } else {
                    PartKind::Literal
                },
            ),
        }
    }
}

fn delimiter(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
    )
}

fn location(src: &[u8], at: usize) -> (u32, u32) {
    let prefix = &src[..at];
    let line = prefix.iter().filter(|&&b| b == b'\n').count() as u32 + 1;
    let start = prefix
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    (line, utf16_len(&src[start..at]) as u32 + 1)
}

#[derive(Clone, Copy)]
pub(super) enum Command {
    Simple { words: Span, first_arg: u32 },
    Group(u32),
    Define { name: Span, body: u32 },
}

#[derive(Clone, Copy)]
pub(super) enum Pipe {
    Last,
    Stdout(u32),
    Both(u32),
}

#[derive(Clone, Copy)]
pub(super) struct Stage {
    pub command: Command,
    pub pipe: Pipe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Link {
    Start,
    Next,
    And,
    Or,
}

#[derive(Clone, Copy)]
pub(super) struct Term {
    pub stage: u32,
    pub negate: bool,
    pub link: Link,
    pub next: Option<u32>,
}

pub(super) struct Ast {
    tokens: KVec<Token>,
    pub parts: KVec<Part>,
    pub terms: KVec<Term>,
    pub stages: KVec<Stage>,
    pub entry: Option<u32>,
}

impl Ast {
    pub fn parse(source: &[u8], budget: &Budget) -> Result<Self, Error> {
        let mut lex = Lexer {
            src: source,
            at: 0,
            count: 0,
            tokens: KVec::new_in(budget),
            parts: KVec::new_in(budget),
        };
        lex.lex()?;
        let mut parser = Parser {
            ast: Ast {
                tokens: lex.tokens,
                parts: lex.parts,
                terms: KVec::new_in(budget),
                stages: KVec::new_in(budget),
                entry: None,
            },
            src: source,
            at: 0,
            depth: 0,
            steps: 0,
        };
        parser.ast.entry = parser.list(false)?;
        Ok(parser.ast)
    }

    pub fn word(&self, index: usize) -> Word {
        self.tokens[index].word
    }
}

struct Parser<'a> {
    ast: Ast,
    src: &'a [u8],
    at: usize,
    depth: usize,
    steps: usize,
}

impl Parser<'_> {
    fn kind(&self) -> Kind {
        self.ast.tokens.get(self.at).map_or(Kind::Eof, |t| t.kind)
    }

    fn plain(&self) -> &[u8] {
        if self.kind() == Kind::Word {
            self.ast.word(self.at).text.get(self.src)
        } else {
            b""
        }
    }

    fn pos(&self) -> (u32, u32) {
        location(
            self.src,
            self.ast
                .tokens
                .get(self.at)
                .map_or(self.src.len(), |t| t.word.text.start as usize),
        )
    }

    fn unexpected(&self) -> Error {
        let (line, col) = self.pos();
        Error::Unexpected(line, col)
    }

    fn step(&mut self) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > STEP_CAP {
            let (line, col) = self.pos();
            return Err(Error::Steps(line, col));
        }
        Ok(())
    }

    fn linebreak(&mut self) {
        while self.kind() == Kind::Newline {
            self.at += 1;
        }
    }

    fn list(&mut self, group: bool) -> Result<Option<u32>, Error> {
        self.linebreak();
        if !group && self.kind() == Kind::Eof {
            return Ok(None);
        }
        let mut entry = None;
        let mut previous: Option<usize> = None;
        let mut link = Link::Start;
        loop {
            self.step()?;
            let (stage, negate) = self.pipeline()?;
            let id = self.ast.terms.len();
            self.ast.terms.try_push(Term {
                stage,
                negate,
                link,
                next: None,
            })?;
            if let Some(prev) = previous {
                self.ast.terms.as_mut_slice()[prev].next = Some(id as u32);
            } else {
                entry = Some(id as u32);
            }
            previous = Some(id);
            link = match self.kind() {
                Kind::And => Link::And,
                Kind::Or => Link::Or,
                Kind::Semi | Kind::Newline => Link::Next,
                Kind::Eof if !group => break,
                _ => return Err(self.unexpected()),
            };
            self.at += 1;
            self.linebreak();
            if link == Link::Next
                && ((!group && self.kind() == Kind::Eof) || (group && self.plain() == b"}"))
            {
                break;
            }
        }
        Ok(entry)
    }

    fn pipeline(&mut self) -> Result<(u32, bool), Error> {
        let mut negate = false;
        while self.plain() == b"!" {
            self.at += 1;
            negate = !negate;
        }
        let mut first = None;
        let mut previous: Option<(usize, Kind)> = None;
        loop {
            self.step()?;
            let command = self.command()?;
            let index = self.ast.stages.len();
            self.ast.stages.try_push(Stage {
                command,
                pipe: Pipe::Last,
            })?;
            if let Some((prev, kind)) = previous {
                self.ast.stages.as_mut_slice()[prev].pipe = if kind == Kind::Pipe {
                    Pipe::Stdout(index as u32)
                } else {
                    Pipe::Both(index as u32)
                };
            } else {
                first = Some(index as u32);
            }
            let kind = self.kind();
            if !matches!(kind, Kind::Pipe | Kind::PipeErr) {
                return Ok((first.unwrap(), negate));
            }
            previous = Some((index, kind));
            self.at += 1;
            self.linebreak();
        }
    }

    fn group(&mut self) -> Result<u32, Error> {
        if self.plain() != b"{" {
            return Err(self.unexpected());
        }
        if self.depth >= DEPTH_CAP {
            let (line, col) = self.pos();
            return Err(Error::Depth(line, col));
        }
        self.at += 1;
        self.depth += 1;
        let entry = self.list(true)?.ok_or_else(|| self.unexpected())?;
        self.depth -= 1;
        self.at += 1;
        Ok(entry)
    }

    fn command(&mut self) -> Result<Command, Error> {
        self.step()?;
        match self.plain() {
            b"{" => return Ok(Command::Group(self.group()?)),
            b"}" | b"if" | b"then" | b"else" | b"elif" | b"fi" | b"for" | b"in" | b"do"
            | b"done" | b"while" | b"until" | b"case" | b"esac" | b"[[" | b"]]" | b"select"
            | b"coproc" => return Err(self.unexpected()),
            _ => {}
        }
        let function = self.plain() == b"function";
        if function {
            self.at += 1;
        }
        if self.kind() != Kind::Word {
            return Err(self.unexpected());
        }
        let name = self.ast.word(self.at).text;
        let parens = self
            .ast
            .tokens
            .get(self.at + 1)
            .is_some_and(|t| t.kind == Kind::Open);
        if function || parens {
            let raw = name.get(self.src);
            if !identifier(raw)
                || matches!(
                    raw,
                    b"function"
                        | b"if"
                        | b"then"
                        | b"else"
                        | b"elif"
                        | b"fi"
                        | b"for"
                        | b"in"
                        | b"do"
                        | b"done"
                        | b"while"
                        | b"until"
                        | b"case"
                        | b"esac"
                        | b"select"
                        | b"coproc"
                )
            {
                return Err(self.unexpected());
            }
            self.at += 1;
            if parens {
                self.at += 1;
                if self.kind() != Kind::Close {
                    return Err(self.unexpected());
                }
                self.at += 1;
            }
            self.linebreak();
            return Ok(Command::Define {
                name,
                body: self.group()?,
            });
        }
        let start = self.at;
        while self.kind() == Kind::Word && assignment(self.ast.word(self.at), self.src).is_some() {
            self.at += 1;
        }
        let first_arg = self.at as u32;
        while self.kind() == Kind::Word {
            self.at += 1;
        }
        Ok(Command::Simple {
            words: Span::new(start, self.at),
            first_arg,
        })
    }
}

pub(super) fn assignment(word: Word, source: &[u8]) -> Option<(Span, bool, u32)> {
    let text = word.text.get(source);
    let equal = text.iter().position(|&b| b == b'=')?;
    let lhs = &text[..equal];
    let name = lhs.strip_suffix(b"+").unwrap_or(lhs);
    identifier(name).then(|| {
        (
            Span::new(
                word.text.start as usize,
                word.text.start as usize + name.len(),
            ),
            name.len() != lhs.len(),
            word.text.start + equal as u32 + 1,
        )
    })
}
