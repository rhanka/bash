//! Full bash arithmetic expression parser (`$((...))`, `$[...]`, `((...))`, `for ((;;))`).
use super::ast::{ArithAssignOp, ArithBinOp, ArithExpr, ArithId, ArithUnaryOp, Ast, Pos, Span};
use super::parser::ParseCtx;
use super::{ident_byte, ident_start, Error};
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
        let left = self.parse_assign()?;
        if self.is_syntax_error(left) {
            return Ok(left);
        }
        self.skip_ws();
        if self.src.get(self.at) != Some(&b',') {
            return Ok(left);
        }
        self.parse_comma_tail(left)
    }

    #[inline(never)]
    fn parse_comma_tail(&mut self, mut left: ArithId) -> Result<ArithId, Error> {
        while self.src.get(self.at) == Some(&b',') {
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
            self.skip_ws();
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
        self.parse_assign_tail(left, op, len)
    }

    #[inline(never)]
    fn parse_assign_tail(
        &mut self,
        first_left: ArithId,
        first_op: ArithAssignOp,
        first_len: usize,
    ) -> Result<ArithId, Error> {
        let mut chain = KVec::new_in(self.ctx.budget);
        let mut cur_left = first_left;
        let mut cur_op = first_op;
        let mut cur_len = first_len;
        let value = loop {
            let target = self.ctx.ast.arith_nodes[cur_left.0 as usize];
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
            self.at += cur_len;
            chain.try_push((cur_op, var, subscript, string_key))?;
            let next = self.parse_ternary()?;
            if self.is_syntax_error(next) {
                return Ok(next);
            }
            self.skip_ws();
            let Some((op, len)) = self.peek_assign_op() else {
                break next;
            };
            cur_left = next;
            cur_op = op;
            cur_len = len;
        };
        let mut acc = value;
        for &(op, var, subscript, string_key) in chain.iter().rev() {
            acc = self.ctx.ast.push_arith(ArithExpr::Assign {
                op,
                var,
                subscript,
                string_key,
                value: acc,
            })?;
        }
        Ok(acc)
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
        let cond = self.parse_bin_expr()?;
        if self.is_syntax_error(cond) {
            return Ok(cond);
        }
        self.skip_ws();
        if self.src.get(self.at) != Some(&b'?') {
            return Ok(cond);
        }
        self.parse_ternary_tail(cond)
    }

    #[inline(never)]
    fn parse_ternary_tail(&mut self, first_cond: ArithId) -> Result<ArithId, Error> {
        let mut chain = KVec::new_in(self.ctx.budget);
        let mut cur_cond = first_cond;
        let alternate = loop {
            self.step()?;
            self.at += 1;
            self.ctx.enter_depth(self.err_pos)?;
            let consequent = self.parse_comma();
            self.ctx.leave_depth();
            let consequent = consequent?;
            if self.is_syntax_error(consequent) {
                return Ok(consequent);
            }
            self.skip_ws();
            if self.src.get(self.at) != Some(&b':') {
                return self.syntax_error(b"`:' expected for conditional expression", self.rest());
            }
            self.at += 1;
            chain.try_push((cur_cond, consequent))?;
            let next = self.parse_bin_expr()?;
            if self.is_syntax_error(next) {
                return Ok(next);
            }
            self.skip_ws();
            if self.src.get(self.at) != Some(&b'?') {
                break next;
            }
            cur_cond = next;
        };
        let mut acc = alternate;
        for &(cond, consequent) in chain.iter().rev() {
            acc = self.ctx.ast.push_arith(ArithExpr::Ternary {
                cond,
                consequent,
                alternate: acc,
            })?;
        }
        Ok(acc)
    }

    fn peek_bin_op(&self) -> Option<(ArithBinOp, u8, usize)> {
        match self.rest() {
            [b'|', b'|', ..] => Some((ArithBinOp::LogOr, 1, 2)),
            [b'&', b'&', ..] => Some((ArithBinOp::LogAnd, 2, 2)),
            [b'|', b'=', ..] => None,
            [b'|', ..] => Some((ArithBinOp::BitOr, 3, 1)),
            [b'^', b'=', ..] => None,
            [b'^', ..] => Some((ArithBinOp::BitXor, 4, 1)),
            [b'&', b'=', ..] => None,
            [b'&', ..] => Some((ArithBinOp::BitAnd, 5, 1)),
            [b'=', b'=', ..] => Some((ArithBinOp::Eq, 6, 2)),
            [b'!', b'=', ..] => Some((ArithBinOp::Ne, 6, 2)),
            [b'<', b'<', b'=', ..] => None,
            [b'>', b'>', b'=', ..] => None,
            [b'<', b'<', ..] => Some((ArithBinOp::Shl, 8, 2)),
            [b'>', b'>', ..] => Some((ArithBinOp::Shr, 8, 2)),
            [b'<', b'=', ..] => Some((ArithBinOp::Le, 7, 2)),
            [b'>', b'=', ..] => Some((ArithBinOp::Ge, 7, 2)),
            [b'<', ..] => Some((ArithBinOp::Lt, 7, 1)),
            [b'>', ..] => Some((ArithBinOp::Gt, 7, 1)),
            [b'+', b'+' | b'=', ..] => None,
            [b'+', ..] => Some((ArithBinOp::Add, 9, 1)),
            [b'-', b'-' | b'=', ..] => None,
            [b'-', ..] => Some((ArithBinOp::Sub, 9, 1)),
            [b'*', b'*', ..] => Some((ArithBinOp::Pow, 11, 2)),
            [b'*', b'=', ..] => None,
            [b'*', ..] => Some((ArithBinOp::Mul, 10, 1)),
            [b'/', b'=', ..] => None,
            [b'/', ..] => Some((ArithBinOp::Div, 10, 1)),
            [b'%', b'=', ..] => None,
            [b'%', ..] => Some((ArithBinOp::Mod, 10, 1)),
            _ => None,
        }
    }

    /// Iterative shunting-yard parser for all binary operators (`||` through `**`),
    /// avoiding an 11-deep Rust call-stack chain per nesting level.
    fn parse_bin_expr(&mut self) -> Result<ArithId, Error> {
        let first = self.parse_unary()?;
        if self.is_syntax_error(first) {
            return Ok(first);
        }
        self.skip_ws();
        let Some((first_op, first_prec, first_len)) = self.peek_bin_op() else {
            return Ok(first);
        };
        self.parse_bin_expr_tail(first, first_op, first_prec, first_len)
    }

    #[inline(never)]
    fn parse_bin_expr_tail(
        &mut self,
        first: ArithId,
        first_op: ArithBinOp,
        first_prec: u8,
        first_len: usize,
    ) -> Result<ArithId, Error> {
        let mut vals = KVec::new_in(self.ctx.budget);
        let mut ops: KVec<(ArithBinOp, u8)> = KVec::new_in(self.ctx.budget);
        vals.try_push(first)?;
        self.step()?;
        self.at += first_len;
        ops.try_push((first_op, first_prec))?;
        loop {
            let rhs = self.parse_unary()?;
            if self.is_syntax_error(rhs) {
                return Ok(rhs);
            }
            vals.try_push(rhs)?;
            self.skip_ws();
            let Some((op, prec, len)) = self.peek_bin_op() else {
                break;
            };
            while let Some(&(top_op, top_prec)) = ops.last() {
                if top_prec > prec || (top_prec == prec && op != ArithBinOp::Pow) {
                    ops.pop();
                    let right = vals.pop().unwrap();
                    let left = vals.pop().unwrap();
                    let node = self.ctx.ast.push_arith(ArithExpr::Binary {
                        op: top_op,
                        left,
                        right,
                    })?;
                    vals.try_push(node)?;
                } else {
                    break;
                }
            }
            self.step()?;
            self.at += len;
            ops.try_push((op, prec))?;
        }
        while let Some((top_op, _)) = ops.pop() {
            let right = vals.pop().unwrap();
            let left = vals.pop().unwrap();
            let node = self.ctx.ast.push_arith(ArithExpr::Binary {
                op: top_op,
                left,
                right,
            })?;
            vals.try_push(node)?;
        }
        Ok(vals.pop().unwrap())
    }

    fn peek_unary_op(&self) -> Option<(ArithUnaryOp, usize)> {
        match self.rest() {
            [b'+', b'+', ..] => Some((ArithUnaryOp::Inc, 2)),
            [b'-', b'-', ..] => Some((ArithUnaryOp::Dec, 2)),
            [b'+', ..] => Some((ArithUnaryOp::Pos, 1)),
            [b'-', ..] => Some((ArithUnaryOp::Neg, 1)),
            [b'!', ..] => Some((ArithUnaryOp::LogNot, 1)),
            [b'~', ..] => Some((ArithUnaryOp::BitNot, 1)),
            _ => None,
        }
    }

    fn parse_unary(&mut self) -> Result<ArithId, Error> {
        self.skip_ws();
        if self.peek_unary_op().is_some() {
            return self.parse_unary_chain();
        }
        self.parse_postfix()
    }

    #[inline(never)]
    fn parse_unary_chain(&mut self) -> Result<ArithId, Error> {
        let mut ops = KVec::new_in(self.ctx.budget);
        while let Some((op, len)) = self.peek_unary_op() {
            self.step()?;
            self.at += len;
            ops.try_push(op)?;
            self.skip_ws();
        }
        let mut expr = self.parse_postfix()?;
        if self.is_syntax_error(expr) {
            return Ok(expr);
        }
        for &op in ops.iter().rev() {
            expr = self.ctx.ast.push_arith(ArithExpr::Unary {
                op,
                operand: expr,
                prefix: true,
            })?;
        }
        Ok(expr)
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
        match b {
            b'(' => self.parse_primary_group(),
            b'\'' => self.parse_primary_single_quote(),
            b'"' => self.parse_primary_double_quote(),
            b'`' => self.parse_primary_backtick(),
            b'$' => self.parse_primary_dollar(),
            b'0'..=b'9' => self.parse_number(),
            c if ident_start(c) => self.parse_primary_ident(),
            _ => {
                let tok = &self.src[self.at..];
                self.syntax_error(b"operand expected", tok)
            }
        }
    }

    #[inline(never)]
    fn parse_primary_group(&mut self) -> Result<ArithId, Error> {
        self.at += 1;
        self.ctx.enter_depth(self.err_pos)?;
        let inner = self.parse_comma();
        self.ctx.leave_depth();
        let inner = inner?;
        if self.is_syntax_error(inner) {
            return Ok(inner);
        }
        self.skip_ws();
        if self.src.get(self.at) != Some(&b')') {
            return self.syntax_error(b"missing `)'", self.rest());
        }
        self.at += 1;
        Ok(self.ctx.ast.push_arith(ArithExpr::Group(inner))?)
    }

    #[inline(never)]
    fn parse_primary_single_quote(&mut self) -> Result<ArithId, Error> {
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
        self.syntax_error(b"unclosed single quote", self.rest())
    }

    #[inline(never)]
    fn parse_primary_double_quote(&mut self) -> Result<ArithId, Error> {
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
        self.ctx.enter_depth(self.err_pos)?;
        let res = parse_arith(inner, self.err_pos, self.ctx);
        self.ctx.leave_depth();
        res
    }

    #[inline(never)]
    fn parse_primary_backtick(&mut self) -> Result<ArithId, Error> {
        let first = self.parse_backtick_piece()?;
        self.maybe_concat(first)
    }

    #[inline(never)]
    fn parse_primary_dollar(&mut self) -> Result<ArithId, Error> {
        let first = self.parse_dollar_piece()?;
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
        self.maybe_concat(first)
    }

    #[inline(never)]
    fn parse_primary_ident(&mut self) -> Result<ArithId, Error> {
        let start = self.at;
        while self.src.get(self.at).is_some_and(|&c| ident_byte(c)) {
            self.at += 1;
        }
        let ident_slice = &self.src[start..self.at];
        if self.src.get(self.at) == Some(&b'[') {
            let array = self.ctx.ast.push_bytes(ident_slice)?;
            let (index, string_key) = self.parse_subscript()?;
            if self.src.get(self.at) == Some(&b'[') {
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
        self.maybe_concat(first)
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
                self.step()?;
                items.try_push(self.parse_dollar_piece()?)?;
            } else if c == b'`' {
                self.step()?;
                items.try_push(self.parse_backtick_piece()?)?;
            } else if ident_byte(c) {
                self.step()?;
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
                self.ctx.enter_depth(self.err_pos)?;
                let nested = parse_arith(inner, self.err_pos, self.ctx);
                self.ctx.leave_depth();
                let nested = nested?;
                Ok(self.ctx.ast.push_arith(ArithExpr::Nested(nested))?)
            }
            Some(b'[') => {
                // Nested `$[...]`
                let inner = self.scan_delimited(b'[', b']')?;
                self.ctx.enter_depth(self.err_pos)?;
                let nested = parse_arith(inner, self.err_pos, self.ctx);
                self.ctx.leave_depth();
                let nested = nested?;
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
        self.ctx.enter_depth(self.err_pos)?;
        let idx = parse_arith(trimmed, self.err_pos, self.ctx);
        self.ctx.leave_depth();
        let idx = idx?;
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
