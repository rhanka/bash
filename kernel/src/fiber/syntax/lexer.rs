//! Full bash lexer with line:col tracking, disambiguation, ANSI-C quoting, and heredoc bodies.
use super::ast::{Pos, Span};
use super::{ident_byte, ident_start, identifier, lexer_err, parser_err, Error, DEPTH_CAP, TOKEN_CAP};
use crate::mem::{Budget, KBytes, KVec};
use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Eof,
    Newline,
    Semicolon,
    Amp,
    Pipe,
    PipeAmp,
    AndAnd,
    OrOr,
    Bang,
    LParen,
    RParen,
    LBrace,
    RBrace,
    DBrackStart,
    DBrackEnd,
    DParenStart,
    DParenEnd,
    // Reserved words
    If,
    Then,
    Else,
    Elif,
    Fi,
    For,
    While,
    Until,
    Do,
    Done,
    Case,
    In,
    Esac,
    Function,
    Time,
    // Case terminators
    DSemi,
    SemiAnd,
    SemiSemiAnd,
    // Redirections
    Less,
    Great,
    DLess,
    DGreat,
    LessAnd,
    GreatAnd,
    LessGreat,
    DLessDash,
    Clobber,
    TLess,
    AndGreat,
    AndDGreat,
    // Words & special tokens
    FdVariable,
    AssignmentWord,
    Number,
    Name,
    Word,
    HeredocContent,
}

impl TokenKind {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Eof => "EOF",
            Self::Newline => "NEWLINE",
            Self::Semicolon => "SEMICOLON",
            Self::Amp => "AMP",
            Self::Pipe => "PIPE",
            Self::PipeAmp => "PIPE_AMP",
            Self::AndAnd => "AND_AND",
            Self::OrOr => "OR_OR",
            Self::Bang => "BANG",
            Self::LParen => "LPAREN",
            Self::RParen => "RPAREN",
            Self::LBrace => "LBRACE",
            Self::RBrace => "RBRACE",
            Self::DBrackStart => "DBRACK_START",
            Self::DBrackEnd => "DBRACK_END",
            Self::DParenStart => "DPAREN_START",
            Self::DParenEnd => "DPAREN_END",
            Self::If => "IF",
            Self::Then => "THEN",
            Self::Else => "ELSE",
            Self::Elif => "ELIF",
            Self::Fi => "FI",
            Self::For => "FOR",
            Self::While => "WHILE",
            Self::Until => "UNTIL",
            Self::Do => "DO",
            Self::Done => "DONE",
            Self::Case => "CASE",
            Self::In => "IN",
            Self::Esac => "ESAC",
            Self::Function => "FUNCTION",
            Self::Time => "TIME",
            Self::DSemi => "DSEMI",
            Self::SemiAnd => "SEMI_AND",
            Self::SemiSemiAnd => "SEMI_SEMI_AND",
            Self::Less => "LESS",
            Self::Great => "GREAT",
            Self::DLess => "DLESS",
            Self::DGreat => "DGREAT",
            Self::LessAnd => "LESSAND",
            Self::GreatAnd => "GREATAND",
            Self::LessGreat => "LESSGREAT",
            Self::DLessDash => "DLESSDASH",
            Self::Clobber => "CLOBBER",
            Self::TLess => "TLESS",
            Self::AndGreat => "AND_GREAT",
            Self::AndDGreat => "AND_DGREAT",
            Self::FdVariable => "FD_VARIABLE",
            Self::AssignmentWord => "ASSIGNMENT_WORD",
            Self::Number => "NUMBER",
            Self::Name => "NAME",
            Self::Word => "WORD",
            Self::HeredocContent => "HEREDOC_CONTENT",
        }
    }

    pub const fn is_redir_op(self) -> bool {
        matches!(
            self,
            Self::Less
                | Self::Great
                | Self::DLess
                | Self::DGreat
                | Self::LessAnd
                | Self::GreatAnd
                | Self::LessGreat
                | Self::DLessDash
                | Self::Clobber
                | Self::TLess
                | Self::AndGreat
                | Self::AndDGreat
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: TokenKind,
    /// Span into `LexOutput::bytes`.
    pub text: Span,
    /// Start byte offset in original source.
    pub src_start: u32,
    /// End byte offset in original source.
    pub src_end: u32,
    /// 1-based line and UTF-16 column of token start.
    pub pos: Pos,
    /// True if any quoting/escaping appeared in the word (or heredoc was quoted).
    pub quoted: bool,
}

pub struct LexOutput {
    inline: [Token; 4],
    inline_len: usize,
    heap: KVec<Token>,
    pub bytes: KVec<u8>,
    pub eof_pos: Pos,
    pub src_len: u32,
}

impl LexOutput {
    pub fn len(&self) -> usize {
        if self.heap.is_empty() {
            self.inline_len
        } else {
            self.heap.len()
        }
    }

    pub fn get(&self, idx: usize) -> Option<&Token> {
        if self.heap.is_empty() {
            (idx < self.inline_len).then(|| &self.inline[idx])
        } else {
            self.heap.get(idx)
        }
    }

    pub fn at(&self, idx: usize) -> Token {
        if self.heap.is_empty() {
            self.inline[idx]
        } else {
            self.heap[idx]
        }
    }
}

const EMPTY_TOKEN: Token = Token {
    kind: TokenKind::Eof,
    text: Span::empty(),
    src_start: 0,
    src_end: 0,
    pos: Pos { line: 1, col: 1 },
    quoted: false,
};

struct PendingHeredoc {
    delim: Span, // into `bytes`
    strip_tabs: bool,
    quoted: bool,
}

pub fn lex<'a>(
    src: &'a [u8],
    max_heredoc_size: u64,
    budget: &'a Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<LexOutput, Error> {
    let mut lx = Lexer {
        src,
        at: 0,
        line: 1,
        col: 1,
        count: 0,
        inline: [EMPTY_TOKEN; 4],
        inline_len: 0,
        tokens: KVec::new_in(budget),
        bytes: KVec::new_in(budget),
        pending_heredocs: KVec::new_in(budget),
        max_heredoc_size,
        budget,
        work,
        cond_depth: 0,
        dparen_active: false,
        dparen_inner_depth: 0,
        last_kind: None,
        expect_cmd: true,
        after_fn_name: false,
        depth: 0,
    };
    lx.run()?;
    let total_tokens = lx.count + 1; // include synthetic EOF token per spec §6.1.2
    if total_tokens > TOKEN_CAP {
        return Err(parser_err(
            Pos { line: 1, col: 1 },
            budget,
            format_args!("Too many tokens: {total_tokens} exceeds limit of {TOKEN_CAP}"),
        ));
    }
    Ok(LexOutput {
        inline: lx.inline,
        inline_len: lx.inline_len,
        heap: lx.tokens,
        bytes: lx.bytes,
        eof_pos: Pos {
            line: lx.line,
            col: lx.col,
        },
        src_len: src.len() as u32,
    })
}

struct Lexer<'a, 'w> {
    src: &'a [u8],
    at: usize,
    line: u32,
    col: u32,
    count: usize,
    inline: [Token; 4],
    inline_len: usize,
    tokens: KVec<Token>,
    bytes: KVec<u8>,
    pending_heredocs: KVec<PendingHeredoc>,
    max_heredoc_size: u64,
    budget: &'a Budget,
    work: &'w mut dyn FnMut(u64) -> Result<(), ()>,
    cond_depth: u32,
    dparen_active: bool,
    dparen_inner_depth: u32,
    last_kind: Option<TokenKind>,
    expect_cmd: bool,
    after_fn_name: bool,
    depth: u32,
}

