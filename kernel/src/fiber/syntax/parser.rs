//! Explicit-budget recursive-descent bash parser covering all 12 command kinds,
//! all 12 redirection operators, deferred root `}`/`)` errors, and shared sub-parser caps.
use super::arith::parse_arith;
use super::ast::{
    Assign, Ast, CaseItem, CaseTerm, CmdId, Command, IfClause, ListOp, Pipeline, Pos, Redir,
    RedirOp, RedirTarget, SimpleCmd, Span, Stmt, StmtId, WordId,
};
use super::lexer::{lex, LexOutput, TokenKind};
use super::word::{parse_array_elements, parse_heredoc_word, parse_word, parse_word_no_brace};
use super::{parser_err, Error, DEPTH_CAP, STEP_CAP, TOKEN_CAP, TOP_STMT_CAP};
use crate::mem::{Budget, KBytes, KVec};
use crate::util::parse_u64;

/// Shared parser state across the root script and every nested sub-parser.
pub struct ParseCtx<'a, 'w> {
    pub ast: Ast,
    pub budget: &'a Budget,
    pub work: &'w mut dyn FnMut(u64) -> Result<(), ()>,
    pub max_heredoc_size: u64,
    pub cumulative_tokens: usize,
    pub depth: u32,
    pub iterations: usize,
}

impl<'a, 'w> ParseCtx<'a, 'w> {
    pub fn step(&mut self, pos: Pos) -> Result<(), Error> {
        (self.work)(1).map_err(|()| Error::Work)?;
        self.iterations += 1;
        if self.iterations > STEP_CAP {
            return Err(parser_err(
                pos,
                self.budget,
                format_args!("Maximum parse iterations exceeded (possible infinite loop)"),
            ));
        }
        Ok(())
    }

    pub fn enter_depth(&mut self, pos: Pos) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > DEPTH_CAP {
            self.depth -= 1;
            return Err(parser_err(
                pos,
                self.budget,
                format_args!("Maximum parser nesting depth exceeded ({DEPTH_CAP})"),
            ));
        }
        Ok(())
    }

    pub fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

