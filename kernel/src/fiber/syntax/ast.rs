//! Flat-arena AST types for the L3 bash parser. Every edge is a `Copy` u32 index or `Span`,
//! so `Drop for Ast` frees flat `KVec` tables with zero call-stack recursion.
#![allow(dead_code)]
use crate::mem::{Budget, KVec, MemError};
use core::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self {
            start: start as u32,
            end: end as u32,
        }
    }

    pub const fn empty() -> Self {
        Self { start: 0, end: 0 }
    }

    pub const fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn get(self, input: &[u8]) -> &[u8] {
        &input[self.start as usize..self.end as usize]
    }

    pub const fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WordId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StmtId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CmdId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArithId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CondId(pub u32);

pub struct Ast {
    /// Interned byte pool (raw token slices, decoded ANSI-C bytes, identifiers, source text spans).
    pub bytes: KVec<u8>,
    pub stmts: KVec<Stmt>,
    pub stmt_lists: KVec<StmtId>,
    pub pipelines: KVec<Pipeline>,
    pub list_ops: KVec<ListOp>,
    pub commands: KVec<Command>,
    pub cmd_lists: KVec<CmdId>,
    pub pipe_stderr: KVec<bool>,
    pub assigns: KVec<Assign>,
    pub redirs: KVec<Redir>,
    pub words: KVec<Word>,
    pub word_lists: KVec<WordId>,
    pub parts: KVec<WordPart>,
    pub param_expands: KVec<ParamExpand>,
    pub brace_items: KVec<BraceItem>,
    pub if_clauses: KVec<IfClause>,
    pub case_items: KVec<CaseItem>,
    pub arith_nodes: KVec<ArithExpr>,
    pub arith_lists: KVec<ArithId>,
    pub cond_nodes: KVec<CondExpr>,
    /// Root script statements (`Span` into `stmt_lists`).
    pub root: Span,
}

impl Ast {
    pub fn new_in(budget: &Budget) -> Self {
        Self {
            bytes: KVec::new_in(budget),
            stmts: KVec::new_in(budget),
            stmt_lists: KVec::new_in(budget),
            pipelines: KVec::new_in(budget),
            list_ops: KVec::new_in(budget),
            commands: KVec::new_in(budget),
            cmd_lists: KVec::new_in(budget),
            pipe_stderr: KVec::new_in(budget),
            assigns: KVec::new_in(budget),
            redirs: KVec::new_in(budget),
            words: KVec::new_in(budget),
            word_lists: KVec::new_in(budget),
            parts: KVec::new_in(budget),
            param_expands: KVec::new_in(budget),
            brace_items: KVec::new_in(budget),
            if_clauses: KVec::new_in(budget),
            case_items: KVec::new_in(budget),
            arith_nodes: KVec::new_in(budget),
            arith_lists: KVec::new_in(budget),
            cond_nodes: KVec::new_in(budget),
            root: Span::empty(),
        }
    }

    pub fn push_bytes(&mut self, data: &[u8]) -> Result<Span, MemError> {
        let start = self.bytes.len();
        self.bytes.try_extend_from_slice(data)?;
        Ok(Span::new(start, self.bytes.len()))
    }

    pub fn bytes(&self, span: Span) -> &[u8] {
        span.get(&self.bytes)
    }

    pub fn push_word(&mut self, parts: Span) -> Result<WordId, MemError> {
        let id = self.words.len() as u32;
        self.words.try_push(Word { parts })?;
        Ok(WordId(id))
    }

    pub fn push_literal_word(&mut self, data: &[u8]) -> Result<WordId, MemError> {
        let span = self.push_bytes(data)?;
        let p_start = self.parts.len();
        self.parts.try_push(WordPart::Literal(span))?;
        self.push_word(Span::new(p_start, self.parts.len()))
    }

    pub fn push_cmd(&mut self, cmd: Command) -> Result<CmdId, MemError> {
        let id = self.commands.len() as u32;
        self.commands.try_push(cmd)?;
        Ok(CmdId(id))
    }

    pub fn push_stmt(&mut self, stmt: Stmt) -> Result<StmtId, MemError> {
        let id = self.stmts.len() as u32;
        self.stmts.try_push(stmt)?;
        Ok(StmtId(id))
    }

    pub fn push_arith(&mut self, expr: ArithExpr) -> Result<ArithId, MemError> {
        let id = self.arith_nodes.len() as u32;
        self.arith_nodes.try_push(expr)?;
        Ok(ArithId(id))
    }

    pub fn push_cond(&mut self, expr: CondExpr) -> Result<CondId, MemError> {
        let id = self.cond_nodes.len() as u32;
        self.cond_nodes.try_push(expr)?;
        Ok(CondId(id))
    }

