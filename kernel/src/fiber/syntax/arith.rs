//! Full bash arithmetic expression parser (`$((...))`, `$[...]`, `((...))`, `for ((;;))`).
use super::ast::{ArithAssignOp, ArithBinOp, ArithExpr, ArithId, ArithUnaryOp, Ast, Pos, Span};
use super::parser::ParseCtx;
use super::{ident_byte, ident_start, parser_err, Error, DEPTH_CAP};
use crate::mem::KVec;

/// Parse an arithmetic expression string into `Ast::arith_nodes`.
/// Depth-cap, memory, and work errors propagate as `Err`; arithmetic syntax errors
/// produce `Ok(ArithExpr::SyntaxError { .. })` for runtime reporting.
pub fn parse_arith(
    src: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<ArithId, Error> {
    let mut p = ArithParser {
        src,
        at: 0,
        err_pos,
        ctx,
    };
    p.skip_ws();
    if p.at >= p.src.len() {
        return Ok(p.ctx.ast.push_arith(ArithExpr::Number(0))?);
    }
    let expr = p.parse_comma()?;
    p.skip_ws();
    if p.at < p.src.len() {
        let rest = &p.src[p.at..];
        return p.syntax_error(b"syntax error in expression", rest);
    }
    Ok(expr)
}

struct ArithParser<'a, 'b, 'w> {
    src: &'a [u8],
    at: usize,
    err_pos: Pos,
    ctx: &'a mut ParseCtx<'b, 'w>,
}

impl<'a, 'b, 'w> ArithParser<'a, 'b, 'w> {
    fn skip_ws(&mut self) {
        while self.at < self.src.len() {
            match self.src[self.at] {
                b' ' | b'\t' | b'\r' | b'\n' => self.at += 1,
                b'\\' if self.src.get(self.at + 1) == Some(&b'\n') => self.at += 2,
                _ => break,
            }
        }
    }