impl<'a, 'w> Lexer<'a, 'w> {
    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    fn charge(&mut self) -> Result<(), Error> {
        (self.work)(1).map_err(|()| Error::Work)
    }

    fn enter_depth(&mut self) -> Result<(), Error> {
        self.charge()?;
        self.depth += 1;
        if self.depth > DEPTH_CAP {
            self.depth -= 1;
            return Err(parser_err(
                self.pos(),
                self.budget,
                format_args!("Maximum parser nesting depth exceeded ({DEPTH_CAP})"),
            ));
        }
        Ok(())
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.at).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.at + offset).copied()
    }

    /// Advance by one byte or one UTF-8 scalar, updating `line` and `col` (UTF-16 units).
    fn advance_one(&mut self) -> Option<u8> {
        let b = *self.src.get(self.at)?;
        if b == b'\n' {
            self.at += 1;
            self.line += 1;
            self.col = 1;
            return Some(b);
        }
        if b < 0x80 {
            self.at += 1;
            self.col += 1;
            return Some(b);
        }
        // Check at most the next 4 bytes for a valid UTF-8 scalar (O(1), not O(N)).
        let end = (self.at + 4).min(self.src.len());
        let rest = &self.src[self.at..end];
        let mut chunk = rest.utf8_chunks();
        if let Some(c) = chunk.next() {
            if let Some(ch) = c.valid().chars().next() {
                self.at += ch.len_utf8();
                self.col += ch.len_utf16() as u32;
                return Some(b);
            }
        }
        self.at += 1;
        self.col += 1;
        Some(b)
    }

    /// Advance one scalar/byte and append all its bytes to `out`.
    fn copy_one(&mut self, out: &mut KVec<u8>) -> Result<Option<u8>, Error> {
        let start = self.at;
        let Some(b) = self.advance_one() else {
            return Ok(None);
        };
        if self.count < TOKEN_CAP {
            out.try_extend_from_slice(&self.src[start..self.at])?;
        }
        Ok(Some(b))
    }

    fn push_bytes(&mut self, slice: &[u8]) -> Result<Span, Error> {
        let start = self.bytes.len();
        if self.count < TOKEN_CAP {
            self.bytes.try_extend_from_slice(slice)?;
        }
        Ok(Span::new(start, self.bytes.len()))
    }

    fn emit(&mut self, kind: TokenKind, text: Span, src_start: usize, pos: Pos, quoted: bool) -> Result<(), Error> {
        self.charge()?;
        self.count += 1;
        if self.count <= TOKEN_CAP {
            let tok = Token {
                kind,
                text,
                src_start: src_start as u32,
                src_end: self.at as u32,
                pos,
                quoted,
            };
            if self.tokens.is_empty() && self.inline_len < self.inline.len() {
                self.inline[self.inline_len] = tok;
                self.inline_len += 1;
            } else {
                if self.tokens.is_empty() {
                    self.tokens.try_reserve(8)?;
                    self.tokens
                        .try_extend_from_slice(&self.inline[..self.inline_len])?;
                }
                self.tokens.try_push(tok)?;
            }
        }
        let prev_kind = self.last_kind;
        self.after_fn_name = prev_kind == Some(TokenKind::Function)
            && matches!(kind, TokenKind::Name | TokenKind::Word)
            && !quoted;
        let is_time_p = prev_kind == Some(TokenKind::Time)
            && !quoted
            && matches!(kind, TokenKind::Word | TokenKind::Name)
            && self.count <= TOKEN_CAP
            && text.get(&self.bytes) == b"-p";
        self.last_kind = Some(kind);
        self.expect_cmd = is_time_p
            || matches!(
                kind,
                TokenKind::Newline
                    | TokenKind::Semicolon
                    | TokenKind::Amp
                    | TokenKind::Pipe
                    | TokenKind::PipeAmp
                    | TokenKind::AndAnd
                    | TokenKind::OrOr
                    | TokenKind::LParen
                    | TokenKind::LBrace
                    | TokenKind::Then
                    | TokenKind::Else
                    | TokenKind::Elif
                    | TokenKind::Do
                    | TokenKind::Bang
                    | TokenKind::Time
                    | TokenKind::DSemi
                    | TokenKind::SemiAnd
                    | TokenKind::SemiSemiAnd
                    | TokenKind::If
                    | TokenKind::While
                    | TokenKind::Until
            );
        Ok(())
    }

    fn run(&mut self) -> Result<(), Error> {
        while self.at < self.src.len() {
            // Skip horizontal whitespace and line continuations.
            match self.peek() {
                Some(b' ' | b'\t' | b'\r') => {
                    self.advance_one();
                    continue;
                }
                Some(b'\\') if self.peek_at(1) == Some(b'\n') => {
                    self.advance_one();
                    self.advance_one();
                    continue;
                }
                Some(b'#') if !self.dparen_active => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.advance_one();
                    }
                    continue;
                }
                _ => {}
            }

            let src_start = self.at;
            let pos = self.pos();
            let b = self.src[self.at];

            // Newline + heredoc body capture
            if b == b'\n' {
                self.advance_one();
                let text = self.push_bytes(b"\n")?;
                self.emit(TokenKind::Newline, text, src_start, pos, false)?;
                if !self.pending_heredocs.is_empty() {
                    self.read_heredocs()?;
                }
                continue;
            }

            // Operators
            if self.try_operator(src_start, pos)? {
                continue;
            }

            // Word / reserved word / number / name / assignment / fd_variable
            self.scan_word(src_start, pos)?;
        }
        Ok(())
    }

    fn try_operator(&mut self, src_start: usize, pos: Pos) -> Result<bool, Error> {
        let rest = &self.src[self.at..];
        match rest {
            [b';', b';', b'&', ..] => {
                self.advance_n(3);
                let text = self.push_bytes(b";;&")?;
                self.emit(TokenKind::SemiSemiAnd, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b';', b';', ..] => {
                self.advance_n(2);
                let text = self.push_bytes(b";;")?;
                self.emit(TokenKind::DSemi, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b';', b'&', ..] => {
                self.advance_n(2);
                let text = self.push_bytes(b";&")?;
                self.emit(TokenKind::SemiAnd, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b';', ..] => {
                self.advance_one();
                let text = self.push_bytes(b";")?;
                self.emit(TokenKind::Semicolon, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'&', b'&', ..] => {
                self.advance_n(2);
                let text = self.push_bytes(b"&&")?;
                self.emit(TokenKind::AndAnd, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'&', b'>', b'>', ..] => {
                self.advance_n(3);
                let text = self.push_bytes(b"&>>")?;
                self.emit(TokenKind::AndDGreat, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'&', b'>', ..] => {
                self.advance_n(2);
                let text = self.push_bytes(b"&>")?;
                self.emit(TokenKind::AndGreat, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'&', ..] => {
                self.advance_one();
                let text = self.push_bytes(b"&")?;
                self.emit(TokenKind::Amp, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'|', b'|', ..] => {
                self.advance_n(2);
                let text = self.push_bytes(b"||")?;
                self.emit(TokenKind::OrOr, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'|', b'&', ..] => {
                self.advance_n(2);
                let text = self.push_bytes(b"|&")?;
                self.emit(TokenKind::PipeAmp, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'|', ..] => {
                self.advance_one();
                let text = self.push_bytes(b"|")?;
                self.emit(TokenKind::Pipe, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'(', b'(', ..] => {
                let is_dparen = if self.last_kind == Some(TokenKind::For) {
                    true
                } else if !self.dparen_active && self.cond_depth == 0 && self.can_start_dparen() {
                    self.is_arithmetic_dparen(self.at)
                } else {
                    false
                };
                if is_dparen {
                    self.advance_n(2);
                    self.dparen_active = true;
                    self.dparen_inner_depth = 0;
                    let text = self.push_bytes(b"((")?;
                    self.emit(TokenKind::DParenStart, text, src_start, pos, false)?;
                    return Ok(true);
                }
                if self.dparen_active {
                    self.dparen_inner_depth += 1;
                }
                self.advance_one();
                let text = self.push_bytes(b"(")?;
                self.emit(TokenKind::LParen, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'(', ..] => {
                if self.dparen_active {
                    self.dparen_inner_depth += 1;
                }
                self.advance_one();
                let text = self.push_bytes(b"(")?;
                self.emit(TokenKind::LParen, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b')', b')', ..] if self.dparen_active && self.dparen_inner_depth == 0 => {
                self.advance_n(2);
                self.dparen_active = false;
                let text = self.push_bytes(b"))")?;
                self.emit(TokenKind::DParenEnd, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b')', ..] => {
                if self.dparen_active && self.dparen_inner_depth > 0 {
                    self.dparen_inner_depth -= 1;
                }
                self.advance_one();
                let text = self.push_bytes(b")")?;
                self.emit(TokenKind::RParen, text, src_start, pos, false)?;
                return Ok(true);
            }
            // Process substitution `<(...)` and `>(...)` outside `((...))` and `[[...]]`
            [b'<' | b'>', b'(', ..] if !self.dparen_active && self.cond_depth == 0 => {
                return Ok(false); // let scan_word handle process substitution!
            }
            [b'<', b'<', b'<', ..] if !self.dparen_active => {
                self.advance_n(3);
                let text = self.push_bytes(b"<<<")?;
                self.emit(TokenKind::TLess, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'<', b'<', b'-', ..] if !self.dparen_active => {
                self.advance_n(3);
                let text = self.push_bytes(b"<<-")?;
                self.emit(TokenKind::DLessDash, text, src_start, pos, false)?;
                self.prescan_heredoc_delim(true)?;
                return Ok(true);
            }
            [b'<', b'<', ..] if !self.dparen_active => {
                self.advance_n(2);
                let text = self.push_bytes(b"<<")?;
                self.emit(TokenKind::DLess, text, src_start, pos, false)?;
                self.prescan_heredoc_delim(false)?;
                return Ok(true);
            }
            [b'<', b'&', ..] if !self.dparen_active => {
                self.advance_n(2);
                let text = self.push_bytes(b"<&")?;
                self.emit(TokenKind::LessAnd, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'<', b'>', ..] if !self.dparen_active => {
                self.advance_n(2);
                let text = self.push_bytes(b"<>")?;
                self.emit(TokenKind::LessGreat, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'<', ..] if !self.dparen_active => {
                self.advance_one();
                let text = self.push_bytes(b"<")?;
                self.emit(TokenKind::Less, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'>', b'>', ..] if !self.dparen_active => {
                self.advance_n(2);
                let text = self.push_bytes(b">>")?;
                self.emit(TokenKind::DGreat, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'>', b'&', ..] if !self.dparen_active => {
                self.advance_n(2);
                let text = self.push_bytes(b">&")?;
                self.emit(TokenKind::GreatAnd, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'>', b'|', ..] if !self.dparen_active => {
                self.advance_n(2);
                let text = self.push_bytes(b">|")?;
                self.emit(TokenKind::Clobber, text, src_start, pos, false)?;
                return Ok(true);
            }
            [b'>', ..] if !self.dparen_active => {
                self.advance_one();
                let text = self.push_bytes(b">")?;
                self.emit(TokenKind::Great, text, src_start, pos, false)?;
                return Ok(true);
            }
            // Standalone `!` at token start (before delimiter, `(`, or EOF) -> BANG
            [b'!', rest @ ..] if !self.dparen_active && rest.first().is_none_or(|&c| is_delimiter(c)) => {
                self.advance_one();
                let text = self.push_bytes(b"!")?;
                self.emit(TokenKind::Bang, text, src_start, pos, false)?;
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn can_start_dparen(&self) -> bool {
        self.expect_cmd
            || self.after_fn_name
            || matches!(
                self.last_kind,
                None | Some(TokenKind::RParen) // e.g. `f() (( x++ ))`
            )
    }

    fn can_start_cond(&self) -> bool {
        self.cond_depth == 0
            && (self.expect_cmd
                || self.after_fn_name
                || matches!(self.last_kind, None | Some(TokenKind::RParen)))
    }

    /// Lookahead from `start` pointing at `((` to decide `DPAREN_START` vs two `LPAREN`s.
    fn is_arithmetic_dparen(&self, start: usize) -> bool {
        let mut i = start + 2;
        let mut depth: i32 = 2;
        let mut in_sq = false;
        let mut in_dq = false;
        let mut prev_was_word = false;
        let mut had_space_after_word = false;
        let mut has_command_like = false;

        while i < self.src.len() {
            let b = self.src[i];
            if in_sq {
                if b == b'\'' {
                    in_sq = false;
                }
                i += 1;
                continue;
            }
            if in_dq {
                if b == b'\\' && i + 1 < self.src.len() {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    in_dq = false;
                }
                i += 1;
                continue;
            }
            match b {
                b'\\' if i + 1 < self.src.len() => {
                    i += 2;
                    continue;
                }
                b'\'' => {
                    in_sq = true;
                    i += 1;
                }
                b'"' => {
                    in_dq = true;
                    i += 1;
                }
                b'`' => {
                    i += 1;
                    while i < self.src.len() && self.src[i] != b'`' {
                        if self.src[i] == b'\\' && i + 1 < self.src.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if i < self.src.len() {
                        i += 1;
                    }
                    prev_was_word = false;
                    had_space_after_word = false;
                }
                b'$' if self.src.get(i + 1) == Some(&b'(') => {
                    i += 2;
                    let mut p = 1u32;
                    while i < self.src.len() && p > 0 {
                        match self.src[i] {
                            b'\\' if i + 1 < self.src.len() => i += 2,
                            b'(' => { p += 1; i += 1; }
                            b')' => { p -= 1; i += 1; }
                            _ => i += 1,
                        }
                    }
                    prev_was_word = false;
                    had_space_after_word = false;
                }
                b'(' => {
                    depth += 1;
                    prev_was_word = false;
                    had_space_after_word = false;
                    i += 1;
                }
                b')' => {
                    if depth == 2 {
                        // Adjacent `))` closes arithmetic command unless the body looks like a command (`echo deep`).
                        if self.src.get(i + 1) == Some(&b')') {
                            return !has_command_like;
                        }
                        // Depth dropped from 2 -> 1 at a single `)` -> nested subshells!
                        return false;
                    }
                    depth -= 1;
                    prev_was_word = false;
                    had_space_after_word = false;
                    i += 1;
                }
                b' ' | b'\t' | b'\r' | b'\n' => {
                    if prev_was_word {
                        had_space_after_word = true;
                    }
                    i += 1;
                }
                b';' => {
                    // Semicolon inside `((...))` (not after `for`) indicates command list in subshell.
                    return false;
                }
                _ if ident_byte(b) => {
                    if prev_was_word && had_space_after_word {
                        has_command_like = true;
                    }
                    while i < self.src.len() && ident_byte(self.src[i]) {
                        i += 1;
                    }
                    prev_was_word = true;
                    had_space_after_word = false;
                }
                _ => {
                    prev_was_word = false;
                    had_space_after_word = false;
                    i += 1;
                }
            }
        }
        // Unclosed `((` -> treat as nested subshells `( (` so parser reports Expected RPAREN.
        false
    }

    fn advance_n(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_one();
        }
    }

    fn scan_word(&mut self, src_start: usize, pos: Pos) -> Result<(), Error> {
        let in_dparen = self.dparen_active;
        // Fast path: plain ASCII identifier/word with no quoting/special chars (zero temp heap alloc).
        if !in_dparen {
            let mut end = self.at;
            while let Some(&b) = self.src.get(end) {
                if is_delimiter(b)
                    || matches!(b, b'\\' | b'\'' | b'"' | b'$' | b'`' | b'[' | b'=' | b'{' | b'}')
                    || b >= 0x80
                {
                    break;
                }
                end += 1;
            }
            if end > self.at
                && self
                    .src
                    .get(end)
                    .is_none_or(|&b| is_delimiter(b) && !(b == b'(' && matches!(self.src[end - 1], b'@' | b'*' | b'+' | b'?' | b'!')))
            {
                let slice = &self.src[self.at..end];
                self.col += (end - self.at) as u32;
                self.at = end;
                let kind = self.classify_word(slice, false);
                if kind == TokenKind::DBrackStart {
                    self.cond_depth += 1;
                } else if kind == TokenKind::DBrackEnd && self.cond_depth > 0 {
                    self.cond_depth -= 1;
                }
                let text = self.push_bytes(slice)?;
                self.emit(kind, text, src_start, pos, false)?;
                return Ok(());
            }
        }

        let mut buf = KBytes::new_in(self.budget);
        let mut quoted = false;

        while let Some(b) = self.peek() {
            if in_dparen {
                if matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b';') {
                    break;
                }
            } else if is_delimiter(b) {
                // Extglob: `@(...)`, `*(...)`, `+(...)`, `?(...)`, `<prefix>!(...)`
                if b == b'('
                    && !buf.is_empty()
                    && matches!(buf.last().copied(), Some(b'@' | b'*' | b'+' | b'?' | b'!'))
                {
                    self.scan_balanced_parens(&mut buf)?;
                    continue;
                }
                // Process substitution `<(...)` / `>(...)` inside word
                if matches!(b, b'<' | b'>') && self.peek_at(1) == Some(b'(') && self.cond_depth == 0 {
                    self.copy_one(&mut buf)?;
                    self.scan_proc_or_cmd_subst(&mut buf, b'(')?;
                    continue;
                }
                break;
            }

            match b {
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    quoted = true;
                    self.copy_one(&mut buf)?;
                    if self.peek().is_some() {
                        self.copy_one(&mut buf)?;
                    }
                }
                b'\'' => {
                    quoted = true;
                    self.scan_single_quote(&mut buf)?;
                }
                b'"' => {
                    quoted = true;
                    self.scan_double_quote(&mut buf)?;
                }
                b'$' => {
                    match self.peek_at(1) {
                        Some(b'\'') => {
                            quoted = true;
                            self.copy_one(&mut buf)?; // $
                            self.scan_ansic_quote(&mut buf)?;
                        }
                        Some(b'"') => {
                            quoted = true;
                            self.copy_one(&mut buf)?; // $
                            self.scan_double_quote(&mut buf)?;
                        }
                        Some(b'{') => {
                            self.copy_one(&mut buf)?; // $
                            self.scan_brace_expand(&mut buf)?;
                        }
                        Some(b'(') => {
                            self.copy_one(&mut buf)?; // $
                            if self.peek_at(1) == Some(b'(') && self.is_arith_subst(self.at) {
                                self.scan_arith_subst(&mut buf)?;
                            } else {
                                self.scan_proc_or_cmd_subst(&mut buf, b'(')?;
                            }
                        }
                        Some(b'[') => {
                            self.copy_one(&mut buf)?; // $
                            self.scan_legacy_arith(&mut buf)?;
                        }
                        _ => {
                            self.copy_one(&mut buf)?;
                        }
                    }
                }
                b'`' => {
                    self.scan_backtick(&mut buf, false)?;
                }
                b'[' if !in_dparen && !quoted && identifier(&buf) => {
                    // Check if `[...]` is followed by `=` or `+=` (array subscript assignment).
                    if self.peek_subscript_assign(self.at) {
                        self.scan_balanced_brackets(&mut buf)?;
                    } else {
                        self.copy_one(&mut buf)?;
                    }
                }
                b'=' if !in_dparen && !quoted && self.peek_at(1) == Some(b'(') && is_assign_lhs(&buf) => {
                    // `name=(...)` or `name+=(...)` or `name[sub]=(...)`: absorb `=(...)` as one token!
                    self.copy_one(&mut buf)?; // '='
                    self.scan_array_literal(&mut buf)?;
                }
                _ => {
                    self.copy_one(&mut buf)?;
                }
            }
        }

        // Check `{varname}` immediately followed by `<` or `>` -> FD_VARIABLE!
        if !in_dparen
            && !quoted
            && buf.len() >= 3
            && buf[0] == b'{'
            && buf[buf.len() - 1] == b'}'
            && identifier(&buf[1..buf.len() - 1])
            && matches!(self.peek(), Some(b'<' | b'>'))
        {
            let text = self.push_bytes(&buf[1..buf.len() - 1])?;
            self.emit(TokenKind::FdVariable, text, src_start, pos, false)?;
            return Ok(());
        }

        let kind = self.classify_word(&buf, quoted);
        if kind == TokenKind::DBrackStart {
            self.cond_depth += 1;
        } else if kind == TokenKind::DBrackEnd && self.cond_depth > 0 {
            self.cond_depth -= 1;
        }
        let text = self.push_bytes(&buf)?;
        self.emit(kind, text, src_start, pos, quoted)?;
        Ok(())
    }

    fn classify_word(&self, buf: &[u8], quoted: bool) -> TokenKind {
        if self.dparen_active {
            if !quoted && !buf.is_empty() && buf.iter().all(|b| b.is_ascii_digit()) {
                return TokenKind::Number;
            }
            if !quoted && identifier(buf) {
                return TokenKind::Name;
            }
            return TokenKind::Word;
        }
        if !quoted {
            match buf {
                b"if" => return TokenKind::If,
                b"then" => return TokenKind::Then,
                b"else" => return TokenKind::Else,
                b"elif" => return TokenKind::Elif,
                b"fi" => return TokenKind::Fi,
                b"for" => return TokenKind::For,
                b"while" => return TokenKind::While,
                b"until" => return TokenKind::Until,
                b"do" => return TokenKind::Do,
                b"done" => return TokenKind::Done,
                b"case" => return TokenKind::Case,
                b"in" => return TokenKind::In,
                b"esac" => return TokenKind::Esac,
                b"function" => return TokenKind::Function,
                b"time" => return TokenKind::Time,
                b"{" => {
                    if self.peek().is_none_or(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n')) {
                        return TokenKind::LBrace;
                    }
                    return TokenKind::Word;
                }
                b"}" => return TokenKind::RBrace,
                b"[[" if self.can_start_cond() => return TokenKind::DBrackStart,
                b"]]" if self.cond_depth > 0 => return TokenKind::DBrackEnd,
                _ => {}
            }
            if !buf.is_empty() && buf.iter().all(|b| b.is_ascii_digit()) {
                return TokenKind::Number;
            }
            if identifier(buf) {
                return TokenKind::Name;
            }
        }
        if is_assignment_word(buf) {
            return TokenKind::AssignmentWord;
        }
        TokenKind::Word
    }

    fn scan_single_quote(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        let open_line = self.line;
        self.copy_one(buf)?; // opening '\''
        while let Some(b) = self.peek() {
            self.copy_one(buf)?;
            if b == b'\'' {
                return Ok(());
            }
        }
        Err(lexer_err(
            open_line,
            self.budget,
            format_args!("unexpected EOF while looking for matching `''"),
        ))
    }

    fn scan_ansic_quote(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        let open_line = self.line;
        self.copy_one(buf)?; // opening '\''
        while let Some(b) = self.peek() {
            if b == b'\\' {
                self.copy_one(buf)?;
                if self.peek().is_some() {
                    self.copy_one(buf)?;
                }
                continue;
            }
            self.copy_one(buf)?;
            if b == b'\'' {
                return Ok(());
            }
        }
        Err(lexer_err(
            open_line,
            self.budget,
            format_args!("unexpected EOF while looking for matching `''"),
        ))
    }

    fn scan_double_quote(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_double_quote_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_double_quote_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        let open_line = self.line;
        self.copy_one(buf)?; // opening '"'
        while let Some(b) = self.peek() {
            match b {
                b'"' => {
                    self.copy_one(buf)?;
                    return Ok(());
                }
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'$' => match self.peek_at(1) {
                    Some(b'{') => {
                        self.copy_one(buf)?;
                        self.scan_brace_expand(buf)?;
                    }
                    Some(b'(') => {
                        self.copy_one(buf)?;
                        if self.peek_at(1) == Some(b'(') && self.is_arith_subst(self.at) {
                            self.scan_arith_subst(buf)?;
                        } else {
                            self.scan_proc_or_cmd_subst(buf, b'(')?;
                        }
                    }
                    Some(b'[') => {
                        self.copy_one(buf)?;
                        self.scan_legacy_arith(buf)?;
                    }
                    _ => {
                        self.copy_one(buf)?;
                    }
                },
                b'`' => {
                    self.scan_backtick(buf, true)?;
                }
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        Err(lexer_err(
            open_line,
            self.budget,
            format_args!("unexpected EOF while looking for matching `\"'"),
        ))
    }

    fn scan_brace_expand(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_brace_expand_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_brace_expand_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        // Consume `{`.
        self.copy_one(buf)?;
        let mut depth: u32 = 1;
        while let Some(b) = self.peek() {
            match b {
                b'}' => {
                    self.copy_one(buf)?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b'{' => {
                    self.copy_one(buf)?;
                    depth += 1;
                }
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'\'' => self.scan_single_quote(buf)?,
                b'"' => self.scan_double_quote(buf)?,
                b'$' => match self.peek_at(1) {
                    Some(b'\'') => {
                        self.copy_one(buf)?;
                        self.scan_ansic_quote(buf)?;
                    }
                    Some(b'{') => {
                        self.copy_one(buf)?;
                        self.scan_brace_expand(buf)?;
                    }
                    Some(b'(') => {
                        self.copy_one(buf)?;
                        if self.peek_at(1) == Some(b'(') && self.is_arith_subst(self.at) {
                            self.scan_arith_subst(buf)?;
                        } else {
                            self.scan_proc_or_cmd_subst(buf, b'(')?;
                        }
                    }
                    _ => {
                        self.copy_one(buf)?;
                    }
                },
                b'`' => self.scan_backtick(buf, false)?,
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        // Unclosed `${` -> exact 0:0 Parse error per spec 6.2.5 and oracle snapshot.
        Err(parser_err(
            Pos { line: 0, col: 0 },
            self.budget,
            format_args!("unexpected EOF while looking for matching '}}'"),
        ))
    }

    /// Disambiguate `$(( ... ))` from `$( ( ... ) )` starting at the first `(`.
    fn is_arith_subst(&self, start: usize) -> bool {
        if self.depth >= DEPTH_CAP {
            return false;
        }
        let mut i = start + 2;
        let mut depth: i32 = 2;
        let mut in_sq = false;
        let mut in_dq = false;
        while i < self.src.len() {
            let b = self.src[i];
            if in_sq {
                if b == b'\'' {
                    in_sq = false;
                }
                i += 1;
                continue;
            }
            if in_dq {
                if b == b'\\' && i + 1 < self.src.len() {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    in_dq = false;
                }
                i += 1;
                continue;
            }
            match b {
                b'\\' if i + 1 < self.src.len() => i += 2,
                b'\'' => {
                    in_sq = true;
                    i += 1;
                }
                b'"' => {
                    in_dq = true;
                    i += 1;
                }
                b'(' => {
                    depth += 1;
                    i += 1;
                }
                b')' => {
                    if depth == 2 {
                        return self.src.get(i + 1) == Some(&b')');
                    }
                    depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        // Unclosed `$((` -> treat as arith subst or cmd subst (unclosed `)` at EOF).
        true
    }

    fn scan_arith_subst(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_arith_subst_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_arith_subst_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        // Consume `((`.
        self.copy_one(buf)?;
        self.copy_one(buf)?;
        let mut depth: u32 = 2;
        while let Some(b) = self.peek() {
            match b {
                b'(' => {
                    self.copy_one(buf)?;
                    depth += 1;
                }
                b')' => {
                    if depth == 2 && self.peek_at(1) == Some(b')') {
                        self.copy_one(buf)?;
                        self.copy_one(buf)?;
                        return Ok(());
                    }
                    self.copy_one(buf)?;
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'\'' => self.scan_single_quote(buf)?,
                b'"' => self.scan_double_quote(buf)?,
                b'$' => match self.peek_at(1) {
                    Some(b'{') => {
                        self.copy_one(buf)?;
                        self.scan_brace_expand(buf)?;
                    }
                    Some(b'(') => {
                        self.copy_one(buf)?;
                        if self.peek_at(1) == Some(b'(') && self.is_arith_subst(self.at) {
                            self.scan_arith_subst(buf)?;
                        } else {
                            self.scan_proc_or_cmd_subst(buf, b'(')?;
                        }
                    }
                    _ => {
                        self.copy_one(buf)?;
                    }
                },
                b'`' => self.scan_backtick(buf, false)?,
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        Err(parser_err(
            self.pos(),
            self.budget,
            format_args!("unexpected EOF while looking for matching `)'"),
        ))
    }

    fn scan_legacy_arith(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_legacy_arith_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_legacy_arith_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.copy_one(buf)?; // '['
        let mut depth: u32 = 1;
        while let Some(b) = self.peek() {
            match b {
                b'[' => {
                    self.copy_one(buf)?;
                    depth += 1;
                }
                b']' => {
                    self.copy_one(buf)?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'\'' => self.scan_single_quote(buf)?,
                b'"' => self.scan_double_quote(buf)?,
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        Err(parser_err(
            self.pos(),
            self.budget,
            format_args!("unexpected EOF while looking for matching `]'"),
        ))
    }

    /// Scans `$(...)`, `<(...)`, or `>(...)` starting at `(`, aware of quotes, comments,
    /// `case ... in ... esac` patterns, nested `$((...))`, and heredocs.
    fn scan_proc_or_cmd_subst(&mut self, buf: &mut KVec<u8>, open: u8) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_proc_or_cmd_subst_inner(buf, open);
        self.depth -= 1;
        res
    }

    fn scan_proc_or_cmd_subst_inner(&mut self, buf: &mut KVec<u8>, _open: u8) -> Result<(), Error> {
        self.copy_one(buf)?; // '('
        let mut paren_depth: u32 = 1;
        let mut case_depth: u32 = 0;
        let mut in_case_pat = false;
        let mut word_start = true;
        let mut word_buf: [u8; 8] = [0; 8];
        let mut word_len: usize = 0;

        while let Some(b) = self.peek() {
            // Check word completion on delimiter
            if is_delimiter(b) && word_len > 0 {
                let w = &word_buf[..word_len];
                if w == b"case" {
                    case_depth += 1;
                } else if w == b"in" && case_depth > 0 {
                    in_case_pat = true;
                } else if w == b"esac" && case_depth > 0 {
                    case_depth -= 1;
                    in_case_pat = false;
                }
                word_len = 0;
            }
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.copy_one(buf)?;
                    word_start = true;
                }
                b'#' if word_start => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.copy_one(buf)?;
                    }
                }
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                    word_start = false;
                    word_len = 0;
                }
                b'\'' => {
                    self.scan_single_quote(buf)?;
                    word_start = false;
                    word_len = 0;
                }
                b'"' => {
                    self.scan_double_quote(buf)?;
                    word_start = false;
                    word_len = 0;
                }
                b'$' => {
                    word_start = false;
                    word_len = 0;
                    match self.peek_at(1) {
                        Some(b'\'') => {
                            self.copy_one(buf)?;
                            self.scan_ansic_quote(buf)?;
                        }
                        Some(b'{') => {
                            self.copy_one(buf)?;
                            self.scan_brace_expand(buf)?;
                        }
                        Some(b'(') => {
                            self.copy_one(buf)?;
                            if self.peek_at(1) == Some(b'(') && self.is_arith_subst(self.at) {
                                self.scan_arith_subst(buf)?;
                            } else {
                                self.scan_proc_or_cmd_subst(buf, b'(')?;
                            }
                        }
                        Some(b'[') => {
                            self.copy_one(buf)?;
                            self.scan_legacy_arith(buf)?;
                        }
                        _ => {
                            self.copy_one(buf)?;
                        }
                    }
                }
                b'`' => {
                    self.scan_backtick(buf, false)?;
                    word_start = false;
                    word_len = 0;
                }
                b';' => {
                    if case_depth > 0 && self.peek_at(1) == Some(b';') {
                        self.copy_one(buf)?;
                        self.copy_one(buf)?;
                        if self.peek() == Some(b'&') {
                            self.copy_one(buf)?;
                        }
                        in_case_pat = true;
                    } else if case_depth > 0 && self.peek_at(1) == Some(b'&') {
                        self.copy_one(buf)?;
                        self.copy_one(buf)?;
                        in_case_pat = true;
                    } else {
                        self.copy_one(buf)?;
                    }
                    word_start = true;
                }
                b'(' => {
                    self.copy_one(buf)?;
                    if !in_case_pat {
                        paren_depth += 1;
                    }
                    word_start = true;
                }
                b')' => {
                    self.copy_one(buf)?;
                    if in_case_pat {
                        in_case_pat = false;
                        word_start = true;
                    } else {
                        paren_depth -= 1;
                        if paren_depth == 0 {
                            return Ok(());
                        }
                        word_start = true;
                    }
                }
                _ => {
                    self.copy_one(buf)?;
                    if word_start || word_len > 0 {
                        if word_len < word_buf.len() {
                            word_buf[word_len] = b;
                            word_len += 1;
                        } else {
                            word_len = 0;
                            word_start = false;
                        }
                    }
                    if word_start && b.is_ascii_alphabetic() {
                        word_start = false;
                    } else if !b.is_ascii_alphabetic() {
                        word_len = 0;
                        word_start = false;
                    }
                }
            }
        }
        Err(parser_err(
            self.pos(),
            self.budget,
            format_args!("unexpected EOF while looking for matching `)'"),
        ))
    }

    fn scan_backtick(&mut self, buf: &mut KVec<u8>, _in_dq: bool) -> Result<(), Error> {
        self.copy_one(buf)?; // opening '`'
        while let Some(b) = self.peek() {
            if b == b'\\' {
                if self.peek_at(1) == Some(b'\n') {
                    self.advance_n(2);
                    continue;
                }
                self.copy_one(buf)?;
                if self.peek().is_some() {
                    self.copy_one(buf)?;
                }
                continue;
            }
            self.copy_one(buf)?;
            if b == b'`' {
                return Ok(());
            }
        }
        Err(parser_err(
            self.pos(),
            self.budget,
            format_args!("unexpected EOF while looking for matching ``'"),
        ))
    }

    fn scan_balanced_parens(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_balanced_parens_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_balanced_parens_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.copy_one(buf)?; // '('
        let mut depth: u32 = 1;
        while let Some(b) = self.peek() {
            match b {
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'\'' => self.scan_single_quote(buf)?,
                b'"' => self.scan_double_quote(buf)?,
                b'(' => {
                    self.copy_one(buf)?;
                    depth += 1;
                }
                b')' => {
                    self.copy_one(buf)?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        Ok(())
    }

    fn scan_array_literal(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_array_literal_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_array_literal_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.copy_one(buf)?; // '('
        let mut depth: u32 = 1;
        while let Some(b) = self.peek() {
            match b {
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'\'' => self.scan_single_quote(buf)?,
                b'"' => self.scan_double_quote(buf)?,
                b'$' => match self.peek_at(1) {
                    Some(b'\'') => {
                        self.copy_one(buf)?;
                        self.scan_ansic_quote(buf)?;
                    }
                    Some(b'{') => {
                        self.copy_one(buf)?;
                        self.scan_brace_expand(buf)?;
                    }
                    Some(b'(') => {
                        self.copy_one(buf)?;
                        if self.peek_at(1) == Some(b'(') && self.is_arith_subst(self.at) {
                            self.scan_arith_subst(buf)?;
                        } else {
                            self.scan_proc_or_cmd_subst(buf, b'(')?;
                        }
                    }
                    _ => {
                        self.copy_one(buf)?;
                    }
                },
                b'`' => self.scan_backtick(buf, false)?,
                b'(' => {
                    self.copy_one(buf)?;
                    depth += 1;
                }
                b')' => {
                    self.copy_one(buf)?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        Err(parser_err(
            self.pos(),
            self.budget,
            format_args!("Expected RPAREN, got EOF"),
        ))
    }

    fn peek_subscript_assign(&self, start: usize) -> bool {
        let mut i = start + 1;
        let mut depth: u32 = 1;
        let mut in_sq = false;
        let mut in_dq = false;
        while i < self.src.len() && depth > 0 {
            let b = self.src[i];
            if in_sq {
                if b == b'\'' {
                    in_sq = false;
                }
                i += 1;
                continue;
            }
            if in_dq {
                if b == b'\\' && i + 1 < self.src.len() {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    in_dq = false;
                }
                i += 1;
                continue;
            }
            match b {
                b'\\' if i + 1 < self.src.len() => i += 2,
                b'\'' => {
                    in_sq = true;
                    i += 1;
                }
                b'"' => {
                    in_dq = true;
                    i += 1;
                }
                b'[' => {
                    depth += 1;
                    i += 1;
                }
                b']' => {
                    depth -= 1;
                    i += 1;
                }
                b'\n' => return false,
                _ => i += 1,
            }
        }
        if depth != 0 {
            return false;
        }
        matches!(
            self.src.get(i..),
            Some([b'=', ..]) | Some([b'+', b'=', ..])
        )
    }

    fn scan_balanced_brackets(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.enter_depth()?;
        let res = self.scan_balanced_brackets_inner(buf);
        self.depth -= 1;
        res
    }

    fn scan_balanced_brackets_inner(&mut self, buf: &mut KVec<u8>) -> Result<(), Error> {
        self.copy_one(buf)?; // '['
        let mut depth: u32 = 1;
        while let Some(b) = self.peek() {
            match b {
                b'\\' => {
                    if self.peek_at(1) == Some(b'\n') {
                        self.advance_n(2);
                        continue;
                    }
                    self.copy_one(buf)?;
                    if self.peek().is_some() {
                        self.copy_one(buf)?;
                    }
                }
                b'\'' => self.scan_single_quote(buf)?,
                b'"' => self.scan_double_quote(buf)?,
                b'[' => {
                    self.copy_one(buf)?;
                    depth += 1;
                }
                b']' => {
                    self.copy_one(buf)?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {
                    self.copy_one(buf)?;
                }
            }
        }
        Ok(())
    }

    /// Pre-scan the delimiter after `<<` / `<<-` without consuming it from the token stream.
    fn prescan_heredoc_delim(&mut self, strip_tabs: bool) -> Result<(), Error> {
        let mut i = self.at;
        while i < self.src.len() && matches!(self.src[i], b' ' | b'\t') {
            i += 1;
        }
        if i >= self.src.len() || is_delimiter(self.src[i]) {
            return Ok(());
        }
        let mut delim = KBytes::new_in(self.budget);
        let mut quoted = false;
        while i < self.src.len() && !is_delimiter(self.src[i]) {
            match self.src[i] {
                b'\'' => {
                    quoted = true;
                    i += 1;
                    let start = i;
                    while i < self.src.len() && self.src[i] != b'\'' {
                        i += 1;
                    }
                    if i >= self.src.len() {
                        return Err(lexer_err(
                            self.line,
                            self.budget,
                            format_args!("unexpected EOF while looking for matching `''"),
                        ));
                    }
                    delim.try_extend_from_slice(&self.src[start..i])?;
                    i += 1;
                }
                b'"' => {
                    quoted = true;
                    i += 1;
                    while i < self.src.len() && self.src[i] != b'"' {
                        if self.src[i] == b'\\' && i + 1 < self.src.len() {
                            i += 1;
                        }
                        delim.try_push(self.src[i])?;
                        i += 1;
                    }
                    if i >= self.src.len() {
                        return Err(lexer_err(
                            self.line,
                            self.budget,
                            format_args!("unexpected EOF while looking for matching `\"'"),
                        ));
                    }
                    i += 1;
                }
                b'$' if self.src.get(i + 1) == Some(&b'\'') => {
                    quoted = true;
                    i += 2;
                    let start = i;
                    while i < self.src.len() && self.src[i] != b'\'' {
                        if self.src[i] == b'\\' && i + 1 < self.src.len() {
                            i += 2;
                            continue;
                        }
                        i += 1;
                    }
                    if i >= self.src.len() {
                        return Err(lexer_err(
                            self.line,
                            self.budget,
                            format_args!("unexpected EOF while looking for matching `''"),
                        ));
                    }
                    delim.try_extend_from_slice(&self.src[start..i])?;
                    i += 1;
                }
                b'$' if self.src.get(i + 1) == Some(&b'"') => {
                    quoted = true;
                    i += 2;
                    while i < self.src.len() && self.src[i] != b'"' {
                        if self.src[i] == b'\\' && i + 1 < self.src.len() {
                            i += 1;
                        }
                        delim.try_push(self.src[i])?;
                        i += 1;
                    }
                    if i >= self.src.len() {
                        return Err(lexer_err(
                            self.line,
                            self.budget,
                            format_args!("unexpected EOF while looking for matching `\"'"),
                        ));
                    }
                    i += 1;
                }
                b'$' if self.src.get(i + 1) == Some(&b'(') => {
                    let start = i;
                    i += 2;
                    let mut p = 1u32;
                    while i < self.src.len() && p > 0 {
                        match self.src[i] {
                            b'(' => p += 1,
                            b')' => p -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    if p != 0 {
                        return Err(lexer_err(
                            self.line,
                            self.budget,
                            format_args!("unexpected EOF while looking for matching `)'"),
                        ));
                    }
                    delim.try_extend_from_slice(&self.src[start..i])?;
                }
                b'\\' => {
                    quoted = true;
                    i += 1;
                    if i < self.src.len() {
                        delim.try_push(self.src[i])?;
                        i += 1;
                    }
                }
                c => {
                    delim.try_push(c)?;
                    i += 1;
                }
            }
        }
        let span = self.push_bytes(&delim)?;
        self.pending_heredocs.try_push(PendingHeredoc {
            delim: span,
            strip_tabs,
            quoted,
        })?;
        Ok(())
    }

    fn read_heredocs(&mut self) -> Result<(), Error> {
        let pending = core::mem::replace(&mut self.pending_heredocs, KVec::new_in(self.budget));
        for hd in pending.iter() {
            let src_start = self.at;
            let pos = self.pos();
            let mut body = KBytes::new_in(self.budget);
            while self.at < self.src.len() {
                let line_start = self.at;
                let line_num = self.line;
                while let Some(b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.advance_one();
                }
                let line_end = self.at;
                let had_nl = if self.peek() == Some(b'\n') {
                    self.advance_one();
                    true
                } else {
                    false
                };
                let raw_line = &self.src[line_start..line_end];
                let check = if hd.strip_tabs {
                    strip_leading_tabs(raw_line)
                } else {
                    raw_line
                };
                if check == hd.delim.get(&self.bytes) {
                    break;
                }
                let content_line = if hd.strip_tabs {
                    strip_leading_tabs(raw_line)
                } else {
                    raw_line
                };
                if !hd.quoted && has_odd_trailing_backslashes(content_line) && had_nl {
                    body.try_extend_from_slice(&content_line[..content_line.len() - 1])?;
                } else {
                    body.try_extend_from_slice(content_line)?;
                    body.try_push(b'\n')?;
                }
                if body.len() as u64 > self.max_heredoc_size {
                    return Err(lexer_err(
                        line_num,
                        self.budget,
                        format_args!(
                            "Heredoc size limit exceeded ({} bytes)",
                            self.max_heredoc_size
                        ),
                    ));
                }
            }
            let text = self.push_bytes(&body)?;
            self.emit(TokenKind::HeredocContent, text, src_start, pos, hd.quoted)?;
        }
        Ok(())
    }
}

pub fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\r' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
    )
}

fn strip_leading_tabs(mut s: &[u8]) -> &[u8] {
    while let Some(rest) = s.strip_prefix(b"\t") {
        s = rest;
    }
    s
}

fn has_odd_trailing_backslashes(s: &[u8]) -> bool {
    s.iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

fn is_assign_lhs(lhs: &[u8]) -> bool {
    let lhs = lhs.strip_suffix(b"+").unwrap_or(lhs);
    if identifier(lhs) {
        return true;
    }
    // Check `ident[...]`
    let Some(lb) = lhs.iter().position(|&b| b == b'[') else {
        return false;
    };
    lb > 0 && identifier(&lhs[..lb]) && lhs.ends_with(b"]")
}

/// Checks if raw token bytes start with an unquoted assignment LHS followed by `=` or `+=`.
pub fn is_assignment_word(buf: &[u8]) -> bool {
    let mut i = 0;
    if !buf.first().is_some_and(|&b| ident_start(b)) {
        return false;
    }
    while i < buf.len() && ident_byte(buf[i]) {
        i += 1;
    }
    if buf.get(i) == Some(&b'[') {
        i += 1;
        let mut depth = 1u32;
        let mut in_sq = false;
        let mut in_dq = false;
        while i < buf.len() && depth > 0 {
            let b = buf[i];
            if in_sq {
                if b == b'\'' {
                    in_sq = false;
                }
                i += 1;
                continue;
            }
            if in_dq {
                if b == b'\\' && i + 1 < buf.len() {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    in_dq = false;
                }
                i += 1;
                continue;
            }
            match b {
                b'\\' if i + 1 < buf.len() => i += 2,
                b'\'' => {
                    in_sq = true;
                    i += 1;
                }
                b'"' => {
                    in_dq = true;
                    i += 1;
                }
                b'[' => {
                    depth += 1;
                    i += 1;
                }
                b']' => {
                    depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        if depth != 0 {
            return false;
        }
    }
    if buf.get(i) == Some(&b'+') {
        i += 1;
    }
    buf.get(i) == Some(&b'=')
}

/// Decode ANSI-C `$'...'` body bytes (between the quotes) per spec section 5.4.8.
pub fn decode_ansic(body: &[u8], out: &mut KVec<u8>, budget: &Budget) -> Result<(), crate::mem::MemError> {
    let mut i = 0;
    let mut byte_run = KBytes::new_in(budget);
    while i < body.len() {
        if body[i] != b'\\' {
            flush_byte_run(&mut byte_run, out)?;
            out.try_push(body[i])?;
            i += 1;
            continue;
        }
        let Some(&next) = body.get(i + 1) else {
            flush_byte_run(&mut byte_run, out)?;
            out.try_push(b'\\')?;
            i += 1;
            continue;
        };
        if next == b'x' {
            let rem = &body[i + 2..];
            let inspect_len = rem.len().min(2);
            let inspect = &rem[..inspect_len];
            let hex_len = inspect.iter().take_while(|b| b.is_ascii_hexdigit()).count();
            if hex_len > 0 {
                let mut val: u8 = 0;
                for &d in &inspect[..hex_len] {
                    val = val * 16 + hex_val(d);
                }
                byte_run.try_push(val)?;
                i += 2 + inspect_len;
            } else {
                flush_byte_run(&mut byte_run, out)?;
                out.try_extend_from_slice(b"\\x")?;
                i += 2;
            }
            continue;
        }
        flush_byte_run(&mut byte_run, out)?;
        match next {
            b'a' => { out.try_push(0x07)?; i += 2; }
            b'b' => { out.try_push(0x08)?; i += 2; }
            b'e' | b'E' => { out.try_push(0x1B)?; i += 2; }
            b'f' => { out.try_push(0x0C)?; i += 2; }
            b'n' => { out.try_push(0x0A)?; i += 2; }
            b'r' => { out.try_push(0x0D)?; i += 2; }
            b't' => { out.try_push(0x09)?; i += 2; }
            b'v' => { out.try_push(0x0B)?; i += 2; }
            b'\\' => { out.try_push(b'\\')?; i += 2; }
            b'\'' => { out.try_push(b'\'')?; i += 2; }
            b'"' => { out.try_push(b'"')?; i += 2; }
            b'u' => {
                let rem = &body[i + 2..];
                let inspect_len = rem.len().min(4);
                let inspect = &rem[..inspect_len];
                let hex_len = inspect.iter().take_while(|b| b.is_ascii_hexdigit()).count();
                if hex_len > 0 {
                    let mut val: u32 = 0;
                    for &d in &inspect[..hex_len] {
                        val = val * 16 + hex_val(d) as u32;
                    }
                    let ch = char::from_u32(val).unwrap_or(char::REPLACEMENT_CHARACTER);
                    let mut enc = [0u8; 4];
                    out.try_extend_from_slice(ch.encode_utf8(&mut enc).as_bytes())?;
                    i += 2 + inspect_len;
                } else {
                    out.try_extend_from_slice(b"\\u")?;
                    i += 2;
                }
            }
            b'0'..=b'7' => {
                let rem = &body[i + 1..];
                let oct_len = rem
                    .iter()
                    .take(3)
                    .take_while(|&&d| matches!(d, b'0'..=b'7'))
                    .count();
                let mut val: u32 = 0;
                for &d in &rem[..oct_len] {
                    val = val * 8 + (d - b'0') as u32;
                }
                let ch = char::from_u32(val & 0xFF).unwrap_or('\0');
                if val < 0x80 {
                    out.try_push(val as u8)?;
                } else {
                    let mut enc = [0u8; 4];
                    out.try_extend_from_slice(ch.encode_utf8(&mut enc).as_bytes())?;
                }
                i += 1 + oct_len;
            }
            b'c' => {
                if let Some(&ch) = body.get(i + 2) {
                    out.try_push(ch & 0x1F)?;
                    i += 3;
                } else {
                    // `\c` directly before closing `'` -> `0x27 & 0x1F = 0x07`
                    out.try_push(b'\'' & 0x1F)?;
                    i += 2;
                }
            }
            other => {
                out.try_push(b'\\')?;
                out.try_push(other)?;
                i += 2;
            }
        }
    }
    flush_byte_run(&mut byte_run, out)?;
    Ok(())
}

/// Decodes consecutive `\xHH` bytes as UTF-8 with per-byte Latin-1 recovery (`U+00XX`).
fn flush_byte_run(run: &mut KBytes, out: &mut KVec<u8>) -> Result<(), crate::mem::MemError> {
    if run.is_empty() {
        return Ok(());
    }
    for chunk in run.utf8_chunks() {
        out.try_extend_from_slice(chunk.valid().as_bytes())?;
        for &b in chunk.invalid() {
            let mut enc = [0u8; 4];
            out.try_extend_from_slice((b as char).encode_utf8(&mut enc).as_bytes())?;
        }
    }
    run.clear();
    Ok(())
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

#[allow(dead_code)]
fn _unused_write(_: &mut dyn Write) {}
