//! Conditional command `[[ ... ]]` parser (precedence, unary/binary ops, `=~`, extglob `!(...)`).
use super::ast::{CondBinOp, CondExpr, CondId, CondUnaryOp, WordId};
use super::lexer::TokenKind;
use super::parser::Parser;
use super::word::parse_word_no_brace;
use super::Error;
use crate::mem::KBytes;

impl Parser<'_, '_, '_> {
    pub(super) fn parse_cond_or(&mut self) -> Result<CondId, Error> {
        self.skip_newlines()?;
        let mut left = self.parse_cond_and()?;
        loop {
            self.skip_newlines()?;
            if self.kind() == TokenKind::OrOr {
                self.advance();
                self.skip_newlines()?;
                let right = self.parse_cond_and()?;
                left = self.ctx.ast.push_cond(CondExpr::Or { left, right })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_cond_and(&mut self) -> Result<CondId, Error> {
        let mut left = self.parse_cond_not()?;
        loop {
            self.skip_newlines()?;
            if self.kind() == TokenKind::AndAnd {
                self.advance();
                self.skip_newlines()?;
                let right = self.parse_cond_not()?;
                left = self.ctx.ast.push_cond(CondExpr::And { left, right })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_cond_not(&mut self) -> Result<CondId, Error> {
        let mut negations: usize = 0;
        loop {
            self.skip_newlines()?;
            if self.kind() == TokenKind::Bang {
                self.step()?;
                self.advance();
                negations += 1;
            } else {
                break;
            }
        }
        let mut expr = self.parse_cond_primary()?;
        for _ in 0..negations {
            expr = self.ctx.ast.push_cond(CondExpr::Not(expr))?;
        }
        Ok(expr)
    }

    fn parse_cond_primary(&mut self) -> Result<CondId, Error> {
        self.skip_newlines()?;
        self.step()?;
        match self.kind() {
            TokenKind::DBrackEnd
            | TokenKind::Eof
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::RParen => {
                return Err(self.error_here(format_args!("Expected conditional expression")));
            }
            TokenKind::LParen => {
                self.advance();
                let pos = self.pos();
                self.ctx.enter_depth(pos)?;
                let inner = self.parse_cond_or();
                self.ctx.leave_depth();
                let inner = inner?;
                self.skip_newlines()?;
                self.expect(TokenKind::RParen)?;
                return Ok(self.ctx.ast.push_cond(CondExpr::Group(inner))?);
            }
            _ => {}
        }

        // Check unary operator (only when not quoted and not followed by a binary operator).
        if !self.is_quoted() {
            if let Some(op) = match_unary_op(self.lexeme()) {
                let next_is_bin = self.peek_binary_op(1).is_some();
                if !next_is_bin {
                    let op_text = self.lexeme();
                    let mut op_copy = [0u8; 4];
                    let op_len = op_text.len().min(op_copy.len());
                    op_copy[..op_len].copy_from_slice(&op_text[..op_len]);
                    self.advance();
                    if matches!(
                        self.kind(),
                        TokenKind::DBrackEnd
                            | TokenKind::Eof
                            | TokenKind::AndAnd
                            | TokenKind::OrOr
                            | TokenKind::RParen
                            | TokenKind::Newline
                    ) {
                        let s = core::str::from_utf8(&op_copy[..op_len]).unwrap_or("");
                        return Err(self.error_here(format_args!("Expected operand after {s}")));
                    }
                    let operand = self.consume_cond_word()?;
                    return Ok(self.ctx.ast.push_cond(CondExpr::Unary { op, operand })?);
                }
            }
        }

        // Parse LHS word, then check for binary operator.
        let left = self.consume_cond_word()?;
        if let Some(op) = self.peek_binary_op(0) {
            self.advance();
            if op == CondBinOp::RegexMatch {
                let right = self.consume_regex_word()?;
                return Ok(self
                    .ctx
                    .ast
                    .push_cond(CondExpr::Binary { op, left, right })?);
            }
            // For `==`/`!=`, recombine `BANG` + `LPAREN ... RPAREN` extglob when followed by `]]`/`&&`/`||`/`)`.
            if matches!(op, CondBinOp::StrEq | CondBinOp::StrNe)
                && self.kind() == TokenKind::Bang
                && self.peek_kind(1) == TokenKind::LParen
            {
                if let Some(right) = self.try_consume_extglob_negation()? {
                    return Ok(self
                        .ctx
                        .ast
                        .push_cond(CondExpr::Binary { op, left, right })?);
                }
            }
            // Note: per spec 6.2.3, for `==`/`!=`/`<`/`>`/`-eq` etc., even `]]` is consumed
            // as RHS word so `[[ a == ]]` fails on the following `expect(DBRACK_END)` with EOF!
            if self.kind() == TokenKind::Eof {
                return Err(self.error_here(format_args!("Expected conditional expression")));
            }
            let right = self.consume_cond_word()?;
            return Ok(self
                .ctx
                .ast
                .push_cond(CondExpr::Binary { op, left, right })?);
        }

        Ok(self.ctx.ast.push_cond(CondExpr::Word(left))?)
    }

    fn peek_binary_op(&self, offset: usize) -> Option<CondBinOp> {
        let tok = self.lex.get(self.at + offset)?;
        if tok.quoted {
            return None;
        }
        match tok.kind {
            TokenKind::Less => Some(CondBinOp::StrLt),
            TokenKind::Great => Some(CondBinOp::StrGt),
            TokenKind::Word | TokenKind::Name => match tok.text.get(&self.lex.bytes) {
                b"==" | b"=" => Some(CondBinOp::StrEq),
                b"!=" => Some(CondBinOp::StrNe),
                b"=~" => Some(CondBinOp::RegexMatch),
                b"-eq" => Some(CondBinOp::Eq),
                b"-ne" => Some(CondBinOp::Ne),
                b"-lt" => Some(CondBinOp::Lt),
                b"-le" => Some(CondBinOp::Le),
                b"-gt" => Some(CondBinOp::Gt),
                b"-ge" => Some(CondBinOp::Ge),
                b"-nt" => Some(CondBinOp::Nt),
                b"-ot" => Some(CondBinOp::Ot),
                b"-ef" => Some(CondBinOp::Ef),
                _ => None,
            },
            _ => None,
        }
    }

    fn consume_cond_word(&mut self) -> Result<WordId, Error> {
        let raw = self.lexeme();
        let mut copy = KBytes::with_capacity_in(raw.len(), self.ctx.budget)?;
        copy.try_extend_from_slice(raw)?;
        self.advance();
        let err_pos = self.pos();
        parse_word_no_brace(&copy, err_pos, self.ctx)
    }

    fn try_consume_extglob_negation(&mut self) -> Result<Option<WordId>, Error> {
        // Scan ahead from `!` + `(` to find matching `)`.
        let mut idx = self.at + 2;
        let mut depth = 1u32;
        while idx < self.lex.len() && depth > 0 {
            match self.lex.at(idx).kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                TokenKind::DBrackEnd | TokenKind::Eof => return Ok(None),
                _ => {}
            }
            idx += 1;
        }
        if depth != 0 {
            return Ok(None);
        }
        let after = self.lex.get(idx).map_or(TokenKind::Eof, |t| t.kind);
        if !matches!(
            after,
            TokenKind::DBrackEnd | TokenKind::AndAnd | TokenKind::OrOr | TokenKind::RParen
        ) {
            return Ok(None);
        }
        let mut buf = KBytes::new_in(self.ctx.budget);
        let mut prev_end: Option<u32> = None;
        while self.at < idx {
            let tok = self.lex.at(self.at);
            if let Some(pe) = prev_end {
                if tok.src_start > pe {
                    buf.try_push(b' ')?;
                }
            }
            buf.try_extend_from_slice(tok.text.get(&self.lex.bytes))?;
            prev_end = Some(tok.src_end);
            self.advance();
        }
        let err_pos = self.pos();
        Ok(Some(parse_word_no_brace(&buf, err_pos, self.ctx)?))
    }

    fn consume_regex_word(&mut self) -> Result<WordId, Error> {
        if matches!(
            self.kind(),
            TokenKind::DBrackEnd
                | TokenKind::Eof
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::RParen
                | TokenKind::Newline
        ) {
            return Err(self.error_here(format_args!("Expected regex pattern after =~")));
        }
        let mut buf = KBytes::new_in(self.ctx.budget);
        let mut paren_depth = 0u32;
        let mut prev_end: Option<u32> = None;

        loop {
            let kind = self.kind();
            if kind == TokenKind::Eof || kind == TokenKind::DBrackEnd || kind == TokenKind::Newline
            {
                break;
            }
            if paren_depth == 0
                && matches!(
                    kind,
                    TokenKind::AndAnd | TokenKind::OrOr | TokenKind::RParen
                )
            {
                break;
            }
            let tok = self.lex.at(self.at);
            if let Some(pe) = prev_end {
                if paren_depth == 0 && tok.src_start > pe {
                    // Whitespace separates another word (e.g. `[[ a =~ b c ]]`): stop!
                    break;
                }
                if tok.src_start > pe {
                    buf.try_push(b' ')?;
                }
            }
            if kind == TokenKind::LParen {
                paren_depth += 1;
            } else if kind == TokenKind::RParen && paren_depth > 0 {
                paren_depth -= 1;
            }
            buf.try_extend_from_slice(tok.text.get(&self.lex.bytes))?;
            prev_end = Some(tok.src_end);
            self.advance();
        }
        let err_pos = self.pos();
        parse_word_no_brace(&buf, err_pos, self.ctx)
    }
}

fn match_unary_op(lexeme: &[u8]) -> Option<CondUnaryOp> {
    match lexeme {
        b"-a" => Some(CondUnaryOp::ExistsA),
        b"-b" => Some(CondUnaryOp::BlockB),
        b"-c" => Some(CondUnaryOp::CharC),
        b"-d" => Some(CondUnaryOp::DirD),
        b"-e" => Some(CondUnaryOp::ExistsE),
        b"-f" => Some(CondUnaryOp::FileF),
        b"-g" => Some(CondUnaryOp::SetgidG),
        b"-h" => Some(CondUnaryOp::SymlinkH),
        b"-k" => Some(CondUnaryOp::StickyK),
        b"-L" => Some(CondUnaryOp::SymlinkL),
        b"-n" => Some(CondUnaryOp::NonEmptyN),
        b"-o" => Some(CondUnaryOp::OptionO),
        b"-p" => Some(CondUnaryOp::PipeP),
        b"-r" => Some(CondUnaryOp::ReadableR),
        b"-s" => Some(CondUnaryOp::NonZeroSizeS),
        b"-t" => Some(CondUnaryOp::TerminalT),
        b"-u" => Some(CondUnaryOp::SetuidU),
        b"-v" => Some(CondUnaryOp::VarSetV),
        b"-w" => Some(CondUnaryOp::WritableW),
        b"-x" => Some(CondUnaryOp::ExecutableX),
        b"-z" => Some(CondUnaryOp::EmptyZ),
        b"-N" => Some(CondUnaryOp::ModifiedN),
        b"-O" => Some(CondUnaryOp::OwnUidO),
        b"-G" => Some(CondUnaryOp::OwnGidG),
        b"-S" => Some(CondUnaryOp::SocketS),
        _ => None,
    }
}