pub fn parse_script(
    source: &[u8],
    max_heredoc_size: u64,
    budget: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Ast, Error> {
    let lex_out = lex(source, max_heredoc_size, budget, work)?;
    let token_count = lex_out.len() + 1; // include synthetic EOF token per spec §6.1.2
    let mut ctx = ParseCtx {
        ast: Ast::new_in(budget),
        budget,
        work,
        max_heredoc_size,
        cumulative_tokens: token_count,
        depth: 0,
        iterations: 0,
    };
    let root = {
        let mut p = Parser::new(&lex_out, &mut ctx);
        p.parse_root_list()?
    };
    ctx.ast.root = root;
    Ok(ctx.ast)
}

/// Parses an embedded script (`$(...)`, `` `...` ``, `<(...)`, `>(...)`) into the shared `Ast`,
/// enforcing the shared cumulative token cap (`100_000`) and nesting depth cap (`200`).
pub fn parse_sub_script(
    source: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<Span, Error> {
    ctx.enter_depth(err_pos)?;
    let res = (|| {
        let lex_out = lex(source, ctx.max_heredoc_size, ctx.budget, ctx.work)?;
        ctx.cumulative_tokens = ctx.cumulative_tokens.saturating_add(lex_out.len() + 1);
        if ctx.cumulative_tokens > TOKEN_CAP {
            return Err(parser_err(
                Pos { line: 1, col: 1 },
                ctx.budget,
                format_args!("Too many tokens: cumulative count exceeds limit of {TOKEN_CAP}"),
            ));
        }
        let mut p = Parser::new(&lex_out, ctx);
        p.parse_root_list()
    })();
    ctx.leave_depth();
    res
}

struct PendingHeredocRedir {
    redir_idx: usize,
    strip_tabs: bool,
    quoted: bool,
}

pub(super) struct Parser<'a, 'b, 'w> {
    pub(super) lex: &'a LexOutput,
    pub(super) ctx: &'a mut ParseCtx<'b, 'w>,
    pub(super) at: usize,
    pending_heredocs: KVec<PendingHeredocRedir>,
}

impl<'a, 'b, 'w> Parser<'a, 'b, 'w> {
    fn new(lex: &'a LexOutput, ctx: &'a mut ParseCtx<'b, 'w>) -> Self {
        let budget = ctx.budget;
        Self {
            lex,
            ctx,
            at: 0,
            pending_heredocs: KVec::new_in(budget),
        }
    }

    pub(super) fn kind(&self) -> TokenKind {
        self.lex
            .get(self.at)
            .map_or(TokenKind::Eof, |t| t.kind)
    }

    pub(super) fn peek_kind(&self, offset: usize) -> TokenKind {
        self.lex
            .get(self.at + offset)
            .map_or(TokenKind::Eof, |t| t.kind)
    }

    pub(super) fn is_quoted(&self) -> bool {
        self.lex.get(self.at).is_some_and(|t| t.quoted)
    }

    pub(super) fn lexeme(&self) -> &'a [u8] {
        self.lex
            .get(self.at)
            .map_or(b"", |t| t.text.get(&self.lex.bytes))
    }

    pub(super) fn pos(&self) -> Pos {
        self.lex
            .get(self.at)
            .map_or(self.lex.eof_pos, |t| t.pos)
    }

    fn src_start(&self) -> u32 {
        self.lex
            .get(self.at)
            .map_or(self.lex.src_len, |t| t.src_start)
    }

    fn prev_src_end(&self) -> u32 {
        self.at
            .checked_sub(1)
            .and_then(|i| self.lex.get(i))
            .map_or(0, |t| t.src_end)
    }

    pub(super) fn advance(&mut self) {
        if self.at < self.lex.len() {
            self.at += 1;
        }
    }

    pub(super) fn step(&mut self) -> Result<(), Error> {
        let pos = self.pos();
        self.ctx.step(pos)
    }

    pub(super) fn error_here(&self, args: core::fmt::Arguments<'_>) -> Error {
        parser_err(self.pos(), self.ctx.budget, args)
    }

    fn unexpected_token(&self) -> Error {
        let lex = self.lexeme();
        let display = if lex.is_empty() {
            self.kind().display_name()
        } else {
            core::str::from_utf8(lex).unwrap_or("?")
        };
        self.error_here(format_args!("syntax error near unexpected token `{display}'"))
    }

    pub(super) fn expect(&mut self, expected: TokenKind) -> Result<(), Error> {
        if self.kind() == expected {
            self.advance();
            Ok(())
        } else {
            Err(self.error_here(format_args!(
                "Expected {}, got {}",
                expected.display_name(),
                self.kind().display_name()
            )))
        }
    }

    pub(super) fn skip_newlines(&mut self) -> Result<(), Error> {
        while self.kind() == TokenKind::Newline {
            self.advance();
            self.attach_pending_heredocs()?;
        }
        Ok(())
    }

    fn attach_pending_heredocs(&mut self) -> Result<(), Error> {
        if self.pending_heredocs.is_empty() {
            return Ok(());
        }
        let pending = core::mem::replace(
            &mut self.pending_heredocs,
            KVec::new_in(self.ctx.budget),
        );
        for item in pending.iter() {
            let content = if self.kind() == TokenKind::HeredocContent {
                let raw = self.lex.at(self.at).text.get(&self.lex.bytes);
                self.advance();
                let err_pos = self.pos();
                parse_heredoc_word(raw, item.quoted, err_pos, self.ctx)?
            } else {
                self.ctx.ast.push_literal_word(b"")?
            };
            self.ctx.ast.redirs.as_mut_slice()[item.redir_idx].target = RedirTarget::HereDoc {
                content,
                strip_tabs: item.strip_tabs,
                quoted: item.quoted,
            };
        }
        Ok(())
    }

    fn parse_root_list(&mut self) -> Result<Span, Error> {
        let mut first_stmt: Option<StmtId> = None;
        let mut stmts = KVec::new_in(self.ctx.budget);
        let mut top_count: usize = 0;

        loop {
            self.skip_newlines()?;
            self.attach_pending_heredocs()?;
            if self.kind() == TokenKind::Eof {
                break;
            }
            self.step()?;
            top_count += 1;
            if top_count > TOP_STMT_CAP {
                return Err(self.error_here(format_args!(
                    "Parser stuck: too many iterations (>{TOP_STMT_CAP})"
                )));
            }

            // Deferred errors at root list for stray `}` or `)`.
            let stmt_id = if matches!(self.kind(), TokenKind::RBrace | TokenKind::RParen) {
                let tok_str = if self.kind() == TokenKind::RBrace {
                    b"syntax error near unexpected token `}'".as_slice()
                } else {
                    b"syntax error near unexpected token `)'".as_slice()
                };
                let src_start = self.src_start();
                self.advance();
                let msg = self.ctx.ast.push_bytes(tok_str)?;
                self.ctx.ast.push_stmt(Stmt {
                    pipelines: Span::empty(),
                    ops: Span::empty(),
                    background: false,
                    deferred_error: Some(msg),
                    source_span: Span::new(src_start as usize, self.prev_src_end() as usize),
                    line: 1,
                })?
            } else {
                let id = self.parse_statement()?;
                match self.kind() {
                    TokenKind::Semicolon => {
                        self.advance();
                    }
                    TokenKind::Newline => {
                        self.advance();
                        self.attach_pending_heredocs()?;
                    }
                    TokenKind::Eof => {}
                    TokenKind::DSemi | TokenKind::SemiAnd | TokenKind::SemiSemiAnd => {
                        return Err(self.unexpected_token());
                    }
                    _ => {}
                }
                id
            };

            if stmts.is_empty() {
                if let Some(first) = first_stmt.take() {
                    stmts.try_push(first)?;
                    stmts.try_push(stmt_id)?;
                } else {
                    first_stmt = Some(stmt_id);
                }
            } else {
                stmts.try_push(stmt_id)?;
            }
        }

        let start = self.ctx.ast.stmt_lists.len();
        if let Some(first) = first_stmt {
            self.ctx.ast.stmt_lists.try_push(first)?;
        } else if !stmts.is_empty() {
            self.ctx.ast.stmt_lists.try_extend_from_slice(&stmts)?;
        }
        Ok(Span::new(start, self.ctx.ast.stmt_lists.len()))
    }

    fn parse_compound_list(&mut self) -> Result<Span, Error> {
        let mut stmts = KVec::new_in(self.ctx.budget);

        loop {
            self.skip_newlines()?;
            self.attach_pending_heredocs()?;
            if self.is_compound_list_end()
                || (stmts.is_empty() && self.kind() == TokenKind::Semicolon)
            {
                break;
            }
            self.step()?;
            let stmt_id = self.parse_statement()?;
            stmts.try_push(stmt_id)?;

            match self.kind() {
                TokenKind::Semicolon => {
                    self.advance();
                }
                TokenKind::Newline => {
                    self.advance();
                    self.attach_pending_heredocs()?;
                }
                _ if self.is_compound_list_end() => break,
                TokenKind::DSemi | TokenKind::SemiAnd | TokenKind::SemiSemiAnd => break,
                _ => {}
            }
        }

        let start = self.ctx.ast.stmt_lists.len();
        self.ctx.ast.stmt_lists.try_extend_from_slice(&stmts)?;
        Ok(Span::new(start, self.ctx.ast.stmt_lists.len()))
    }

    fn is_compound_list_end(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Eof
                | TokenKind::Then
                | TokenKind::Else
                | TokenKind::Elif
                | TokenKind::Fi
                | TokenKind::Do
                | TokenKind::Done
                | TokenKind::Esac
                | TokenKind::RBrace
                | TokenKind::RParen
                | TokenKind::DSemi
                | TokenKind::SemiAnd
                | TokenKind::SemiSemiAnd
        )
    }

    fn parse_statement(&mut self) -> Result<StmtId, Error> {
        // Reject unexpected statement-start tokens immediately.
        if matches!(
            self.kind(),
            TokenKind::Do
                | TokenKind::Done
                | TokenKind::Then
                | TokenKind::Else
                | TokenKind::Elif
                | TokenKind::Fi
                | TokenKind::Esac
                | TokenKind::DSemi
                | TokenKind::SemiAnd
                | TokenKind::SemiSemiAnd
                | TokenKind::Semicolon
                | TokenKind::Amp
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::Pipe
                | TokenKind::PipeAmp
                | TokenKind::RBrace
                | TokenKind::RParen
        ) {
            return Err(self.unexpected_token());
        }

        let start_pos = self.pos();
        let src_start = self.src_start();
        let first_pipe = self.parse_pipeline()?;
        let mut pipes = KVec::new_in(self.ctx.budget);
        let mut ops = KVec::new_in(self.ctx.budget);

        while matches!(self.kind(), TokenKind::AndAnd | TokenKind::OrOr) {
            if pipes.is_empty() {
                pipes.try_push(first_pipe)?;
            }
            let op = if self.kind() == TokenKind::AndAnd {
                ListOp::And
            } else {
                ListOp::Or
            };
            self.advance();
            self.skip_newlines()?;
            ops.try_push(op)?;
            // Trailing `&&` / `||` followed by EOF or compound terminator -> empty no-op SimpleCmd.
            if self.is_empty_rhs_end() {
                pipes.try_push(self.make_empty_pipeline(false, false, false)?)?;
                break;
            }
            pipes.try_push(self.parse_pipeline()?)?;
        }

        let background = if self.kind() == TokenKind::Amp {
            self.advance();
            true
        } else {
            false
        };

        let p_start = self.ctx.ast.pipelines.len();
        if pipes.is_empty() {
            self.ctx.ast.pipelines.try_push(first_pipe)?;
        } else {
            self.ctx.ast.pipelines.try_extend_from_slice(&pipes)?;
        }
        let pipelines = Span::new(p_start, self.ctx.ast.pipelines.len());

        let o_start = self.ctx.ast.list_ops.len();
        self.ctx.ast.list_ops.try_extend_from_slice(&ops)?;
        let ops_span = Span::new(o_start, self.ctx.ast.list_ops.len());

        let src_end = self.prev_src_end().max(src_start);
        Ok(self.ctx.ast.push_stmt(Stmt {
            pipelines,
            ops: ops_span,
            background,
            deferred_error: None,
            source_span: Span::new(src_start as usize, src_end as usize),
            line: start_pos.line,
        })?)
    }

    fn is_empty_rhs_end(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::Semicolon
                | TokenKind::Amp
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::Pipe
                | TokenKind::PipeAmp
                | TokenKind::RParen
                | TokenKind::RBrace
                | TokenKind::Then
                | TokenKind::Else
                | TokenKind::Elif
                | TokenKind::Fi
                | TokenKind::Do
                | TokenKind::Done
                | TokenKind::Esac
                | TokenKind::DSemi
                | TokenKind::SemiAnd
                | TokenKind::SemiSemiAnd
        )
    }

    fn make_empty_pipeline(
        &mut self,
        negated: bool,
        timed: bool,
        time_posix: bool,
    ) -> Result<Pipeline, Error> {
        let cmd = self.ctx.ast.push_cmd(Command::Simple(SimpleCmd {
            assigns: Span::empty(),
            name: None,
            args: Span::empty(),
            redirs: Span::empty(),
            line: self.pos().line,
        }))?;
        let c_start = self.ctx.ast.cmd_lists.len();
        self.ctx.ast.cmd_lists.try_push(cmd)?;
        Ok(Pipeline {
            commands: Span::new(c_start, self.ctx.ast.cmd_lists.len()),
            pipe_stderr: Span::empty(),
            negated,
            timed,
            time_posix,
        })
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline, Error> {
        let mut negated = false;
        let mut leading_bangs = 0u32;
        while self.kind() == TokenKind::Bang {
            self.advance();
            negated = !negated;
            leading_bangs += 1;
        }

        let mut timed = false;
        let mut time_posix = false;
        if leading_bangs == 0 && self.kind() == TokenKind::Time {
            timed = true;
            self.advance();
            if matches!(self.kind(), TokenKind::Word | TokenKind::Name)
                && !self.is_quoted()
                && self.lexeme() == b"-p"
            {
                time_posix = true;
                self.advance();
            }
            while self.kind() == TokenKind::Bang {
                self.advance();
                negated = !negated;
            }
        }

        // Bare `!` or `time` with no following command -> empty no-op SimpleCmd.
        if (leading_bangs > 0 || timed) && self.is_empty_rhs_end() {
            return self.make_empty_pipeline(negated, timed, time_posix);
        }

        let first_cmd = self.parse_command()?;
        let mut cmds = KVec::new_in(self.ctx.budget);
        let mut stderr_flags = KVec::new_in(self.ctx.budget);

        while matches!(self.kind(), TokenKind::Pipe | TokenKind::PipeAmp) {
            if cmds.is_empty() {
                cmds.try_push(first_cmd)?;
            }
            let is_both = self.kind() == TokenKind::PipeAmp;
            self.advance();
            self.skip_newlines()?;
            stderr_flags.try_push(is_both)?;
            if self.is_empty_rhs_end() {
                let empty = self.ctx.ast.push_cmd(Command::Simple(SimpleCmd {
                    assigns: Span::empty(),
                    name: None,
                    args: Span::empty(),
                    redirs: Span::empty(),
                    line: self.pos().line,
                }))?;
                cmds.try_push(empty)?;
                break;
            }
            cmds.try_push(self.parse_command()?)?;
        }

        let c_start = self.ctx.ast.cmd_lists.len();
        if cmds.is_empty() {
            self.ctx.ast.cmd_lists.try_push(first_cmd)?;
        } else {
            self.ctx.ast.cmd_lists.try_extend_from_slice(&cmds)?;
        }
        let p_start = self.ctx.ast.pipe_stderr.len();
        if !stderr_flags.is_empty() {
            self.ctx.ast.pipe_stderr.try_extend_from_slice(&stderr_flags)?;
        }

        Ok(Pipeline {
            commands: Span::new(c_start, self.ctx.ast.cmd_lists.len()),
            pipe_stderr: Span::new(p_start, self.ctx.ast.pipe_stderr.len()),
            negated,
            timed,
            time_posix,
        })
    }

    fn parse_command(&mut self) -> Result<CmdId, Error> {
        self.step()?;
        match self.kind() {
            TokenKind::If => self.parse_if(),
            TokenKind::For => self.parse_for(),
            TokenKind::While => self.parse_while_until(false),
            TokenKind::Until => self.parse_while_until(true),
            TokenKind::Case => self.parse_case(),
            TokenKind::LParen => self.parse_subshell(),
            TokenKind::LBrace => self.parse_group(),
            TokenKind::DParenStart => self.parse_arith_cmd(),
            TokenKind::DBrackStart => self.parse_cond_cmd(),
            TokenKind::Function => self.parse_function_keyword(),
            TokenKind::Name | TokenKind::Word
                if self.peek_kind(1) == TokenKind::LParen
                    && self.peek_kind(2) == TokenKind::RParen
                    && self.is_valid_fn_name() =>
            {
                self.parse_function_parens()
            }
            _ => self.parse_simple_command(),
        }
    }

    fn is_valid_fn_name(&self) -> bool {
        if self.is_quoted() || self.kind() == TokenKind::Number {
            return false;
        }
        let name = self.lexeme();
        !name.is_empty() && !name.contains(&b'=') && !name.iter().all(|b| b.is_ascii_digit())
    }

    fn enter_compound(&mut self) -> Result<(), Error> {
        self.ctx.depth += 1;
        if self.ctx.depth > DEPTH_CAP {
            return Err(self.error_here(format_args!(
                "Maximum parser nesting depth exceeded ({DEPTH_CAP})"
            )));
        }
        Ok(())
    }

    fn parse_if(&mut self) -> Result<CmdId, Error> {
        self.advance(); // consume `if`
        self.enter_compound()?;

        let mut clauses = KVec::new_in(self.ctx.budget);
        let cond = self.parse_compound_list()?;
        self.expect(TokenKind::Then)?;
        let body = self.parse_compound_list()?;
        if body.is_empty() {
            let tok = match self.kind() {
                TokenKind::Elif => "elif",
                TokenKind::Else => "else",
                _ => "fi",
            };
            self.ctx.depth -= 1;
            return Err(
                self.error_here(format_args!("syntax error near unexpected token `{tok}'"))
            );
        }
        clauses.try_push(IfClause { cond, body })?;

        while self.kind() == TokenKind::Elif {
            self.advance();
            let cond = self.parse_compound_list()?;
            self.expect(TokenKind::Then)?;
            let body = self.parse_compound_list()?;
            if body.is_empty() {
                let tok = match self.kind() {
                    TokenKind::Elif => "elif",
                    TokenKind::Else => "else",
                    _ => "fi",
                };
                self.ctx.depth -= 1;
                return Err(
                    self.error_here(format_args!("syntax error near unexpected token `{tok}'"))
                );
            }
            clauses.try_push(IfClause { cond, body })?;
        }

        let else_body = if self.kind() == TokenKind::Else {
            self.advance();
            let eb = self.parse_compound_list()?;
            if eb.is_empty() {
                self.ctx.depth -= 1;
                return Err(self.error_here(format_args!("syntax error near unexpected token `fi'")));
            }
            Some(eb)
        } else {
            None
        };

        self.expect(TokenKind::Fi)?;
        self.ctx.depth -= 1;

        let redirs = self.parse_trailing_redirs()?;
        let c_start = self.ctx.ast.if_clauses.len();
        self.ctx.ast.if_clauses.try_extend_from_slice(&clauses)?;
        Ok(self.ctx.ast.push_cmd(Command::If {
            clauses: Span::new(c_start, self.ctx.ast.if_clauses.len()),
            else_body,
            redirs,
        })?)
    }

    fn parse_for(&mut self) -> Result<CmdId, Error> {
        let start_line = self.pos().line;
        self.advance(); // consume `for`
        self.enter_compound()?;

        // C-style `for (( init; cond; update ))`
        if self.kind() == TokenKind::DParenStart {
            let cmd = self.parse_c_style_for(start_line);
            self.ctx.depth -= 1;
            return cmd;
        }

        if !matches!(
            self.kind(),
            TokenKind::Name
                | TokenKind::Word
                | TokenKind::Number
                | TokenKind::In
                | TokenKind::Do
                | TokenKind::Done
        ) {
            self.ctx.depth -= 1;
            return Err(self.error_here(format_args!(
                "Expected NAME, got {}",
                self.kind().display_name()
            )));
        }
        let var = self.ctx.ast.push_bytes(self.lexeme())?;
        self.advance();

        self.skip_newlines()?;
        let words = if self.kind() == TokenKind::In {
            self.advance();
            let mut list = KVec::new_in(self.ctx.budget);
            while self.is_word_like_in_list() {
                let w = self.consume_word(true)?;
                list.try_push(w)?;
            }
            if matches!(self.kind(), TokenKind::Semicolon | TokenKind::Newline) {
                self.advance();
                self.attach_pending_heredocs()?;
            }
            let start = self.ctx.ast.word_lists.len();
            self.ctx.ast.word_lists.try_extend_from_slice(&list)?;
            Some(Span::new(start, self.ctx.ast.word_lists.len()))
        } else {
            if self.kind() == TokenKind::Semicolon {
                self.advance();
            }
            None
        };

        self.skip_newlines()?;
        self.expect(TokenKind::Do)?;
        let body = self.parse_compound_list()?;
        if body.is_empty() {
            self.ctx.depth -= 1;
            return Err(self.error_here(format_args!("syntax error near unexpected token `done'")));
        }
        self.expect(TokenKind::Done)?;
        self.ctx.depth -= 1;

        let redirs = self.parse_trailing_redirs()?;
        Ok(self.ctx.ast.push_cmd(Command::For {
            var,
            words,
            body,
            redirs,
        })?)
    }

    fn parse_c_style_for(&mut self, line: u32) -> Result<CmdId, Error> {
        self.advance(); // consume `((`
        let init = self.collect_c_for_clause()?;
        let cond = if self.kind() == TokenKind::Semicolon {
            self.advance();
            self.collect_c_for_clause()?
        } else {
            None
        };
        let update = if self.kind() == TokenKind::Semicolon {
            self.advance();
            let u = self.collect_c_for_clause()?;
            if self.kind() == TokenKind::Semicolon {
                self.advance();
            }
            u
        } else {
            None
        };
        self.expect(TokenKind::DPAREN_END_ALIAS)?;

        while matches!(self.kind(), TokenKind::Semicolon | TokenKind::Newline) {
            self.advance();
            self.attach_pending_heredocs()?;
        }

        let body = if self.kind() == TokenKind::LBrace {
            self.advance();
            let b = self.parse_compound_list()?;
            self.expect(TokenKind::RBrace)?;
            b
        } else {
            self.expect(TokenKind::Do)?;
            let b = self.parse_compound_list()?;
            if b.is_empty() {
                return Err(
                    self.error_here(format_args!("syntax error near unexpected token `done'"))
                );
            }
            self.expect(TokenKind::Done)?;
            b
        };

        let redirs = self.parse_trailing_redirs()?;
        Ok(self.ctx.ast.push_cmd(Command::CStyleFor {
            init,
            cond,
            update,
            body,
            redirs,
            line,
        })?)
    }

    fn collect_c_for_clause(&mut self) -> Result<Option<super::ast::ArithId>, Error> {
        let mut buf = KBytes::new_in(self.ctx.budget);
        let mut prev_end: Option<u32> = None;
        while !matches!(
            self.kind(),
            TokenKind::Semicolon | TokenKind::DParenEnd | TokenKind::Eof
        ) {
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
        if buf.is_empty() {
            Ok(None)
        } else {
            let err_pos = self.pos();
            Ok(Some(parse_arith(&buf, err_pos, self.ctx)?))
        }
    }

    fn parse_while_until(&mut self, until: bool) -> Result<CmdId, Error> {
        self.advance(); // consume `while` / `until`
        self.enter_compound()?;

        let cond = self.parse_compound_list()?;
        self.expect(TokenKind::Do)?;
        let body = self.parse_compound_list()?;
        if body.is_empty() {
            self.ctx.depth -= 1;
            return Err(self.error_here(format_args!("syntax error near unexpected token `done'")));
        }
        self.expect(TokenKind::Done)?;
        self.ctx.depth -= 1;

        let redirs = self.parse_trailing_redirs()?;
        Ok(self.ctx.ast.push_cmd(if until {
            Command::Until { cond, body, redirs }
        } else {
            Command::While { cond, body, redirs }
        })?)
    }

    fn parse_case(&mut self) -> Result<CmdId, Error> {
        self.advance(); // consume `case`
        self.enter_compound()?;

        if !self.is_word_like_in_list() {
            self.ctx.depth -= 1;
            return Err(self.error_here(format_args!(
                "Expected WORD, got {}",
                self.kind().display_name()
            )));
        }
        let word = self.consume_word(false)?;
        self.skip_newlines()?;
        self.expect(TokenKind::In)?;

        let mut items = KVec::new_in(self.ctx.budget);
        loop {
            self.skip_newlines()?;
            if self.kind() == TokenKind::Esac || self.kind() == TokenKind::Eof {
                break;
            }
            self.step()?;
            let had_lparen = if self.kind() == TokenKind::LParen {
                self.advance();
                true
            } else {
                false
            };
            let mut patterns = KVec::new_in(self.ctx.budget);
            if !had_lparen && self.kind() == TokenKind::Esac {
                break;
            }
            patterns.try_push(self.consume_case_pattern()?)?;
            while self.kind() == TokenKind::Pipe {
                self.advance();
                patterns.try_push(self.consume_case_pattern()?)?;
            }
            self.expect(TokenKind::RParen)?;
            self.skip_newlines()?;
            let body = self.parse_compound_list()?;
            let terminator = match self.kind() {
                TokenKind::DSemi => {
                    self.advance();
                    CaseTerm::DoubleSemi
                }
                TokenKind::SemiAnd => {
                    self.advance();
                    CaseTerm::SemiAnd
                }
                TokenKind::SemiSemiAnd => {
                    self.advance();
                    CaseTerm::SemiSemiAnd
                }
                _ => CaseTerm::DoubleSemi,
            };
            let p_start = self.ctx.ast.word_lists.len();
            self.ctx.ast.word_lists.try_extend_from_slice(&patterns)?;
            items.try_push(CaseItem {
                patterns: Span::new(p_start, self.ctx.ast.word_lists.len()),
                body,
                terminator,
            })?;
        }
        self.expect(TokenKind::Esac)?;
        self.ctx.depth -= 1;

        let redirs = self.parse_trailing_redirs()?;
        let i_start = self.ctx.ast.case_items.len();
        self.ctx.ast.case_items.try_extend_from_slice(&items)?;
        Ok(self.ctx.ast.push_cmd(Command::Case {
            word,
            items: Span::new(i_start, self.ctx.ast.case_items.len()),
            redirs,
        })?)
    }

    fn consume_case_pattern(&mut self) -> Result<WordId, Error> {
        if matches!(
            self.kind(),
            TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::RParen
                | TokenKind::Pipe
                | TokenKind::DSemi
                | TokenKind::SemiAnd
                | TokenKind::SemiSemiAnd
        ) {
            return Err(self.unexpected_token());
        }
        self.consume_word(false)
    }

    fn parse_subshell(&mut self) -> Result<CmdId, Error> {
        self.advance(); // consume `(`
        self.enter_compound()?;
        let body = self.parse_compound_list()?;
        self.expect(TokenKind::RParen)?;
        self.ctx.depth -= 1;
        let redirs = self.parse_trailing_redirs()?;
        Ok(self.ctx.ast.push_cmd(Command::Subshell { body, redirs })?)
    }

    fn parse_group(&mut self) -> Result<CmdId, Error> {
        self.advance(); // consume `{`
        self.enter_compound()?;
        let body = self.parse_compound_list()?;
        self.expect(TokenKind::RBrace)?;
        self.ctx.depth -= 1;
        let redirs = self.parse_trailing_redirs()?;
        Ok(self.ctx.ast.push_cmd(Command::Group { body, redirs })?)
    }

    fn parse_function_keyword(&mut self) -> Result<CmdId, Error> {
        self.advance(); // consume `function`
        if !matches!(self.kind(), TokenKind::Name | TokenKind::Word) || !self.is_valid_fn_name() {
            return Err(self.error_here(format_args!("Expected function name")));
        }
        let name = self.ctx.ast.push_bytes(self.lexeme())?;
        self.advance();
        if self.kind() == TokenKind::LParen {
            self.advance();
            self.expect(TokenKind::RParen)?;
        }
        self.skip_newlines()?;
        let body = self.parse_function_body()?;
        let redirs = self.parse_trailing_redirs()?;
        Ok(self
            .ctx
            .ast
            .push_cmd(Command::FunctionDef { name, body, redirs })?)
    }

    fn parse_function_parens(&mut self) -> Result<CmdId, Error> {
        let name = self.ctx.ast.push_bytes(self.lexeme())?;
        self.advance(); // name
        self.advance(); // `(`
        self.advance(); // `)`
        self.skip_newlines()?;
        let body = self.parse_function_body()?;
        let redirs = self.parse_trailing_redirs()?;
        Ok(self
            .ctx
            .ast
            .push_cmd(Command::FunctionDef { name, body, redirs })?)
    }

    fn parse_function_body(&mut self) -> Result<CmdId, Error> {
        match self.kind() {
            TokenKind::LBrace => self.parse_group(),
            TokenKind::LParen => self.parse_subshell(),
            TokenKind::If => self.parse_if(),
            TokenKind::For => self.parse_for(),
            TokenKind::While => self.parse_while_until(false),
            TokenKind::Until => self.parse_while_until(true),
            TokenKind::Case => self.parse_case(),
            TokenKind::DParenStart => self.parse_arith_cmd(),
            TokenKind::DBrackStart => self.parse_cond_cmd(),
            _ => Err(self.error_here(format_args!("Expected compound command for function body"))),
        }
    }

    fn parse_arith_cmd(&mut self) -> Result<CmdId, Error> {
        let line = self.pos().line;
        self.advance(); // consume `((`
        let mut buf = KBytes::new_in(self.ctx.budget);
        let mut prev_end: Option<u32> = None;
        while !matches!(self.kind(), TokenKind::DParenEnd | TokenKind::Eof) {
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
        self.expect(TokenKind::DParenEnd)?;
        let err_pos = self.pos();
        let expr = parse_arith(&buf, err_pos, self.ctx)?;
        let redirs = self.parse_trailing_redirs()?;
        Ok(self
            .ctx
            .ast
            .push_cmd(Command::Arithmetic { expr, redirs, line })?)
    }

    fn parse_cond_cmd(&mut self) -> Result<CmdId, Error> {
        let line = self.pos().line;
        self.advance(); // consume `[[`
        let expr = self.parse_cond_or()?;
        self.skip_newlines()?;
        self.expect(TokenKind::DBrackEnd)?;
        let redirs = self.parse_trailing_redirs()?;
        Ok(self
            .ctx
            .ast
            .push_cmd(Command::Conditional { expr, redirs, line })?)
    }

    fn parse_simple_command(&mut self) -> Result<CmdId, Error> {
        let line = self.pos().line;
        let mut assigns = KVec::new_in(self.ctx.budget);
        let mut args = KVec::new_in(self.ctx.budget);
        let mut redirs = KVec::new_in(self.ctx.budget);
        let mut name: Option<WordId> = None;

        loop {
            // Check redirection prefix: `[NUMBER | FD_VARIABLE] redir_op` or bare `redir_op`
            if self.is_redir_start() {
                let redir = self.parse_redirection_entry(&mut redirs)?;
                let _ = redir;
                continue;
            }

            // Before command name: accept `ASSIGNMENT_WORD`
            if name.is_none() && self.kind() == TokenKind::AssignmentWord {
                let assign = self.parse_assignment()?;
                assigns.try_push(assign)?;
                continue;
            }

            // Reject standalone unquoted `(` among arguments (e.g. `echo (a)` or `123() { ... }`).
            if self.kind() == TokenKind::LParen {
                return Err(self.unexpected_token());
            }

            // Accept words (and after command name, reserved words / `{` / `}` / `!` / `ASSIGNMENT_WORD`)
            if self.is_simple_word(name.is_some()) {
                let w = self.consume_word(true)?;
                if name.is_none() {
                    name = Some(w);
                } else {
                    args.try_push(w)?;
                }
                continue;
            }

            break;
        }

        if name.is_none() && assigns.is_empty() && redirs.is_empty() {
            return Err(self.unexpected_token());
        }

        let a_start = self.ctx.ast.assigns.len();
        self.ctx.ast.assigns.try_extend_from_slice(&assigns)?;
        let arg_start = self.ctx.ast.word_lists.len();
        self.ctx.ast.word_lists.try_extend_from_slice(&args)?;
        let r_start = self.ctx.ast.redirs.len();
        self.commit_redirs(&redirs, r_start)?;

        Ok(self.ctx.ast.push_cmd(Command::Simple(SimpleCmd {
            assigns: Span::new(a_start, self.ctx.ast.assigns.len()),
            name,
            args: Span::new(arg_start, self.ctx.ast.word_lists.len()),
            redirs: Span::new(r_start, self.ctx.ast.redirs.len()),
            line,
        }))?)
    }

    fn is_simple_word(&self, has_cmd: bool) -> bool {
        match self.kind() {
            TokenKind::Word
            | TokenKind::Name
            | TokenKind::Number
            | TokenKind::Time
            | TokenKind::In => true,
            TokenKind::AssignmentWord
            | TokenKind::If
            | TokenKind::Then
            | TokenKind::Else
            | TokenKind::Elif
            | TokenKind::Fi
            | TokenKind::For
            | TokenKind::While
            | TokenKind::Until
            | TokenKind::Do
            | TokenKind::Done
            | TokenKind::Case
            | TokenKind::Esac
            | TokenKind::Function
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Bang
            | TokenKind::DBrackStart
            | TokenKind::DBrackEnd
                if has_cmd =>
            {
                true
            }
            _ => false,
        }
    }

    fn is_word_like_in_list(&self) -> bool {
        !matches!(
            self.kind(),
            TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::Semicolon
                | TokenKind::Amp
                | TokenKind::Pipe
                | TokenKind::PipeAmp
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::LParen
                | TokenKind::RParen
                | TokenKind::DSemi
                | TokenKind::SemiAnd
                | TokenKind::SemiSemiAnd
        ) && !self.kind().is_redir_op()
    }

    fn consume_word(&mut self, allow_brace: bool) -> Result<WordId, Error> {
        let raw = self.lexeme();
        self.advance();
        // Error position is `self.pos()` AFTER consuming the word token, matching `just-bash@3.6.0`!
        let err_pos = self.pos();
        if allow_brace {
            parse_word(raw, err_pos, self.ctx)
        } else {
            parse_word_no_brace(raw, err_pos, self.ctx)
        }
    }

    fn parse_assignment(&mut self) -> Result<Assign, Error> {
        let raw = self.lexeme();
        let mut copy = KBytes::with_capacity_in(raw.len(), self.ctx.budget)?;
        copy.try_extend_from_slice(raw)?;
        self.advance();
        let err_pos = self.pos();

        // Parse `name`, optional `[subscript]`, optional `+`, `=`, RHS
        let mut i = 0;
        while i < copy.len() && super::ident_byte(copy[i]) {
            i += 1;
        }
        let name = self.ctx.ast.push_bytes(&copy[..i])?;
        let subscript = if copy.get(i) == Some(&b'[') {
            let sub_start = i + 1;
            let mut depth = 1u32;
            i += 1;
            while i < copy.len() && depth > 0 {
                match copy[i] {
                    b'\\' if i + 1 < copy.len() => i += 2,
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
            let sub_end = i.saturating_sub(1);
            Some(self.ctx.ast.push_bytes(&copy[sub_start..sub_end])?)
        } else {
            None
        };
        let append = if copy.get(i) == Some(&b'+') {
            i += 1;
            true
        } else {
            false
        };
        if copy.get(i) == Some(&b'=') {
            i += 1;
        }
        let rhs = &copy[i..];

        // Array literal `name=(...)` / `name+=(...)`
        if subscript.is_none() && rhs.starts_with(b"(") && rhs.ends_with(b")") && rhs.len() >= 2 {
            let arr = parse_array_elements(&rhs[1..rhs.len() - 1], err_pos, self.ctx)?;
            return Ok(Assign {
                name,
                subscript: None,
                value: None,
                array: Some(arr),
                append,
            });
        }

        let value = parse_word_no_brace(rhs, err_pos, self.ctx)?;
        Ok(Assign {
            name,
            subscript,
            value: Some(value),
            array: None,
            append,
        })
    }

    fn is_redir_start(&self) -> bool {
        if self.kind().is_redir_op() {
            return true;
        }
        if self.kind() == TokenKind::FdVariable && self.peek_kind(1).is_redir_op() {
            return true;
        }
        if self.kind() == TokenKind::Number && !self.is_quoted() && self.peek_kind(1).is_redir_op()
        {
            let cur = self.lex.at(self.at);
            let next = self.lex.at(self.at + 1);
            if cur.src_end == next.src_start {
                if let Some(n) = parse_u64(cur.text.get(&self.lex.bytes)) {
                    return n <= i32::MAX as u64;
                }
            }
        }
        false
    }

    fn parse_trailing_redirs(&mut self) -> Result<Span, Error> {
        let mut redirs = KVec::new_in(self.ctx.budget);
        while self.is_redir_start() {
            self.parse_redirection_entry(&mut redirs)?;
        }
        if redirs.is_empty() {
            return Ok(Span::empty());
        }
        let r_start = self.ctx.ast.redirs.len();
        self.commit_redirs(&redirs, r_start)?;
        Ok(Span::new(r_start, self.ctx.ast.redirs.len()))
    }

    fn parse_redirection_entry(
        &mut self,
        redirs: &mut KVec<(Redir, Option<(bool, bool)>)>,
    ) -> Result<(), Error> {
        let mut fd: Option<u32> = None;
        let mut fd_var: Option<Span> = None;

        if self.kind() == TokenKind::Number {
            fd = parse_u64(self.lexeme()).map(|v| v as u32);
            self.advance();
        } else if self.kind() == TokenKind::FdVariable {
            fd_var = Some(self.ctx.ast.push_bytes(self.lexeme())?);
            self.advance();
        }

        let op_kind = self.kind();
        self.advance();

        if matches!(op_kind, TokenKind::DLess | TokenKind::DLessDash) {
            let strip_tabs = op_kind == TokenKind::DLessDash;
            if !self.is_word_like_in_list() {
                return Err(self.error_here(format_args!("Expected here-document delimiter")));
            }
            let quoted = self.is_quoted();
            self.advance(); // consume delimiter token
            let placeholder = self.ctx.ast.push_literal_word(b"")?;
            let r = Redir {
                fd,
                fd_var,
                op: RedirOp::In,
                target: RedirTarget::HereDoc {
                    content: placeholder,
                    strip_tabs,
                    quoted,
                },
            };
            redirs.try_push((r, Some((strip_tabs, quoted))))?;
            return Ok(());
        }

        let op = match op_kind {
            TokenKind::Less => RedirOp::In,
            TokenKind::Great => RedirOp::Out,
            TokenKind::DGreat => RedirOp::Append,
            TokenKind::LessAnd => RedirOp::DupIn,
            TokenKind::GreatAnd => RedirOp::DupOut,
            TokenKind::LessGreat => RedirOp::ReadWrite,
            TokenKind::Clobber => RedirOp::Clobber,
            TokenKind::TLess => RedirOp::HereStr,
            TokenKind::AndGreat => RedirOp::AndOut,
            TokenKind::AndDGreat => RedirOp::AndAppend,
            _ => return Err(self.unexpected_token()),
        };

        if !self.is_word_like_in_list() {
            return Err(self.error_here(format_args!("Expected redirection target")));
        }
        let target_word = self.consume_word(false)?;
        redirs.try_push((
            Redir {
                fd,
                fd_var,
                op,
                target: RedirTarget::Word(target_word),
            },
            None,
        ))?;
        Ok(())
    }

    fn commit_redirs(
        &mut self,
        redirs: &[(Redir, Option<(bool, bool)>)],
        base_idx: usize,
    ) -> Result<(), Error> {
        for (i, &(redir, hd)) in redirs.iter().enumerate() {
            self.ctx.ast.redirs.try_push(redir)?;
            if let Some((strip_tabs, quoted)) = hd {
                self.pending_heredocs.try_push(PendingHeredocRedir {
                    redir_idx: base_idx + i,
                    strip_tabs,
                    quoted,
                })?;
            }
        }
        Ok(())
    }
}

impl TokenKind {
    const DPAREN_END_ALIAS: TokenKind = TokenKind::DParenEnd;
}