    pub fn push_param(&mut self, param: ParamExpand) -> Result<u32, MemError> {
        let id = self.param_expands.len() as u32;
        self.param_expands.try_push(param)?;
        Ok(id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOp {
    And, // &&
    Or,  // ||
}

#[derive(Clone, Copy, Debug)]
pub struct Stmt {
    pub pipelines: Span, // into `Ast::pipelines`
    pub ops: Span,       // into `Ast::list_ops` (count == pipeline count - 1, saturating)
    pub background: bool,
    pub deferred_error: Option<Span>, // into `Ast::bytes`
    pub source_span: Span,            // byte span in normalized source for `set -v` (L5)
    pub line: u32,                    // 1-based start line (`0` when unset on deferred error -> reported as 1)
}

#[derive(Clone, Copy, Debug)]
pub struct Pipeline {
    pub commands: Span,    // into `Ast::cmd_lists`
    pub pipe_stderr: Span, // into `Ast::pipe_stderr` (count == command count - 1, saturating)
    pub negated: bool,
    pub timed: bool,
    pub time_posix: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Simple(SimpleCmd),
    If { clauses: Span, else_body: Option<Span>, redirs: Span },
    For { var: Span, words: Option<Span>, body: Span, redirs: Span },
    CStyleFor {
        init: Option<ArithId>,
        cond: Option<ArithId>,
        update: Option<ArithId>,
        body: Span,
        redirs: Span,
        line: u32,
    },
    While { cond: Span, body: Span, redirs: Span },
    Until { cond: Span, body: Span, redirs: Span },
    Case { word: WordId, items: Span, redirs: Span },
    Subshell { body: Span, redirs: Span },
    Group { body: Span, redirs: Span },
    FunctionDef { name: Span, body: CmdId, redirs: Span },
    Arithmetic { expr: ArithId, redirs: Span, line: u32 },
    Conditional { expr: CondId, redirs: Span, line: u32 },
}

#[derive(Clone, Copy, Debug)]
pub struct SimpleCmd {
    pub assigns: Span, // into `Ast::assigns`
    pub name: Option<WordId>,
    pub args: Span,    // into `Ast::word_lists`
    pub redirs: Span,  // into `Ast::redirs`
    pub line: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct IfClause {
    pub cond: Span, // into `Ast::stmt_lists`
    pub body: Span, // into `Ast::stmt_lists`
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseTerm {
    DoubleSemi,  // ;;
    SemiAnd,     // ;&
    SemiSemiAnd, // ;;&
}

#[derive(Clone, Copy, Debug)]
pub struct CaseItem {
    pub patterns: Span, // into `Ast::word_lists`
    pub body: Span,     // into `Ast::stmt_lists`
    pub terminator: CaseTerm,
}

#[derive(Clone, Copy, Debug)]
pub struct Assign {
    pub name: Span,              // identifier bytes in `Ast::bytes`
    pub subscript: Option<Span>, // raw subscript expression bytes (`a[expr]=...`), `None` for scalar/whole-array
    pub value: Option<WordId>,   // scalar/element rhs (`None` when `array.is_some()`)
    pub array: Option<Span>,     // into `Ast::word_lists` for `name=(...)` / `name+=(...)`
    pub append: bool,            // `+=`
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirOp {
    In,        // < (also used on resolved << and <<-)
    Out,       // >
    Append,    // >>
    DupIn,     // <&
    DupOut,    // >&
    ReadWrite, // <>
    Clobber,   // >|
    HereStr,   // <<<
    AndOut,    // &>
    AndAppend, // &>>
}

#[derive(Clone, Copy, Debug)]
pub enum RedirTarget {
    Word(WordId),
    HereDoc {
        content: WordId,
        strip_tabs: bool,
        quoted: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Redir {
    pub fd: Option<u32>,
    pub fd_var: Option<Span>, // `{varname}` without braces
    pub op: RedirOp,
    pub target: RedirTarget,
}

#[derive(Clone, Copy, Debug)]
pub struct Word {
    pub parts: Span, // into `Ast::parts`
}

#[derive(Clone, Copy, Debug)]
pub enum WordPart {
    Literal(Span),                  // raw bytes in `Ast::bytes`
    SingleQuoted(Span),             // literal bytes in `Ast::bytes`
    AnsiCQuoted(Span),              // decoded bytes from `$'...'` in `Ast::bytes`
    DoubleQuoted(Span),             // nested `WordPart` span in `Ast::parts`
    Escaped(Span),                  // escaped char/byte in `Ast::bytes`
    ParamExpand(u32),               // index into `Ast::param_expands`
    CommandSubst { body: Span, backtick: bool }, // `StmtId` list in `Ast::stmt_lists`
    ArithExpand(ArithId),           // `$((...))` or `$[...]`
    ProcessSubst { body: Span, output: bool },   // `<(...)` (`output: false`) or `>(...)` (`output: true`)
    BraceExpand(Span),              // into `Ast::brace_items`
    TildeExpand { user: Option<Span> },
    Glob { pattern: Span },
}

#[derive(Clone, Copy, Debug)]
pub struct ParamExpand {
    pub parameter: Span, // variable/special name or `arr[sub]` in `Ast::bytes`
    pub op: Option<ParamOp>,
}

#[derive(Clone, Copy, Debug)]
pub enum ParamOp {
    DefaultValue { word: WordId, check_empty: bool },
    AssignDefault { word: WordId, check_empty: bool },
    ErrorIfUnset { word: Option<WordId>, check_empty: bool },
    UseAlternative { word: WordId, check_empty: bool },
    Length,
    LengthSliceError,               // deferred `${#var:...}` -> runtime bad substitution on `var`
    BadSubstitution { text: Span }, // deferred `${...}` -> runtime `bash: <text>: bad substitution`
    Substring { offset: ArithId, length: Option<ArithId> },
    PatternRemoval { pattern: WordId, side: RemovalSide, greedy: bool },
    PatternReplacement {
        pattern: WordId,
        replacement: Option<WordId>,
        all: bool,
        anchor: Option<ReplaceAnchor>,
    },
    CaseModification { direction: CaseDir, all: bool, pattern: Option<WordId> },
    Transform { operator: u8 },             // Q, E, P, A, K, a, k, u, U, L
    Indirection { inner_op: Option<u32> },  // optional chained index into `Ast::param_expands`
    ArrayKeys { array: Span, star: bool },  // `${!arr[@]}` / `${!arr[*]}`
    VarNamePrefix { prefix: Span, star: bool }, // `${!prefix*}` / `${!prefix@}`
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalSide {
    Prefix,
    Suffix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceAnchor {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseDir {
    Upper,
    Lower,
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub enum BraceItem {
    Word(WordId),
    Range {
        start_num: Option<i64>,
        end_num: Option<i64>,
        start_char: Option<u8>,
        end_char: Option<u8>,
        step: Option<i64>,
        raw: Span, // raw `{...}` fallback when step/case fails at expansion time
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ArithExpr {
    Number(i64),
    NumberOverflow, // maps to JS NaN -> 0 / runtime handling in L4
    Var { name: Span, has_dollar: bool },
    SpecialVar { name: Span },
    Binary { op: ArithBinOp, left: ArithId, right: ArithId },
    Unary { op: ArithUnaryOp, operand: ArithId, prefix: bool },
    Ternary { cond: ArithId, consequent: ArithId, alternate: ArithId },
    Assign {
        op: ArithAssignOp,
        var: Span,
        subscript: Option<ArithId>,
        string_key: Option<Span>,
        value: ArithId,
    },
    Group(ArithId),
    Nested(ArithId), // `$((...))` or `$[...]` inside arithmetic
    CommandSubst { body: Span, backtick: bool },
    BracedExp(Span), // raw `${...}` content inside arithmetic for L4
    DynamicBase { base_expr: Span, value: Span },
    DynamicNumber { prefix: Span, suffix: Span },
    ArrayElement { array: Span, index: Option<ArithId>, string_key: Option<Span> },
    NumberSubscript { number: Span, error_token: Span }, // deferred `1[2]` error
    DoubleSubscript { array: Span, index: ArithId },     // deferred `a[1][1]` error
    Concat(Span),                                        // `Span` into `Ast::arith_lists`
    SingleQuote { content: Span, value: i64 },           // `'a'` inside `$((...))` vs error in `((...))`
    SyntaxError { message: Span, error_token: Span },    // deferred runtime syntax error
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithBinOp {
    Add, Sub, Mul, Div, Mod, Pow,
    Shl, Shr, BitAnd, BitOr, BitXor,
    Lt, Le, Gt, Ge, Eq, Ne,
    LogAnd, LogOr, Comma,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithUnaryOp {
    Neg, Pos, LogNot, BitNot, Inc, Dec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithAssignOp {
    Assign, Add, Sub, Mul, Div, Mod, Shl, Shr, BitAnd, BitOr, BitXor,
}

#[derive(Clone, Copy, Debug)]
pub enum CondExpr {
    Or { left: CondId, right: CondId },
    And { left: CondId, right: CondId },
    Not(CondId),
    Group(CondId),
    Unary { op: CondUnaryOp, operand: WordId },
    Binary { op: CondBinOp, left: WordId, right: WordId },
    Word(WordId), // non-empty check (`-n`)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondUnaryOp {
    ExistsA, BlockB, CharC, DirD, ExistsE, FileF, SetgidG, SymlinkH, StickyK,
    SymlinkL, NonEmptyN, OptionO, PipeP, ReadableR, NonZeroSizeS, TerminalT,
    SetuidU, VarSetV, WritableW, ExecutableX, EmptyZ, ModifiedN, OwnUidO, OwnGidG, SocketS,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondBinOp {
    StrEq,      // == or =
    StrNe,      // !=
    RegexMatch, // =~
    StrLt,      // <
    StrGt,      // >
    Eq, Ne, Lt, Le, Gt, Ge, // -eq, -ne, -lt, -le, -gt, -ge
    Nt, Ot, Ef,             // -nt, -ot, -ef
}