    fn rest(&self) -> &'a [u8] {
        &self.src[self.at..]
    }

    fn syntax_error(&mut self, msg: &[u8], tok: &[u8]) -> Result<ArithId, Error> {
        let message = self.ctx.ast.push_bytes(msg)?;
        let error_token = self.ctx.ast.push_bytes(tok)?;
        Ok(self
            .ctx
            .ast
            .push_arith(ArithExpr::SyntaxError { message, error_token })?)
    }

    fn is_syntax_error(&self, id: ArithId) -> bool {
        matches!(
            self.ctx.ast.arith_nodes[id.0 as usize],
            ArithExpr::SyntaxError { .. }
        )
    }

    fn step(&mut self) -> Result<(), Error> {
        self.ctx.step(self.err_pos)
    }

    fn parse_comma(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_assign()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            if self.src.get(self.at) == Some(&b',') {
                self.step()?;
                self.at += 1;
                let right = self.parse_assign()?;
                if self.is_syntax_error(right) {
                    return Ok(right);
                }
                left = self.ctx.ast.push_arith(ArithExpr::Binary {
                    op: ArithBinOp::Comma,
                    left,
                    right,
                })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_assign(&mut self) -> Result<ArithId, Error> {
        let left = self.parse_ternary()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        self.skip_ws();
        let Some((op, len)) = self.peek_assign_op() else {
            return Ok(left);
        };
        let target = self.ctx.ast.arith_nodes[left.0 as usize];
        let (var, subscript, string_key) = match target {
            ArithExpr::Var { name, has_dollar: false } => (name, None, None),
            ArithExpr::ArrayElement {
                array,
                index,
                string_key,
            } => (array, index, string_key),
            _ => {
                let tok = &self.src[self.at..];
                return self.syntax_error(b"attempted assignment to non-variable", tok);
            }
        };
        self.step()?;
        self.at += len;
        let value = self.parse_assign()?;
        if self.is_syntax_error(value) {
            return Ok(value);
        }
        Ok(self.ctx.ast.push_arith(ArithExpr::Assign {
            op,
            var,
            subscript,
            string_key,
            value,
        })?)
    }

    fn peek_assign_op(&self) -> Option<(ArithAssignOp, usize)> {
        match self.rest() {
            [b'<', b'<', b'=', ..] => Some((ArithAssignOp::Shl, 3)),
            [b'>', b'>', b'=', ..] => Some((ArithAssignOp::Shr, 3)),
            [b'+', b'=', ..] => Some((ArithAssignOp::Add, 2)),
            [b'-', b'=', ..] => Some((ArithAssignOp::Sub, 2)),
            [b'*', b'=', ..] => Some((ArithAssignOp::Mul, 2)),
            [b'/', b'=', ..] => Some((ArithAssignOp::Div, 2)),
            [b'%', b'=', ..] => Some((ArithAssignOp::Mod, 2)),
            [b'&', b'=', ..] => Some((ArithAssignOp::BitAnd, 2)),
            [b'|', b'=', ..] => Some((ArithAssignOp::BitOr, 2)),
            [b'^', b'=', ..] => Some((ArithAssignOp::BitXor, 2)),
            [b'=', next, ..] if *next != b'=' => Some((ArithAssignOp::Assign, 1)),
            [b'='] => Some((ArithAssignOp::Assign, 1)),
            _ => None,
        }
    }

    fn parse_ternary(&mut self) -> Result<ArithId, Error> {
        let cond = self.parse_log_or()?;
        if self.is_syntax_error(cond) {
            return Ok(cond);
        }
        self.skip_ws();
        if self.src.get(self.at) == Some(&b'?') {
            self.step()?;
            self.at += 1;
            let consequent = self.parse_comma()?;
            if self.is_syntax_error(consequent) {
                return Ok(consequent);
            }
            self.skip_ws();
            if self.src.get(self.at) != Some(&b':') {
                return self.syntax_error(b"`:' expected for conditional expression", self.rest());
            }
            self.at += 1;
            let alternate = self.parse_ternary()?;
            if self.is_syntax_error(alternate) {
                return Ok(alternate);
            }
            return Ok(self.ctx.ast.push_arith(ArithExpr::Ternary {
                cond,
                consequent,
                alternate,
            })?);
        }
        Ok(cond)
    }

    fn parse_log_or(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_log_and()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            if matches!(self.rest(), [b'|', b'|', ..]) {
                self.step()?;
                self.at += 2;
                let right = self.parse_log_and()?;
                if self.is_syntax_error(right) {
                    return Ok(right);
                }
                left = self.ctx.ast.push_arith(ArithExpr::Binary {
                    op: ArithBinOp::LogOr,
                    left,
                    right,
                })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_log_and(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_bit_or()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            if matches!(self.rest(), [b'&', b'&', ..]) {
                self.step()?;
                self.at += 2;
                let right = self.parse_bit_or()?;
                if self.is_syntax_error(right) {
                    return Ok(right);
                }
                left = self.ctx.ast.push_arith(ArithExpr::Binary {
                    op: ArithBinOp::LogAnd,
                    left,
                    right,
                })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_bit_or(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_bit_xor()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            if matches!(self.rest(), [b'|', c, ..] if *c != b'|' && *c != b'=')
                || self.rest() == b"|"
            {
                self.step()?;
                self.at += 1;
                let right = self.parse_bit_xor()?;
                if self.is_syntax_error(right) {
                    return Ok(right);
                }
                left = self.ctx.ast.push_arith(ArithExpr::Binary {
                    op: ArithBinOp::BitOr,
                    left,
                    right,
                })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_bit_xor(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_bit_and()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            if matches!(self.rest(), [b'^', c, ..] if *c != b'=') || self.rest() == b"^" {
                self.step()?;
                self.at += 1;
                let right = self.parse_bit_and()?;
                if self.is_syntax_error(right) {
                    return Ok(right);
                }
                left = self.ctx.ast.push_arith(ArithExpr::Binary {
                    op: ArithBinOp::BitXor,
                    left,
                    right,
                })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_bit_and(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_equality()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            if matches!(self.rest(), [b'&', c, ..] if *c != b'&' && *c != b'=')
                || self.rest() == b"&"
            {
                self.step()?;
                self.at += 1;
                let right = self.parse_equality()?;
                if self.is_syntax_error(right) {
                    return Ok(right);
                }
                left = self.ctx.ast.push_arith(ArithExpr::Binary {
                    op: ArithBinOp::BitAnd,
                    left,
                    right,
                })?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_relational()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            let op = match self.rest() {
                [b'=', b'=', ..] => ArithBinOp::Eq,
                [b'!', b'=', ..] => ArithBinOp::Ne,
                _ => break,
            };
            self.step()?;
            self.at += 2;
            let right = self.parse_relational()?;
            if self.is_syntax_error(right) {
                return Ok(right);
            }
            left = self.ctx.ast.push_arith(ArithExpr::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_shift()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            let (op, len) = match self.rest() {
                [b'<', b'=', ..] => (ArithBinOp::Le, 2),
                [b'>', b'=', ..] => (ArithBinOp::Ge, 2),
                [b'<', c, ..] if *c != b'<' => (ArithBinOp::Lt, 1),
                [b'<'] => (ArithBinOp::Lt, 1),
                [b'>', c, ..] if *c != b'>' => (ArithBinOp::Gt, 1),
                [b'>'] => (ArithBinOp::Gt, 1),
                _ => break,
            };
            self.step()?;
            self.at += len;
            let right = self.parse_shift()?;
            if self.is_syntax_error(right) {
                return Ok(right);
            }
            left = self.ctx.ast.push_arith(ArithExpr::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_add()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            let op = match self.rest() {
                [b'<', b'<', c, ..] if *c != b'=' => ArithBinOp::Shl,
                [b'<', b'<'] => ArithBinOp::Shl,
                [b'>', b'>', c, ..] if *c != b'=' => ArithBinOp::Shr,
                [b'>', b'>'] => ArithBinOp::Shr,
                _ => break,
            };
            self.step()?;
            self.at += 2;
            let right = self.parse_add()?;
            if self.is_syntax_error(right) {
                return Ok(right);
            }
            left = self.ctx.ast.push_arith(ArithExpr::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_mul()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            let op = match self.rest() {
                [b'+', c, ..] if *c != b'+' && *c != b'=' => ArithBinOp::Add,
                [b'+'] => ArithBinOp::Add,
                [b'-', c, ..] if *c != b'-' && *c != b'=' => ArithBinOp::Sub,
                [b'-'] => ArithBinOp::Sub,
                _ => break,
            };
            self.step()?;
            self.at += 1;
            let right = self.parse_mul()?;
            if self.is_syntax_error(right) {
                return Ok(right);
            }
            left = self.ctx.ast.push_arith(ArithExpr::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<ArithId, Error> {
        let mut left = self.parse_pow()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        loop {
            self.skip_ws();
            let op = match self.rest() {
                [b'*', c, ..] if *c != b'*' && *c != b'=' => ArithBinOp::Mul,
                [b'*'] => ArithBinOp::Mul,
                [b'/', c, ..] if *c != b'=' => ArithBinOp::Div,
                [b'/'] => ArithBinOp::Div,
                [b'%', c, ..] if *c != b'=' => ArithBinOp::Mod,
                [b'%'] => ArithBinOp::Mod,
                _ => break,
            };
            self.step()?;
            self.at += 1;
            let right = self.parse_pow()?;
            if self.is_syntax_error(right) {
                return Ok(right);
            }
            left = self.ctx.ast.push_arith(ArithExpr::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_pow(&mut self) -> Result<ArithId, Error> {
        let left = self.parse_unary()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        self.skip_ws();
        if matches!(self.rest(), [b'*', b'*', ..]) {
            self.step()?;
            self.at += 2;
            let right = self.parse_pow()?;
            if self.is_syntax_error(right) {
                return Ok(right);
            }
            return Ok(self.ctx.ast.push_arith(ArithExpr::Binary {
                op: ArithBinOp::Pow,
                left,
                right,
            })?);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<ArithId, Error> {
        self.skip_ws();
        let (op, len) = match self.rest() {
            [b'+', b'+', ..] => (ArithUnaryOp::Inc, 2),
            [b'-', b'-', ..] => (ArithUnaryOp::Dec, 2),
            [b'+', ..] => (ArithUnaryOp::Pos, 1),
            [b'-', ..] => (ArithUnaryOp::Neg, 1),
            [b'!', ..] => (ArithUnaryOp::LogNot, 1),
            [b'~', ..] => (ArithUnaryOp::BitNot, 1),
            _ => return self.parse_postfix(),
        };
        self.step()?;
        self.at += len;
        let operand = self.parse_unary()?;
        if self.is_syntax_error(operand) {
            return Ok(operand);
        }
        Ok(self.ctx.ast.push_arith(ArithExpr::Unary {
            op,
            operand,
            prefix: true,
        })?)
    }

    fn parse_postfix(&mut self) -> Result<ArithId, Error> {
        let expr = self.parse_primary()?;
        if self.is_syntax_error(expr) {
            return Ok(expr);
        }
        self.skip_ws();
        let op = match self.rest() {
            [b'+', b'+', ..] => ArithUnaryOp::Inc,
            [b'-', b'-', ..] => ArithUnaryOp::Dec,
            _ => return Ok(expr),
        };
        self.step()?;
        self.at += 2;
        Ok(self.ctx.ast.push_arith(ArithExpr::Unary {
            op,
            operand: expr,
            prefix: false,
        })?)
    }

    fn parse_primary(&mut self) -> Result<ArithId, Error> {
        self.skip_ws();
        self.step()?;
        let Some(&b) = self.src.get(self.at) else {
            return self.syntax_error(b"operand expected", b"");
        };

        // Parenthesized expression `(...)` — enforces shared depth cap (200)!
        if b == b'(' {
            self.at += 1;
            self.ctx.depth += 1;
            if self.ctx.depth > DEPTH_CAP {
                return Err(parser_err(
                    self.err_pos,
                    self.ctx.budget,
                    format_args!("Maximum parser nesting depth exceeded ({DEPTH_CAP})"),
                ));
            }
            let inner = self.parse_comma()?;
            self.ctx.depth -= 1;
            if self.is_syntax_error(inner) {
                return Ok(inner);
            }
            self.skip_ws();
            if self.src.get(self.at) != Some(&b')') {
                return self.syntax_error(b"missing `)'", self.rest());
            }
            self.at += 1;
            return Ok(self.ctx.ast.push_arith(ArithExpr::Group(inner))?);
        }

        // Single-quoted character literal `'a'`
        if b == b'\'' {
            let start = self.at + 1;
            if let Some(rel_end) = self.src[start..].iter().position(|&c| c == b'\'') {
                let content_slice = &self.src[start..start + rel_end];
                let val = content_slice.first().copied().unwrap_or(0) as i64;
                let content = self.ctx.ast.push_bytes(content_slice)?;
                self.at = start + rel_end + 1;
                return Ok(self
                    .ctx
                    .ast
                    .push_arith(ArithExpr::SingleQuote { content, value: val })?);
            }
            return self.syntax_error(b"unclosed single quote", self.rest());
        }

        // Double-quoted piece (strip quotes and parse content or treat as var/number)
        if b == b'"' {
            let start = self.at + 1;
            let mut end = start;
            while end < self.src.len() && self.src[end] != b'"' {
                if self.src[end] == b'\\' && end + 1 < self.src.len() {
                    end += 2;
                } else {
                    end += 1;
                }
            }
            let inner = &self.src[start..end];
            self.at = if end < self.src.len() { end + 1 } else { end };
            return parse_arith(inner, self.err_pos, self.ctx);
        }

        // Backtick command substitution
        if b == b'`' {
            let first = self.parse_backtick_piece()?;
            return self.maybe_concat(first);
        }

        // `$` expansions (`$((...))`, `$[...]`, `$(...)`, `${...}`, `$var`, `$1`, `$?`, ...)
        if b == b'$' {
            let first = self.parse_dollar_piece()?;
            // Check dynamic base: `$base#val` or `${base}#val`
            if self.src.get(self.at) == Some(&b'#') {
                if let Some(base_span) = self.extract_dynamic_base_span(first) {
                    self.at += 1;
                    let val_start = self.at;
                    while self
                        .src
                        .get(self.at)
                        .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'@' || c == b'_')
                    {
                        self.at += 1;
                    }
                    let value = self.ctx.ast.push_bytes(&self.src[val_start..self.at])?;
                    return Ok(self.ctx.ast.push_arith(ArithExpr::DynamicBase {
                        base_expr: base_span,
                        value,
                    })?);
                }
            }
            return self.maybe_concat(first);
        }

        // Number literal (decimal, 0-octal, 0x-hex, base#digits, dynamic number, `1[2]`)
        if b.is_ascii_digit() {
            return self.parse_number();
        }

        // Identifier / array element
        if ident_start(b) {
            let start = self.at;
            while self.src.get(self.at).is_some_and(|&c| ident_byte(c)) {
                self.at += 1;
            }
            let ident_slice = &self.src[start..self.at];
            // Check subscript `arr[expr]`
            if self.src.get(self.at) == Some(&b'[') {
                let array = self.ctx.ast.push_bytes(ident_slice)?;
                let (index, string_key) = self.parse_subscript()?;
                if self.src.get(self.at) == Some(&b'[') {
                    // Double subscript `a[1][1]` -> deferred DoubleSubscript
                    let _ = self.parse_subscript()?;
                    let idx_id = match index {
                        Some(id) => id,
                        None => self.ctx.ast.push_arith(ArithExpr::Number(0))?,
                    };
                    return Ok(self
                        .ctx
                        .ast
                        .push_arith(ArithExpr::DoubleSubscript { array, index: idx_id })?);
                }
                return Ok(self.ctx.ast.push_arith(ArithExpr::ArrayElement {
                    array,
                    index,
                    string_key,
                })?);
            }
            let name = self.ctx.ast.push_bytes(ident_slice)?;
            let first = self.ctx.ast.push_arith(ArithExpr::Var {
                name,
                has_dollar: false,
            })?;
            return self.maybe_concat(first);
        }

        let tok = &self.src[self.at..];
        self.syntax_error(b"operand expected", tok)
    }

    fn extract_dynamic_base_span(&mut self, id: ArithId) -> Option<Span> {
        match self.ctx.ast.arith_nodes[id.0 as usize] {
            ArithExpr::Var { name, .. } => Some(name),
            ArithExpr::BracedExp(s) => Some(s),
            _ => None,
        }
    }

    fn maybe_concat(&mut self, first: ArithId) -> Result<ArithId, Error> {
        if !matches!(self.src.get(self.at), Some(b'$' | b'`'))
            && !self.src.get(self.at).is_some_and(|&c| ident_byte(c))
        {
            return Ok(first);
        }
        let mut items = KVec::new_in(self.ctx.budget);
        items.try_push(first)?;
        while let Some(&c) = self.src.get(self.at) {
            if c == b'$' {
                items.try_push(self.parse_dollar_piece()?)?;
            } else if c == b'`' {
                items.try_push(self.parse_backtick_piece()?)?;
            } else if ident_byte(c) {
                let s = self.at;
                while self.src.get(self.at).is_some_and(|&x| ident_byte(x)) {
                    self.at += 1;
                }
                let sp = self.ctx.ast.push_bytes(&self.src[s..self.at])?;
                items.try_push(self.ctx.ast.push_arith(ArithExpr::Var {
                    name: sp,
                    has_dollar: false,
                })?)?;
            } else {
                break;
            }
        }
        let list_start = self.ctx.ast.arith_lists.len();
        self.ctx.ast.arith_lists.try_extend_from_slice(&items)?;
        let span = Span::new(list_start, self.ctx.ast.arith_lists.len());
        Ok(self.ctx.ast.push_arith(ArithExpr::Concat(span))?)
    }

    fn parse_dollar_piece(&mut self) -> Result<ArithId, Error> {
        self.at += 1; // skip '$'
        match self.src.get(self.at).copied() {
            Some(b'(') if self.src.get(self.at + 1) == Some(&b'(') => {
                // Nested `$((...))`
                let inner = self.scan_nested_dparen()?;
                let nested = parse_arith(inner, self.err_pos, self.ctx)?;
                Ok(self.ctx.ast.push_arith(ArithExpr::Nested(nested))?)
            }
            Some(b'[') => {
                // Nested `$[...]`
                let inner = self.scan_delimited(b'[', b']')?;
                let nested = parse_arith(inner, self.err_pos, self.ctx)?;
                Ok(self.ctx.ast.push_arith(ArithExpr::Nested(nested))?)
            }
            Some(b'(') => {
                // Command substitution `$(...)`
                let inner = self.scan_delimited(b'(', b')')?;
                let body = super::parser::parse_sub_script(inner, self.err_pos, self.ctx)?;
                Ok(self.ctx.ast.push_arith(ArithExpr::CommandSubst {
                    body,
                    backtick: false,
                })?)
            }
            Some(b'{') => {
                let inner = self.scan_delimited(b'{', b'}')?;
                let span = self.ctx.ast.push_bytes(inner)?;
                Ok(self.ctx.ast.push_arith(ArithExpr::BracedExp(span))?)
            }
            Some(c) if ident_start(c) => {
                let start = self.at;
                while self.src.get(self.at).is_some_and(|&x| ident_byte(x)) {
                    self.at += 1;
                }
                let name = self.ctx.ast.push_bytes(&self.src[start..self.at])?;
                Ok(self.ctx.ast.push_arith(ArithExpr::Var {
                    name,
                    has_dollar: true,
                })?)
            }
            Some(b'?' | b'$' | b'!' | b'#' | b'@' | b'*' | b'-' | b'0'..=b'9') => {
                let start = self.at;
                self.at += 1;
                let name = self.ctx.ast.push_bytes(&self.src[start..self.at])?;
                Ok(self.ctx.ast.push_arith(ArithExpr::SpecialVar { name })?)
            }
            _ => self.syntax_error(b"operand expected", b"$"),
        }
    }

    fn parse_backtick_piece(&mut self) -> Result<ArithId, Error> {
        self.at += 1; // skip '`'
        let start = self.at;
        while self.at < self.src.len() && self.src[self.at] != b'`' {
            if self.src[self.at] == b'\\' && self.at + 1 < self.src.len() {
                self.at += 2;
            } else {
                self.at += 1;
            }
        }
        let inner = &self.src[start..self.at];
        if self.at < self.src.len() {
            self.at += 1;
        }
        let body = super::parser::parse_sub_script(inner, self.err_pos, self.ctx)?;
        Ok(self.ctx.ast.push_arith(ArithExpr::CommandSubst {
            body,
            backtick: true,
        })?)
    }

    fn scan_nested_dparen(&mut self) -> Result<&'a [u8], Error> {
        self.at += 2; // skip '(('
        let start = self.at;
        let mut depth = 2u32;
        while self.at < self.src.len() {
            match self.src[self.at] {
                b'(' => {
                    depth += 1;
                    self.at += 1;
                }
                b')' => {
                    if depth == 2 && self.src.get(self.at + 1) == Some(&b')') {
                        let end = self.at;
                        self.at += 2;
                        return Ok(&self.src[start..end]);
                    }
                    depth = depth.saturating_sub(1);
                    self.at += 1;
                }
                _ => self.at += 1,
            }
        }
        Ok(&self.src[start..self.at])
    }

    fn scan_delimited(&mut self, open: u8, close: u8) -> Result<&'a [u8], Error> {
        self.at += 1; // skip open
        let start = self.at;
        let mut depth = 1u32;
        while self.at < self.src.len() && depth > 0 {
            let c = self.src[self.at];
            if c == b'\\' && self.at + 1 < self.src.len() {
                self.at += 2;
                continue;
            }
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    let end = self.at;
                    self.at += 1;
                    return Ok(&self.src[start..end]);
                }
            }
            self.at += 1;
        }
        Ok(&self.src[start..self.at])
    }

    fn parse_subscript(&mut self) -> Result<(Option<ArithId>, Option<Span>), Error> {
        let inner = self.scan_delimited(b'[', b']')?;
        let trimmed = trim_ascii(inner);
        if trimmed.is_empty() {
            return Ok((None, None));
        }
        // Quoted or non-arith string keys (for associative arrays) vs arithmetic index
        if (trimmed.starts_with(b"'") && trimmed.ends_with(b"'") && trimmed.len() >= 2)
            || (trimmed.starts_with(b"\"") && trimmed.ends_with(b"\"") && trimmed.len() >= 2)
        {
            let key = self.ctx.ast.push_bytes(&trimmed[1..trimmed.len() - 1])?;
            return Ok((None, Some(key)));
        }
        let idx = parse_arith(trimmed, self.err_pos, self.ctx)?;
        Ok((Some(idx), None))
    }

    fn parse_number(&mut self) -> Result<ArithId, Error> {
        let start = self.at;
        // Scan prefix digits / hex / base#digits
        while self
            .src
            .get(self.at)
            .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.at += 1;
        }
        // Check `base#digits`
        if self.src.get(self.at) == Some(&b'#') {
            let base_bytes = &self.src[start..self.at];
            self.at += 1; // skip '#'
            let digits_start = self.at;
            while self
                .src
                .get(self.at)
                .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'@' || c == b'_')
            {
                self.at += 1;
            }
            let digits = &self.src[digits_start..self.at];
            if let Some(base) = parse_decimal_u32(base_bytes) {
                if (2..=64).contains(&base) && !digits.is_empty() {
                    return match parse_base_digits(base, digits) {
                        BaseResult::Ok(n) => Ok(self.ctx.ast.push_arith(ArithExpr::Number(n))?),
                        BaseResult::Overflow => {
                            Ok(self.ctx.ast.push_arith(ArithExpr::NumberOverflow)?)
                        }
                        BaseResult::InvalidDigit => {
                            self.syntax_error(b"value too great for base", &self.src[start..self.at])
                        }
                    };
                }
                return self.syntax_error(b"invalid arithmetic base", &self.src[start..self.at]);
            }
            return self.syntax_error(b"invalid arithmetic base", &self.src[start..self.at]);
        }

        let token = &self.src[start..self.at];

        // Dynamic number suffix like `12$x` or `0x$h`
        if self.src.get(self.at) == Some(&b'$') {
            let suffix_start = self.at;
            while self
                .src
                .get(self.at)
                .is_some_and(|&c| c == b'$' || c == b'{' || c == b'}' || ident_byte(c))
            {
                self.at += 1;
            }
            let prefix = self.ctx.ast.push_bytes(token)?;
            let suffix = self.ctx.ast.push_bytes(&self.src[suffix_start..self.at])?;
            return Ok(self
                .ctx
                .ast
                .push_arith(ArithExpr::DynamicNumber { prefix, suffix })?);
        }

        // Number subscript `1[2]` -> deferred `NumberSubscript`
        if self.src.get(self.at) == Some(&b'[') {
            let err_start = self.at;
            let _ = self.scan_delimited(b'[', b']')?;
            let number = self.ctx.ast.push_bytes(token)?;
            let error_token = self.ctx.ast.push_bytes(&self.src[err_start..self.at])?;
            return Ok(self.ctx.ast.push_arith(ArithExpr::NumberSubscript {
                number,
                error_token,
            })?);
        }

        // Hex `0x...` / `0X...`
        if token.len() >= 2 && token[0] == b'0' && matches!(token[1], b'x' | b'X') {
            let digits = &token[2..];
            if digits.is_empty() {
                return self.syntax_error(b"invalid hex constant", token);
            }
            return match parse_base_digits(16, digits) {
                BaseResult::Ok(n) => Ok(self.ctx.ast.push_arith(ArithExpr::Number(n))?),
                BaseResult::Overflow => Ok(self.ctx.ast.push_arith(ArithExpr::NumberOverflow)?),
                BaseResult::InvalidDigit => self.syntax_error(b"invalid hex constant", token),
            };
        }

        // Octal `0...`
        if token.len() >= 2 && token[0] == b'0' {
            return match parse_base_digits(8, &token[1..]) {
                BaseResult::Ok(n) => Ok(self.ctx.ast.push_arith(ArithExpr::Number(n))?),
                BaseResult::Overflow => Ok(self.ctx.ast.push_arith(ArithExpr::NumberOverflow)?),
                BaseResult::InvalidDigit => {
                    self.syntax_error(b"value too great for base", token)
                }
            };
        }

        // Decimal
        match parse_base_digits(10, token) {
            BaseResult::Ok(n) => Ok(self.ctx.ast.push_arith(ArithExpr::Number(n))?),
            BaseResult::Overflow => Ok(self.ctx.ast.push_arith(ArithExpr::NumberOverflow)?),
            BaseResult::InvalidDigit => self.syntax_error(b"invalid number", token),
        }
    }
}

enum BaseResult {
    Ok(i64),
    Overflow,
    InvalidDigit,
}

fn parse_decimal_u32(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut n: u32 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(n)
}

fn digit_value(base: u32, b: u8) -> Option<u32> {
    let v = match b {
        b'0'..=b'9' => (b - b'0') as u32,
        b'a'..=b'z' => (b - b'a') as u32 + 10,
        b'A'..=b'Z' => {
            if base <= 36 {
                (b - b'A') as u32 + 10
            } else {
                (b - b'A') as u32 + 36
            }
        }
        b'@' => 62,
        b'_' => 63,
        _ => return None,
    };
    (v < base).then_some(v)
}

fn parse_base_digits(base: u32, digits: &[u8]) -> BaseResult {
    let mut acc: u64 = 0;
    let base64 = base as u64;
    for &b in digits {
        let Some(d) = digit_value(base, b) else {
            return BaseResult::InvalidDigit;
        };
        let Some(next) = acc.checked_mul(base64).and_then(|v| v.checked_add(d as u64)) else {
            return BaseResult::Overflow;
        };
        acc = next;
    }
    if acc > i64::MAX as u64 {
        BaseResult::Overflow
    } else {
        BaseResult::Ok(acc as i64)
    }
}

fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while let Some(&b) = s.first() {
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            s = &s[1..];
        } else {
            break;
        }
    }
    while let Some(&b) = s.last() {
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            s = &s[..s.len() - 1];
        } else {
            break;
        }
    }
    s
}

#[allow(dead_code)]
fn _touch_ast(_: &Ast) {}
