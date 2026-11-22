//! Native Rust AST unit tests (>= 60 tests covering all 12 command kinds, all 10 redir ops +
//! fd_var + heredoc variants, scalar/append/subscript/array assign, all 15 ParamOp variants,
//! BraceItem, all ArithExpr variants, all CondExpr ops, and seeded parser fuzzing).
use super::*;
use crate::mem::{Budget, LimitId};

const CEIL: u64 = 1 << 30;

fn parse_ok(src: &str) -> Ast {
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let mut charge = |_: u64| Ok(());
    Ast::parse(src.as_bytes(), 1 << 20, &b, &mut charge).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
}

fn parse_err(src: &str) -> std::string::String {
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let mut charge = |_: u64| Ok(());
    let err = Ast::parse(src.as_bytes(), 1 << 20, &b, &mut charge).err().expect("expected error");
    std::format!("{err}")
}

fn first_cmd(ast: &Ast) -> Command {
    let stmt = ast.stmts[ast.stmt_lists[ast.root.start as usize].0 as usize];
    let pipe = ast.pipelines[stmt.pipelines.start as usize];
    ast.commands[ast.cmd_lists[pipe.commands.start as usize].0 as usize]
}

fn first_arg_word(ast: &Ast) -> Word {
    let Command::Simple(sc) = first_cmd(ast) else { panic!("expected Simple") };
    ast.words[ast.word_lists[sc.args.start as usize].0 as usize]
}

fn first_arg_param(ast: &Ast) -> ParamExpand {
    let w = first_arg_word(ast);
    let WordPart::ParamExpand(idx) = ast.parts[w.parts.start as usize] else { panic!("expected ParamExpand") };
    ast.param_expands[idx as usize]
}

fn first_arith(ast: &Ast) -> ArithExpr {
    let Command::Arithmetic { expr, .. } = first_cmd(ast) else { panic!("expected Arithmetic") };
    ast.arith_nodes[expr.0 as usize]
}

fn first_cond(ast: &Ast) -> CondExpr {
    let Command::Conditional { expr, .. } = first_cmd(ast) else { panic!("expected Conditional") };
    ast.cond_nodes[expr.0 as usize]
}

// --- 12 Command kinds ---
#[test]
fn cmd_simple() {
    let ast = parse_ok("echo hi");
    assert!(matches!(first_cmd(&ast), Command::Simple(s) if s.name.is_some() && s.args.len() == 1));
}
#[test]
fn cmd_if_elif_else() {
    let ast = parse_ok("if a; then b; elif c; then d; else e; fi");
    assert!(matches!(first_cmd(&ast), Command::If { clauses, else_body, .. } if clauses.len() == 2 && else_body.is_some()));
}
#[test]
fn cmd_for_words() {
    let ast = parse_ok("for x in a b; do echo $x; done");
    assert!(matches!(first_cmd(&ast), Command::For { var, words: Some(w), .. } if ast.bytes(var) == b"x" && w.len() == 2));
}
#[test]
fn cmd_for_default_positional() {
    let ast = parse_ok("for p; do echo $p; done");
    assert!(matches!(first_cmd(&ast), Command::For { words: None, .. }));
}
#[test]
fn cmd_c_style_for() {
    let ast = parse_ok("for ((i=0; i<3; i++)); do echo $i; done");
    assert!(matches!(first_cmd(&ast), Command::CStyleFor { init: Some(_), cond: Some(_), update: Some(_), .. }));
}
#[test]
fn cmd_while() {
    let ast = parse_ok("while true; do echo 1; done");
    assert!(matches!(first_cmd(&ast), Command::While { .. }));
}
#[test]
fn cmd_until() {
    let ast = parse_ok("until false; do echo 1; done");
    assert!(matches!(first_cmd(&ast), Command::Until { .. }));
}
#[test]
fn cmd_case_all_terminators() {
    let ast = parse_ok("case $x in a|b) echo 1 ;; c) echo 2 ;& (d) echo 3 ;;& esac");
    let Command::Case { items, .. } = first_cmd(&ast) else { panic!() };
    assert_eq!(items.len(), 3);
    assert_eq!(ast.case_items[items.start as usize].terminator, CaseTerm::DoubleSemi);
    assert_eq!(ast.case_items[items.start as usize + 1].terminator, CaseTerm::SemiAnd);
    assert_eq!(ast.case_items[items.start as usize + 2].terminator, CaseTerm::SemiSemiAnd);
}
#[test]
fn cmd_subshell() {
    let ast = parse_ok("( echo a )");
    assert!(matches!(first_cmd(&ast), Command::Subshell { body, .. } if body.len() == 1));
}
#[test]
fn cmd_group() {
    let ast = parse_ok("{ echo a; }");
    assert!(matches!(first_cmd(&ast), Command::Group { body, .. } if body.len() == 1));
}
#[test]
fn cmd_function_def_keyword_and_parens() {
    let ast = parse_ok("function foo-bar { echo 1; }\nbaz:1() ( echo 2 )");
    assert_eq!(ast.stmt_lists[ast.root.range()].len(), 2);
    assert!(matches!(first_cmd(&ast), Command::FunctionDef { name, .. } if ast.bytes(name) == b"foo-bar"));
}
#[test]
fn cmd_arithmetic() {
    let ast = parse_ok("(( x = 1 + 2 ))");
    assert!(matches!(first_cmd(&ast), Command::Arithmetic { .. }));
}
#[test]
fn cmd_conditional() {
    let ast = parse_ok("[[ -f /a && a == b ]]");
    assert!(matches!(first_cmd(&ast), Command::Conditional { .. }));
}

// --- Pipelines & Lists ---
#[test]
fn pipeline_time_and_negate() {
    let ast = parse_ok("time -p ! echo a |& echo b && echo c || echo d &");
    let stmt = ast.stmts[ast.stmt_lists[ast.root.start as usize].0 as usize];
    assert!(stmt.background);
    assert_eq!(stmt.pipelines.len(), 3);
    assert_eq!(ast.list_ops[stmt.ops.range()], [ListOp::And, ListOp::Or]);
    let p0 = ast.pipelines[stmt.pipelines.start as usize];
    assert!(p0.timed && p0.time_posix && p0.negated);
    assert_eq!(ast.pipe_stderr[p0.pipe_stderr.range()], [true]);
}

// --- All 10 RedirOp variants + fd_var + heredoc variants ---
#[test]
fn redir_in_out_append() {
    let ast = parse_ok("cmd <a >b >>c");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let r = &ast.redirs[s.redirs.range()];
    assert_eq!(r.len(), 3);
    assert_eq!((r[0].op, r[1].op, r[2].op), (RedirOp::In, RedirOp::Out, RedirOp::Append));
}
#[test]
fn redir_dup_rw_clobber() {
    let ast = parse_ok("cmd 0<&3 2>&1 4<>rw >|clob");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let r = &ast.redirs[s.redirs.range()];
    assert_eq!(r[0].fd, Some(0));
    assert_eq!(r[0].op, RedirOp::DupIn);
    assert_eq!(r[1].fd, Some(2));
    assert_eq!(r[1].op, RedirOp::DupOut);
    assert_eq!(r[2].fd, Some(4));
    assert_eq!(r[2].op, RedirOp::ReadWrite);
    assert_eq!(r[3].op, RedirOp::Clobber);
}
#[test]
fn redir_herestr_andout_andappend() {
    let ast = parse_ok("cmd <<<hi &>all &>>more");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let r = &ast.redirs[s.redirs.range()];
    assert_eq!((r[0].op, r[1].op, r[2].op), (RedirOp::HereStr, RedirOp::AndOut, RedirOp::AndAppend));
}
#[test]
fn redir_fd_variable() {
    let ast = parse_ok("cmd {myfd}>out");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let r = ast.redirs[s.redirs.start as usize];
    assert_eq!(ast.bytes(r.fd_var.unwrap()), b"myfd");
    assert_eq!(r.op, RedirOp::Out);
}
#[test]
fn redir_heredoc_unquoted_quoted_strip() {
    let ast = parse_ok("cat <<EOF <<'Q' <<-T\nhello $X\nEOF\nlit $X\nQ\n\tindented\n\tT\n");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let r = &ast.redirs[s.redirs.range()];
    assert_eq!(r.len(), 3);
    assert!(matches!(r[0].target, RedirTarget::HereDoc { strip_tabs: false, quoted: false, .. }));
    assert!(matches!(r[1].target, RedirTarget::HereDoc { strip_tabs: false, quoted: true, .. }));
    assert!(matches!(r[2].target, RedirTarget::HereDoc { strip_tabs: true, quoted: false, .. }));
}
#[test]
fn redir_non_adjacent_number_is_arg() {
    let ast = parse_ok("echo 2 > out");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    assert_eq!(s.args.len(), 1);
    assert_eq!(ast.redirs[s.redirs.start as usize].fd, None);
}

// --- Assign forms (scalar, append, subscript, array) ---
#[test]
fn assign_scalar_and_append() {
    let ast = parse_ok("a=1 b+=2");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let asg = &ast.assigns[s.assigns.range()];
    assert_eq!(asg.len(), 2);
    assert!(!asg[0].append && asg[0].value.is_some() && asg[0].subscript.is_none());
    assert!(asg[1].append && asg[1].value.is_some());
}
#[test]
fn assign_subscript() {
    let ast = parse_ok("a[1 + 2]=val");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let a = ast.assigns[s.assigns.start as usize];
    assert_eq!(ast.bytes(a.name), b"a");
    assert_eq!(ast.bytes(a.subscript.unwrap()), b"1 + 2");
    assert!(a.value.is_some());
}
#[test]
fn assign_array_and_append() {
    let ast = parse_ok("arr=(one \"two three\") arr+=(four)");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let asg = &ast.assigns[s.assigns.range()];
    assert_eq!(asg[0].array.unwrap().len(), 2);
    assert!(!asg[0].append);
    assert_eq!(asg[1].array.unwrap().len(), 1);
    assert!(asg[1].append);
}

// --- All 15 ParamOp variants ---
#[test]
fn param_default_value() {
    let ast = parse_ok("echo ${x:-def} ${y-def}");
    let p = first_arg_param(&ast);
    assert!(matches!(p.op, Some(ParamOp::DefaultValue { check_empty: true, .. })));
}
#[test]
fn param_assign_default() {
    let ast = parse_ok("echo ${x:=def}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::AssignDefault { check_empty: true, .. })));
}
#[test]
fn param_error_if_unset() {
    let ast = parse_ok("echo ${x:?err}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::ErrorIfUnset { word: Some(_), check_empty: true })));
}
#[test]
fn param_use_alternative() {
    let ast = parse_ok("echo ${x:+alt}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::UseAlternative { check_empty: true, .. })));
}
#[test]
fn param_length() {
    let ast = parse_ok("echo ${#x}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::Length)));
}
#[test]
fn param_length_slice_error() {
    let ast = parse_ok("echo ${#x:1}");
    let p = first_arg_param(&ast);
    assert_eq!(ast.bytes(p.parameter), b"x");
    assert!(matches!(p.op, Some(ParamOp::LengthSliceError)));
}
#[test]
fn param_bad_substitution_deferred() {
    let ast = parse_ok("echo ${%}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::BadSubstitution { .. })));
}
#[test]
fn param_substring() {
    let ast = parse_ok("echo ${x:1:3}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::Substring { length: Some(_), .. })));
}
#[test]
fn param_pattern_removal() {
    let ast = parse_ok("echo ${x##a*}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::PatternRemoval { side: RemovalSide::Prefix, greedy: true, .. })));
    let ast2 = parse_ok("echo ${x%b*}");
    assert!(matches!(first_arg_param(&ast2).op, Some(ParamOp::PatternRemoval { side: RemovalSide::Suffix, greedy: false, .. })));
}
#[test]
fn param_pattern_replacement() {
    let ast = parse_ok("echo ${x//a/b}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::PatternReplacement { all: true, anchor: None, replacement: Some(_), .. })));
    let ast2 = parse_ok("echo ${x/#a/b}");
    assert!(matches!(first_arg_param(&ast2).op, Some(ParamOp::PatternReplacement { anchor: Some(ReplaceAnchor::Start), .. })));
    let ast3 = parse_ok("echo ${x/%a/b}");
    assert!(matches!(first_arg_param(&ast3).op, Some(ParamOp::PatternReplacement { anchor: Some(ReplaceAnchor::End), .. })));
}
#[test]
fn param_case_modification() {
    let ast = parse_ok("echo ${x^^}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::CaseModification { direction: CaseDir::Upper, all: true, .. })));
    let ast2 = parse_ok("echo ${x,a}");
    assert!(matches!(first_arg_param(&ast2).op, Some(ParamOp::CaseModification { direction: CaseDir::Lower, all: false, pattern: Some(_) })));
    let ast3 = parse_ok("echo ${x~~}");
    assert!(matches!(first_arg_param(&ast3).op, Some(ParamOp::CaseModification { direction: CaseDir::Toggle, all: true, .. })));
}
#[test]
fn param_transform() {
    let ast = parse_ok("echo ${x@Q}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::Transform { operator: b'Q' })));
}
#[test]
fn param_indirection() {
    let ast = parse_ok("echo ${!ref}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::Indirection { inner_op: None })));
}
#[test]
fn param_array_keys() {
    let ast = parse_ok("echo ${!arr[@]}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::ArrayKeys { star: false, .. })));
}
#[test]
fn param_var_name_prefix() {
    let ast = parse_ok("echo ${!PRE*}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::VarNamePrefix { star: true, .. })));
}

// --- Brace, Tilde, Glob, CommandSubst, ProcessSubst, AnsiC ---
#[test]
fn word_brace_items_and_ranges() {
    let ast = parse_ok("echo {a,b}{1..5..2}{a..z}");
    let w = first_arg_word(&ast);
    let parts = &ast.parts[w.parts.range()];
    assert_eq!(parts.len(), 3);
    let WordPart::BraceExpand(b0) = parts[0] else { panic!() };
    assert_eq!(b0.len(), 2);
    assert!(matches!(ast.brace_items[b0.start as usize], BraceItem::Word(_)));
    let WordPart::BraceExpand(b1) = parts[1] else { panic!() };
    assert!(matches!(ast.brace_items[b1.start as usize], BraceItem::Range { start_num: Some(1), end_num: Some(5), step: Some(2), .. }));
    let WordPart::BraceExpand(b2) = parts[2] else { panic!() };
    assert!(matches!(ast.brace_items[b2.start as usize], BraceItem::Range { start_char: Some(b'a'), end_char: Some(b'z'), .. }));
}
#[test]
fn word_tilde_glob_extglob() {
    let ast = parse_ok("echo ~/dir *.txt a+(b|c) [a-z]");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let args = &ast.word_lists[s.args.range()];
    assert!(matches!(ast.parts[ast.words[args[0].0 as usize].parts.start as usize], WordPart::TildeExpand { user: None }));
    assert!(matches!(ast.parts[ast.words[args[1].0 as usize].parts.start as usize], WordPart::Glob { .. }));
    assert!(matches!(ast.parts[ast.words[args[3].0 as usize].parts.start as usize], WordPart::Glob { .. }));
}
#[test]
fn word_cmd_subst_and_proc_subst() {
    let ast = parse_ok("echo $(echo a) `echo b` <(echo c) >(echo d)");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let args = &ast.word_lists[s.args.range()];
    assert!(matches!(ast.parts[ast.words[args[0].0 as usize].parts.start as usize], WordPart::CommandSubst { backtick: false, .. }));
    assert!(matches!(ast.parts[ast.words[args[1].0 as usize].parts.start as usize], WordPart::CommandSubst { backtick: true, .. }));
    assert!(matches!(ast.parts[ast.words[args[2].0 as usize].parts.start as usize], WordPart::ProcessSubst { output: false, .. }));
    assert!(matches!(ast.parts[ast.words[args[3].0 as usize].parts.start as usize], WordPart::ProcessSubst { output: true, .. }));
}
#[test]
fn word_ansic_decoded() {
    let ast = parse_ok("echo $'a\\nb\\x41\\c'");
    let w = first_arg_word(&ast);
    let WordPart::AnsiCQuoted(s) = ast.parts[w.parts.start as usize] else { panic!() };
    assert_eq!(ast.bytes(s), b"a\nbA\x07");
}

// --- ArithExpr variants & bases ---
#[test]
fn arith_bases_2_8_16_36_64() {
    assert!(matches!(first_arith(&parse_ok("(( 2#1010 ))")), ArithExpr::Number(10)));
    assert!(matches!(first_arith(&parse_ok("(( 077 ))")), ArithExpr::Number(63)));
    assert!(matches!(first_arith(&parse_ok("(( 0xff ))")), ArithExpr::Number(255)));
    assert!(matches!(first_arith(&parse_ok("(( 36#z ))")), ArithExpr::Number(35)));
    assert!(matches!(first_arith(&parse_ok("(( 64#_ ))")), ArithExpr::Number(63)));
    assert!(matches!(first_arith(&parse_ok("(( 64#@ ))")), ArithExpr::Number(62)));
    assert!(matches!(first_arith(&parse_ok("(( 64#A ))")), ArithExpr::Number(36)));
}
#[test]
fn arith_number_overflow() {
    assert!(matches!(first_arith(&parse_ok("(( 99999999999999999999999999 ))")), ArithExpr::NumberOverflow));
}
#[test]
fn arith_precedence_and_ternary() {
    let ast = parse_ok("(( a ? b + c * d ** 2 : e, f += 1 ))");
    assert!(matches!(first_arith(&ast), ArithExpr::Binary { op: ArithBinOp::Comma, .. }));
}
#[test]
fn arith_unary_and_postfix() {
    let ast = parse_ok("(( ++x ))");
    assert!(matches!(first_arith(&ast), ArithExpr::Unary { op: ArithUnaryOp::Inc, prefix: true, .. }));
    let ast2 = parse_ok("(( x-- ))");
    assert!(matches!(first_arith(&ast2), ArithExpr::Unary { op: ArithUnaryOp::Dec, prefix: false, .. }));
}
#[test]
fn arith_assign_array_element() {
    let ast = parse_ok("(( a[1] <<= 2 ))");
    assert!(matches!(first_arith(&ast), ArithExpr::Assign { op: ArithAssignOp::Shl, subscript: Some(_), .. }));
}
#[test]
fn arith_group_nested_cmdsubst_braced() {
    assert!(matches!(first_arith(&parse_ok("(( (1) ))")), ArithExpr::Group(_)));
    assert!(matches!(first_arith(&parse_ok("(( $((1)) ))")), ArithExpr::Nested(_)));
    assert!(matches!(first_arith(&parse_ok("(( $(echo 1) ))")), ArithExpr::CommandSubst { backtick: false, .. }));
    assert!(matches!(first_arith(&parse_ok("(( ${x} ))")), ArithExpr::BracedExp(_)));
}
#[test]
fn arith_dynamic_base_and_number() {
    assert!(matches!(first_arith(&parse_ok("(( $b#10 ))")), ArithExpr::DynamicBase { .. }));
    assert!(matches!(first_arith(&parse_ok("(( 12$x ))")), ArithExpr::DynamicNumber { .. }));
}
#[test]
fn arith_number_subscript_and_double_subscript() {
    assert!(matches!(first_arith(&parse_ok("(( 1[2] ))")), ArithExpr::NumberSubscript { .. }));
    assert!(matches!(first_arith(&parse_ok("(( a[1][2] ))")), ArithExpr::DoubleSubscript { .. }));
}
#[test]
fn arith_single_quote_concat_and_syntax_error() {
    assert!(matches!(first_arith(&parse_ok("(( 'A' ))")), ArithExpr::SingleQuote { value: 65, .. }));
    assert!(matches!(first_arith(&parse_ok("(( $a$b ))")), ArithExpr::Concat(_)));
    assert!(matches!(first_arith(&parse_ok("(( 1 + ))")), ArithExpr::SyntaxError { .. }));
}
#[test]
fn arith_legacy_bracket_expand() {
    let ast = parse_ok("echo $[1 + 2]");
    let w = first_arg_word(&ast);
    assert!(matches!(ast.parts[w.parts.start as usize], WordPart::ArithExpand(_)));
}

// --- CondExpr unary, binary, =~, !(...) ---
#[test]
fn cond_logical_and_group() {
    let ast = parse_ok("[[ ! ( -f /a || -d /b ) && -n x ]]");
    assert!(matches!(first_cond(&ast), CondExpr::And { .. }));
}
#[test]
fn cond_all_unary_ops() {
    let ops = [
        ("-a", CondUnaryOp::ExistsA), ("-b", CondUnaryOp::BlockB), ("-c", CondUnaryOp::CharC),
        ("-d", CondUnaryOp::DirD), ("-e", CondUnaryOp::ExistsE), ("-f", CondUnaryOp::FileF),
        ("-g", CondUnaryOp::SetgidG), ("-h", CondUnaryOp::SymlinkH), ("-k", CondUnaryOp::StickyK),
        ("-L", CondUnaryOp::SymlinkL), ("-n", CondUnaryOp::NonEmptyN), ("-o", CondUnaryOp::OptionO),
        ("-p", CondUnaryOp::PipeP), ("-r", CondUnaryOp::ReadableR), ("-s", CondUnaryOp::NonZeroSizeS),
        ("-t", CondUnaryOp::TerminalT), ("-u", CondUnaryOp::SetuidU), ("-v", CondUnaryOp::VarSetV),
        ("-w", CondUnaryOp::WritableW), ("-x", CondUnaryOp::ExecutableX), ("-z", CondUnaryOp::EmptyZ),
        ("-N", CondUnaryOp::ModifiedN), ("-O", CondUnaryOp::OwnUidO), ("-G", CondUnaryOp::OwnGidG),
        ("-S", CondUnaryOp::SocketS),
    ];
    for (flag, expected) in ops {
        let ast = parse_ok(&std::format!("[[ {flag} arg ]]"));
        assert!(matches!(first_cond(&ast), CondExpr::Unary { op, .. } if op == expected), "failed for {flag}");
    }
}
#[test]
fn cond_all_binary_ops() {
    let ops = [
        ("==", CondBinOp::StrEq), ("=", CondBinOp::StrEq), ("!=", CondBinOp::StrNe),
        ("=~", CondBinOp::RegexMatch), ("<", CondBinOp::StrLt), (">", CondBinOp::StrGt),
        ("-eq", CondBinOp::Eq), ("-ne", CondBinOp::Ne), ("-lt", CondBinOp::Lt),
        ("-le", CondBinOp::Le), ("-gt", CondBinOp::Gt), ("-ge", CondBinOp::Ge),
        ("-nt", CondBinOp::Nt), ("-ot", CondBinOp::Ot), ("-ef", CondBinOp::Ef),
    ];
    for (sym, expected) in ops {
        let ast = parse_ok(&std::format!("[[ a {sym} b ]]"));
        assert!(matches!(first_cond(&ast), CondExpr::Binary { op, .. } if op == expected), "failed for {sym}");
    }
}
#[test]
fn cond_regex_and_extglob_negation() {
    let ast = parse_ok("[[ $a =~ ^(x|y)+$ && $b == !(foo|bar) ]]");
    let CondExpr::And { left, right } = first_cond(&ast) else { panic!() };
    assert!(matches!(ast.cond_nodes[left.0 as usize], CondExpr::Binary { op: CondBinOp::RegexMatch, .. }));
    assert!(matches!(ast.cond_nodes[right.0 as usize], CondExpr::Binary { op: CondBinOp::StrEq, .. }));
}
#[test]
fn cond_single_word() {
    let ast = parse_ok("[[ hello ]]");
    assert!(matches!(first_cond(&ast), CondExpr::Word(_)));
}

// --- Deferred errors, caps, normalization, diagnostics ---
#[test]
fn deferred_root_rbrace_and_rparen() {
    let ast = parse_ok("echo ok\n}");
    assert_eq!(ast.stmt_lists[ast.root.range()].len(), 2);
    let s1 = ast.stmts[ast.stmt_lists[ast.root.start as usize + 1].0 as usize];
    assert_eq!(ast.bytes(s1.deferred_error.unwrap()), b"syntax error near unexpected token `}'");
}
#[test]
fn normalize_multi_heredoc_queue() {
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let src = "  cat <<A <<B\n    bodyA\nA\n    bodyB\nB\n  echo end";
    let norm = normalize(src.as_bytes(), &b).unwrap();
    assert_eq!(norm.as_slice(), b"cat <<A <<B\n    bodyA\nA\n    bodyB\nB\necho end");
}
#[test]
fn parse_error_diagnostics() {
    assert!(parse_err("if ; then echo a; fi").contains("Expected THEN, got SEMICOLON"));
    assert!(parse_err("echo ${#x-y}").contains("${#x-y}: bad substitution"));
    assert!(parse_err("echo ${x").contains("Parse error at 0:0: unexpected EOF while looking for matching '}'"));
    assert!(parse_err("[[ -f ]]").contains("Expected operand after -f"));
    assert!(parse_err("[[ a =~ ]]").contains("Expected regex pattern after =~"));
    assert!(parse_err("[[ a == ]]").contains("Expected DBRACK_END, got EOF"));
}
#[test]
fn depth_cap_200_vs_201() {
    let ok_s = std::format!("{}echo ok;{}", "{ ".repeat(200), " }".repeat(200));
    let _ = parse_ok(&ok_s);
    let err_s = std::format!("{}echo ok;{}", "{ ".repeat(201), " }".repeat(201));
    assert!(parse_err(&err_s).contains("Maximum parser nesting depth exceeded (200)"));
}
#[test]
fn arena_sizes_and_flat_drop() {
    let sz_cmd = core::mem::size_of::<Command>();
    let sz_stmt = core::mem::size_of::<Stmt>();
    let sz_part = core::mem::size_of::<WordPart>();
    let sz_arith = core::mem::size_of::<ArithExpr>();
    let sz_cond = core::mem::size_of::<CondExpr>();
    std::println!("SIZES Command={sz_cmd} Stmt={sz_stmt} WordPart={sz_part} ArithExpr={sz_arith} CondExpr={sz_cond}");
    assert!(sz_cmd <= 64);
    assert!(sz_stmt <= 64);
    assert!(sz_part <= 32);
    assert!(sz_arith <= 64);
    assert!(sz_cond <= 24);
}
#[test]
fn work_limit_stops_parse() {
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let mut left = 5u64;
    let mut charge = |u: u64| {
        if left < u {
            Err(())
        } else {
            left -= u;
            Ok(())
        }
    };
    let res = Ast::parse(b"echo 1 2 3 4 5 6 7 8 9", 1 << 20, &b, &mut charge);
    assert!(matches!(res, Err(Error::Work)));
}

#[test]
fn param_empty_and_specials() {
    let ast = parse_ok("echo ${} ${#} ${!} ${@} ${*}");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    assert_eq!(s.args.len(), 5);
}
#[test]
fn param_indirection_chained() {
    let ast = parse_ok("echo ${!ref:-fallback}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::Indirection { inner_op: Some(_) })));
}
#[test]
fn param_transform_all_valid_ops() {
    for op in ['Q', 'E', 'P', 'A', 'K', 'a', 'k', 'u', 'U', 'L'] {
        let ast = parse_ok(&std::format!("echo ${{x@{op}}}"));
        assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::Transform { operator }) if operator == op as u8));
    }
    let bad = parse_ok("echo ${x@Z}");
    assert!(matches!(first_arg_param(&bad).op, Some(ParamOp::BadSubstitution { .. })));
}
#[test]
fn param_multi_subscript_deferred_bad_subst() {
    let ast = parse_ok("echo ${a[0][1]}");
    assert!(matches!(first_arg_param(&ast).op, Some(ParamOp::BadSubstitution { .. })));
}
#[test]
fn param_parse_time_bad_subst_variants() {
    assert!(parse_err("echo ${#x-y}").contains("${#x-y}: bad substitution"));
    assert!(parse_err("echo ${#x/a/b}").contains("${#x/a/b}: bad substitution"));
    assert!(parse_err("echo ${#x#p}").contains("${#x#p}: bad substitution"));
    assert!(parse_err("echo ${x!y}").contains("${x!y}: bad substitution"));
}
#[test]
fn word_double_quoted_and_locale_quoted() {
    let ast = parse_ok("echo \"hi $x\" $\"loc $y\"");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let args = &ast.word_lists[s.args.range()];
    assert!(matches!(ast.parts[ast.words[args[0].0 as usize].parts.start as usize], WordPart::DoubleQuoted(_)));
    assert!(matches!(ast.parts[ast.words[args[1].0 as usize].parts.start as usize], WordPart::DoubleQuoted(_)));
}
#[test]
fn word_single_quoted_and_escaped() {
    let ast = parse_ok("echo 'abc' a\\ b");
    let Command::Simple(s) = first_cmd(&ast) else { panic!() };
    let args = &ast.word_lists[s.args.range()];
    assert!(matches!(ast.parts[ast.words[args[0].0 as usize].parts.start as usize], WordPart::SingleQuoted(_)));
    let w1 = ast.words[args[1].0 as usize];
    assert!(ast.parts[w1.parts.range()].iter().any(|p| matches!(p, WordPart::Escaped(_))));
}
#[test]
fn word_tilde_with_user() {
    let ast = parse_ok("echo ~alice/bin");
    let w = first_arg_word(&ast);
    assert!(matches!(ast.parts[w.parts.start as usize], WordPart::TildeExpand { user: Some(u) } if ast.bytes(u) == b"alice"));
}
#[test]
fn arith_all_binary_and_assign_ops() {
    let ast = parse_ok("(( a = (1 | 2 ^ 3 & 4) == (5 != 6) && (7 < 8) || (9 <= 10) + (11 > 12) - (13 >= 14) * (15 << 1) / (16 >> 1) % 3 ))");
    assert!(matches!(first_arith(&ast), ArithExpr::Assign { op: ArithAssignOp::Assign, .. }));
    for (op_s, expected) in [
        ("+=", ArithAssignOp::Add), ("-=", ArithAssignOp::Sub), ("*=", ArithAssignOp::Mul),
        ("/=", ArithAssignOp::Div), ("%=", ArithAssignOp::Mod), ("<<=", ArithAssignOp::Shl),
        (">>=", ArithAssignOp::Shr), ("&=", ArithAssignOp::BitAnd), ("|=", ArithAssignOp::BitOr),
        ("^=", ArithAssignOp::BitXor),
    ] {
        let a = parse_ok(&std::format!("(( x {op_s} 1 ))"));
        assert!(matches!(first_arith(&a), ArithExpr::Assign { op, .. } if op == expected));
    }
}
#[test]
fn arith_all_unary_ops_and_special_var() {
    assert!(matches!(first_arith(&parse_ok("(( -x ))")), ArithExpr::Unary { op: ArithUnaryOp::Neg, prefix: true, .. }));
    assert!(matches!(first_arith(&parse_ok("(( +x ))")), ArithExpr::Unary { op: ArithUnaryOp::Pos, prefix: true, .. }));
    assert!(matches!(first_arith(&parse_ok("(( !x ))")), ArithExpr::Unary { op: ArithUnaryOp::LogNot, prefix: true, .. }));
    assert!(matches!(first_arith(&parse_ok("(( ~x ))")), ArithExpr::Unary { op: ArithUnaryOp::BitNot, prefix: true, .. }));
    assert!(matches!(first_arith(&parse_ok("(( $? ))")), ArithExpr::SpecialVar { .. }));
}
#[test]
fn arith_assoc_string_key_and_backtick() {
    assert!(matches!(first_arith(&parse_ok("(( a['key'] ))")), ArithExpr::ArrayElement { string_key: Some(_), .. }));
    assert!(matches!(first_arith(&parse_ok("(( `echo 1` ))")), ArithExpr::CommandSubst { backtick: true, .. }));
}
#[test]
fn c_style_for_partial_clauses_and_brace_body() {
    let ast = parse_ok("for (( ; ; )) { echo x; }");
    assert!(matches!(first_cmd(&ast), Command::CStyleFor { init: None, cond: None, update: None, .. }));
}
#[test]
fn dparen_vs_nested_subshells() {
    let ast = parse_ok("((echo a) && (echo b))");
    assert!(matches!(first_cmd(&ast), Command::Subshell { .. }));
    let deep = std::format!("{}echo deep{}", "(".repeat(201), ")".repeat(201));
    assert!(parse_err(&deep).contains("Parse error at 1:202: Maximum parser nesting depth exceeded (200)"));
}
#[test]
fn trailing_ops_produce_empty_simple_cmd() {
    let ast = parse_ok("!");
    let stmt = ast.stmts[ast.stmt_lists[ast.root.start as usize].0 as usize];
    assert!(ast.pipelines[stmt.pipelines.start as usize].negated);
    assert!(matches!(first_cmd(&ast), Command::Simple(s) if s.name.is_none()));
    let _ = parse_ok("echo a &&");
    let _ = parse_ok("echo a |");
}
#[test]
fn function_with_compound_bodies_and_redirs() {
    let ast = parse_ok("f() if true; then echo 1; fi >/dev/null");
    let Command::FunctionDef { body, redirs, .. } = first_cmd(&ast) else { panic!() };
    assert!(matches!(ast.commands[body.0 as usize], Command::If { .. }));
    assert!(redirs.len() == 0 || redirs.len() == 1);
}
#[test]
fn cond_negation_and_unary_as_binary_lhs() {
    let ast = parse_ok("[[ -f == -f ]]");
    assert!(matches!(first_cond(&ast), CondExpr::Binary { op: CondBinOp::StrEq, .. }));
    let ast2 = parse_ok("[[ ! -z x ]]");
    assert!(matches!(first_cond(&ast2), CondExpr::Not(_)));
}
#[test]
fn deferred_rparen_not_deferred_inside_if() {
    let err = parse_err("if echo ok; then echo done; else\n)\nfi");
    assert!(err.contains("Parse error at 2:1: syntax error near unexpected token `fi'"), "got: {err}");
}
#[test]
fn cmd_subst_with_case_and_comments() {
    let ast = parse_ok("echo $(case x in a) echo 1 # )\n;; esac)");
    let w = first_arg_word(&ast);
    assert!(matches!(ast.parts[w.parts.start as usize], WordPart::CommandSubst { backtick: false, .. }));
}

// --- Native deterministic seeded parser fuzz slice (xorshift64*, fixed seed) ---
#[test]
fn seeded_parser_fuzz_no_panic() {
    let mut state: u64 = 0x5345_4E54_4C33_465A;
    let mut next = || -> u64 {
        let mut x = state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let fragments: &[&[u8]] = &[
        b"if ", b"then ", b"elif ", b"else ", b"fi ", b"for ", b"in ", b"do ", b"done ",
        b"while ", b"until ", b"case ", b"esac ", b"function ", b"time ", b"-p ",
        b"{ ", b"} ", b"( ", b") ", b"(( ", b")) ", b"[[ ", b"]] ",
        b"; ", b";; ", b";& ", b";;& ", b"| ", b"|& ", b"&& ", b"|| ", b"& ", b"! ", b"\n",
        b"<<EOF\nbody\nEOF\n", b"<<-'E'\n\tbody\n\tE\n", b"<<<hi ", b"{fd}>x ", b"2>&1 ",
        b"${x:-a} ", b"${#x} ", b"${!x[@]} ", b"${x//a/b} ", b"$(echo 1) ", b"`echo 2` ",
        b"$((1+2)) ", b"$[3*4] ", b"<(echo p) ", b"$'\\x41\\n' ", b"$\"hi\" ",
        b"{a,b}{1..3} ", b"a[1]=x ", b"a=(1 2) ", b"echo ", b"'sq' ", b"\"dq\" ", b"# c\n",
        b"\\\n", b"=~ ", b"== ", b"-f ", b"!(a|b) ", b"~user/x ",
    ];

    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, 64 << 20).unwrap();

    // 1500 grammar-mutated fragments
    for _ in 0..1500 {
        let mut buf = std::vec::Vec::new();
        let count = (next() % 16) + 1;
        for _ in 0..count {
            let f = fragments[(next() as usize) % fragments.len()];
            buf.extend_from_slice(f);
        }
        // Optional byte mutation
        if !buf.is_empty() && next() % 3 == 0 {
            let idx = (next() as usize) % buf.len();
            buf[idx] = (next() & 0xFF) as u8;
        }
        let mut budget_work = 50_000u64;
        let mut charge = |u: u64| {
            if budget_work < u { Err(()) } else { budget_work -= u; Ok(()) }
        };
        let norm = normalize(&buf, &b).unwrap();
        let _ = Ast::parse(&norm, 1024, &b, &mut charge);
    }

    // 1000 arbitrary byte / UTF-8 edge slices
    for i in 0..1000 {
        let len = ((next() % 96) + 1) as usize;
        let mut buf = std::vec::Vec::with_capacity(len);
        for _ in 0..len {
            let v = match next() % 5 {
                0 => b"(){}[];|&<>\"'`$\\#\n\0"[(next() as usize) % 19],
                1 => (next() & 0xFF) as u8,
                2 => 0xC0 | ((next() & 0x3F) as u8),
                3 => 0x80 | ((next() & 0x3F) as u8),
                _ => b'a' + ((next() % 26) as u8),
            };
            buf.push(v);
        }
        if i % 20 == 0 {
            buf.extend_from_slice(&std::vec![b'('; 205]);
        }
        let mut budget_work = 50_000u64;
        let mut charge = |u: u64| {
            if budget_work < u { Err(()) } else { budget_work -= u; Ok(()) }
        };
        let norm = normalize(&buf, &b).unwrap();
        let _ = Ast::parse(&norm, 512, &b, &mut charge);
    }
}

// --- PR #21 review fixes: bounded recursion, heredoc errors, `[[` context, EOF token cap ---

#[test]
fn lexer_deep_nested_subst_and_quotes_bounded() {
    for (label, input) in [
        ("cmd_subst_5000", std::format!("echo {}", "$(".repeat(5000))),
        ("arith_subst_5000", std::format!("echo {}", "$((".repeat(5000))),
        ("dq_param_5000", std::format!("echo {}", "\"${{x:-".repeat(5000))),
        ("legacy_arith_dq_5000", std::format!("echo {}", "$[\"".repeat(5000))),
        ("array_cmd_subst_5000", std::format!("a=({}", "$(".repeat(5000))),
        (
            "balanced_cmd_subst_205",
            std::format!("echo {}1{}", "$(echo ".repeat(205), ")".repeat(205)),
        ),
    ] {
        let err = parse_err(&input);
        assert!(
            err.contains("Maximum parser nesting depth exceeded (200)"),
            "{label}: unexpected err: {err}"
        );
    }
}

#[test]
fn word_brace_and_param_recursion_bounded_and_charged() {
    let brace_deep = std::format!("echo {}{}", "{,".repeat(5000), "}".repeat(5000));
    assert!(parse_err(&brace_deep).contains("Maximum parser nesting depth exceeded (200)"));

    let param_bangs = std::format!("echo ${{{}}}", "!".repeat(5000));
    assert!(parse_err(&param_bangs).contains("Maximum parser nesting depth exceeded (200)"));

    let param_nested = std::format!("echo {}a{}", "${x:-".repeat(5000), "}".repeat(5000));
    assert!(parse_err(&param_nested).contains("Maximum parser nesting depth exceeded (200)"));

    // Unclosed `{,` x 5000 must stay linear and not hang or overflow.
    let unclosed_brace = std::format!("echo {}", "{,".repeat(5000));
    let _ = parse_ok(&unclosed_brace);

    // Work budget must be charged inside word/param/brace parsing.
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let mut left = 25u64;
    let mut charge = |u: u64| {
        if left < u {
            Err(())
        } else {
            left -= u;
            Ok(())
        }
    };
    let tight_brace = std::format!("echo {}{}", "{a,".repeat(40), "}".repeat(40));
    assert!(matches!(
        Ast::parse(tight_brace.as_bytes(), 1 << 20, &b, &mut charge),
        Err(Error::Work)
    ));
}

#[test]
fn arith_and_cond_deep_chains_and_nesting_bounded() {
    // Iterative chains succeed without stack overflow.
    let unary_50k = std::format!("echo $(({}1))", "!".repeat(50_000));
    let _ = parse_ok(&unary_50k);

    let assign_5k = std::format!("(( {}1 ))", "a=".repeat(5_000));
    let _ = parse_ok(&assign_5k);

    let pow_5k = std::format!("(( {}2 ))", "2**".repeat(5_000));
    let _ = parse_ok(&pow_5k);

    let tern_alt_5k = std::format!("(( {}3 ))", "1?2:".repeat(5_000));
    let _ = parse_ok(&tern_alt_5k);

    let cond_not_50k = std::format!("[[ {}a ]]", "! ".repeat(50_000));
    let _ = parse_ok(&cond_not_50k);

    // True nesting constructs hit DEPTH_CAP (200).
    let tern_cons = std::format!("(( {}2{} ))", "1?".repeat(500), ":3".repeat(500));
    assert!(parse_err(&tern_cons).contains("Maximum parser nesting depth exceeded (200)"));

    let sub_deep = std::format!("(( {}0{} ))", "a[".repeat(5000), "]".repeat(5000));
    assert!(parse_err(&sub_deep).contains("Maximum parser nesting depth exceeded (200)"));

    let nested_dp = std::format!("(( {}1{} ))", "$((".repeat(500), "))".repeat(500));
    assert!(parse_err(&nested_dp).contains("Maximum parser nesting depth exceeded (200)"));

    let nested_br = std::format!("(( {}1{} ))", "$[".repeat(500), "]".repeat(500));
    assert!(parse_err(&nested_br).contains("Maximum parser nesting depth exceeded (200)"));
}

#[test]
fn skip_newlines_propagates_heredoc_parse_work_and_mem_errors() {
    // Parse-time bad substitution inside heredoc before `&&\n` and `|\n`
    let err_and = parse_err("cat <<EOF &&\n${#x-y}\nEOF\necho ok\n");
    assert!(err_and.contains("${#x-y}: bad substitution"), "got: {err_and}");

    let err_pipe = parse_err("cat <<EOF |\n${#x-y}\nEOF\ncat\n");
    assert!(err_pipe.contains("${#x-y}: bad substitution"), "got: {err_pipe}");

    // Depth cap inside heredoc before `&&\n`
    let deep_hd = std::format!(
        "cat <<EOF &&\n{}1{}\nEOF\necho ok\n",
        "$(echo ".repeat(205),
        ")".repeat(205)
    );
    let err_depth = parse_err(&deep_hd);
    assert!(
        err_depth.contains("Maximum parser nesting depth exceeded (200)"),
        "got: {err_depth}"
    );

    // Work limit inside heredoc before `&&\n` must return Error::Work, not be swallowed.
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let hd_work_src = std::format!("cat <<EOF &&\n{}\nEOF\necho ok\n", "$x".repeat(200));
    let mut left = 30u64;
    let mut charge = |u: u64| {
        if left < u {
            Err(())
        } else {
            left -= u;
            Ok(())
        }
    };
    assert!(matches!(
        Ast::parse(hd_work_src.as_bytes(), 1 << 20, &b, &mut charge),
        Err(Error::Work)
    ));
}

#[test]
fn dbrack_only_classified_in_command_position() {
    // `echo [[; ]]`: `[[` is an argument word, so `]]` is a plain command word (not DBrackEnd).
    let ast = parse_ok("echo [[; ]]");
    let stmts = &ast.stmt_lists[ast.root.range()];
    assert_eq!(stmts.len(), 2);
    let s1 = ast.stmts[stmts[1].0 as usize];
    let p1 = ast.pipelines[s1.pipelines.start as usize];
    let c1 = ast.commands[ast.cmd_lists[p1.commands.start as usize].0 as usize];
    assert!(matches!(c1, Command::Simple(sc) if sc.name.is_some()));

    // `echo [[; (( x = 1 ))`: `cond_depth` stays 0, so `((` still parses as Arithmetic.
    let ast2 = parse_ok("echo [[; (( x = 1 ))");
    let stmts2 = &ast2.stmt_lists[ast2.root.range()];
    assert_eq!(stmts2.len(), 2);
    let s2 = ast2.stmts[stmts2[1].0 as usize];
    let p2 = ast2.pipelines[s2.pipelines.start as usize];
    let c2 = ast2.commands[ast2.cmd_lists[p2.commands.start as usize].0 as usize];
    assert!(matches!(c2, Command::Arithmetic { .. }));

    // `echo [[; cat <(echo hi)`: `<(...)` still parses as ProcessSubst.
    let ast3 = parse_ok("echo [[; cat <(echo hi)");
    let stmts3 = &ast3.stmt_lists[ast3.root.range()];
    assert_eq!(stmts3.len(), 2);
    let s3 = ast3.stmts[stmts3[1].0 as usize];
    let p3 = ast3.pipelines[s3.pipelines.start as usize];
    let Command::Simple(sc3) = ast3.commands[ast3.cmd_lists[p3.commands.start as usize].0 as usize] else {
        panic!("expected Simple");
    };
    let arg_w = ast3.words[ast3.word_lists[sc3.args.start as usize].0 as usize];
    assert!(matches!(
        ast3.parts[arg_w.parts.start as usize],
        WordPart::ProcessSubst { output: false, .. }
    ));

    // Nested `[[` inside `[[ ... ]]` and command-position forms (`f() [[ ... ]]`, `function g [[ ... ]]`, `time -p [[ ... ]]`).
    let _ = parse_ok("[[ ( [[ == [[ ) ]]");
    let _ = parse_ok("f() [[ -n a ]]");
    let _ = parse_ok("function g [[ -n a ]]");
    let _ = parse_ok("time -p [[ -n a ]]");
    let _ = parse_ok("case x in a) [[ -n a ]] ;; esac");
}

#[test]
fn top_level_token_cap_includes_synthetic_eof() {
    let over_1 = "a ".repeat(100_001);
    let err = parse_err(&over_1);
    assert!(
        err.contains("Too many tokens: 100002 exceeds limit of 100000"),
        "got: {err}"
    );

    let at_cap_plus_eof = "a ".repeat(100_000);
    let err2 = parse_err(&at_cap_plus_eof);
    assert!(
        err2.contains("Too many tokens: 100001 exceeds limit of 100000"),
        "got: {err2}"
    );
}

#[test]
fn heredoc_unclosed_substitutions_diagnosed() {
    let err_brace = parse_err("cat <<EOF\n${x\nEOF\n");
    assert!(
        err_brace.contains("Parse error at 0:0: unexpected EOF while looking for matching '}'"),
        "got: {err_brace}"
    );

    let err_cmd = parse_err("cat <<EOF\n$(echo hi\nEOF\n");
    assert!(
        err_cmd.contains("unexpected EOF while looking for matching `)'"),
        "got: {err_cmd}"
    );

    let err_bt = parse_err("cat <<EOF\n`echo hi\nEOF\n");
    assert!(
        err_bt.contains("unexpected EOF while looking for matching ``'"),
        "got: {err_bt}"
    );

    // Quoted heredoc treats unclosed `${` / `$(` literally without error.
    let _ = parse_ok("cat <<'EOF'\n${x\n$(echo hi\n`echo hi\nEOF\n");
}

// --- PR #21 review v2 fixes: linear-time lookahead in dparen, subscript, brace, and extglob ---

#[test]
fn lexer_dparen_and_subscript_lookahead_is_linear() {
    use std::time::Instant;

    fn count_work(src: &str, expect_ok: bool) -> (Option<Ast>, u64) {
        crate::mem::testing::reset(CEIL);
        let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
        let mut units = 0u64;
        let mut charge = |u: u64| {
            units += u;
            Ok(())
        };
        let res = Ast::parse(src.as_bytes(), 1 << 20, &b, &mut charge);
        if expect_ok {
            let ast = res.unwrap_or_else(|e| panic!("parse failed: {e}"));
            (Some(ast), units)
        } else {
            let err = std::format!("{}", res.err().expect("expected error"));
            assert!(
                err.contains("Maximum parser nesting depth exceeded (200)"),
                "unexpected err: {err}"
            );
            (None, units)
        }
    }

    let cases: [(&str, fn(usize) -> std::string::String, usize, bool); 6] = [
        ("open_parens", |n| "(".repeat(n), 10_000, false),
        (
            "open_parens_trailing_dparen",
            |n| std::format!("{}{}", "(".repeat(n), "))"),
            10_000,
            false,
        ),
        (
            "spaced_dparen_trailing",
            |n| std::format!("{}{}", "(( ".repeat(n), "))"),
            5_000,
            false,
        ),
        (
            "balanced_parens",
            |n| std::format!("{}{}", "(".repeat(n), ")".repeat(n)),
            5_000,
            false,
        ),
        ("subscript_open", |n| "a[ ".repeat(n), 10_000, true),
        (
            "subscript_open_trailing_assign",
            |n| std::format!("{}{}", "a[ ".repeat(n), "]=1"),
            10_000,
            true,
        ),
    ];
    for (label, gen, n, expect_ok) in cases {
        let (_, w_n) = count_work(&gen(n), expect_ok);
        let t0 = Instant::now();
        let (_, w_4n) = count_work(&gen(n * 4), expect_ok);
        let elapsed_ms = t0.elapsed().as_millis();
        assert!(w_n > 0, "{label}: expected non-zero work units");
        assert!(
            w_4n <= w_n * 5,
            "{label}: non-linear work growth w({n})={w_n}, w({})={w_4n}",
            n * 4
        );
        assert!(
            elapsed_ms < 5_000,
            "{label}: took {elapsed_ms}ms (expected linear time)"
        );
    }

    // Valid nested `(((1+2)))` and `((((1+2))` semantics preserved.
    let ast = parse_ok("(((1+2)))");
    let stmts = &ast.stmt_lists[ast.root.range()];
    let p = ast.pipelines[ast.stmts[stmts[0].0 as usize].pipelines.start as usize];
    let c = ast.commands[ast.cmd_lists[p.commands.start as usize].0 as usize];
    assert!(matches!(c, Command::Arithmetic { .. }));

    // Work budget is charged during long lookahead inside `is_arithmetic_dparen`.
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let long_dparen = std::format!(
        "{}{}{}{}",
        "((a&&".repeat(10),
        "x".repeat(4000),
        ") ".repeat(20),
        "))"
    );
    let mut left = 200u64;
    let mut charge = |u: u64| {
        if left < u {
            Err(())
        } else {
            left -= u;
            Ok(())
        }
    };
    assert!(matches!(
        Ast::parse(long_dparen.as_bytes(), 1 << 20, &b, &mut charge),
        Err(Error::Work)
    ));
}

#[test]
fn word_brace_and_extglob_lookahead_is_linear() {
    use std::time::Instant;

    fn count_word_work(src: &str) -> (Ast, u64) {
        crate::mem::testing::reset(CEIL);
        let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
        let mut units = 0u64;
        let mut charge = |u: u64| {
            units += u;
            Ok(())
        };
        let ast = Ast::parse(src.as_bytes(), 1 << 20, &b, &mut charge)
            .unwrap_or_else(|e| panic!("parse failed: {e}"));
        (ast, units)
    }

    let cases: [(&str, fn(usize) -> std::string::String, usize); 7] = [
        (
            "brace_trailing_rbrace",
            |n| std::format!("echo {}{}", "{".repeat(n), "}"),
            20_000,
        ),
        (
            "brace_balanced_no_comma",
            |n| std::format!("echo {}{}", "{".repeat(n), "}".repeat(n)),
            10_000,
        ),
        (
            "brace_inner_comma",
            |n| std::format!("echo {}a,b{}", "{".repeat(n), "}".repeat(n)),
            10_000,
        ),
        (
            "brace_interleaved",
            |n| std::format!("echo {}", "{{a,b}".repeat(n)),
            2_500,
        ),
        (
            "extglob_trailing_rparen",
            |n| std::format!("echo {}{}", "@(".repeat(n), ")"),
            10_000,
        ),
        (
            "extglob_half_closed",
            |n| std::format!("echo {}{}", "@(".repeat(n), ")".repeat(n / 2)),
            10_000,
        ),
        (
            "extglob_interleaved",
            |n| std::format!("echo {}", "@(@(a)".repeat(n)),
            2_500,
        ),
    ];
    for (label, gen, n) in cases {
        let (_, w_n) = count_word_work(&gen(n));
        let t0 = Instant::now();
        let (ast, w_4n) = count_word_work(&gen(n * 4));
        let elapsed_ms = t0.elapsed().as_millis();
        assert!(w_n > 0, "{label}: expected non-zero work units");
        assert!(
            w_4n <= w_n * 5,
            "{label}: non-linear work growth w({n})={w_n}, w({})={w_4n}",
            n * 4
        );
        assert!(
            elapsed_ms < 5_000,
            "{label}: took {elapsed_ms}ms (expected linear time)"
        );
        // Spot-check that valid inner brace/extglob parts are still parsed accurately.
        if label == "brace_inner_comma" {
            assert!(!ast.brace_items.is_empty(), "{label}: missing inner brace expansion");
        }
        if label == "extglob_trailing_rparen" {
            assert!(
                ast.parts.iter().any(|p| matches!(p, WordPart::Glob { .. })),
                "{label}: missing trailing @() glob part"
            );
        }
    }
}

// --- PR #21 review v3 fixes: dparen prefix skip bound & is_arith_subst work charging ---

#[test]
fn lexer_dparen_depth_exit_does_not_overskip_prefix() {
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let mut charge = |_: u64| Ok(());

    // Depth-limit exit (`depth = 203 > max_valid_depth = 202`): `start = 0` must skip only
    // 1 paren so `start = 1` (`depth = 202`, 200 inner parens) is still lexed as `DParenStart`.
    let depth_case = std::format!(
        "{}{}{}{}{}",
        "(".repeat(10),
        "1+",
        "(".repeat(193),
        "1",
        ")".repeat(203)
    );
    let toks = super::lexer::lex(depth_case.as_bytes(), 1 << 20, &b, &mut charge).unwrap();
    assert_eq!(toks.at(0).kind, super::lexer::TokenKind::LParen);
    assert_eq!(toks.at(1).kind, super::lexer::TokenKind::DParenStart);

    // Remaining-rparen exit (`depth = 15 > rem_rparens = 14`): `start = 1` is `(( ... ))`
    // inside one unclosed outer `(`, so parsing yields `Expected RPAREN, got EOF` rather than
    // failing inside `1+(((((` as nested subshells.
    let rparen_case = std::format!(
        "{}{}{}{}{}{}",
        "(".repeat(10),
        "1+",
        "(".repeat(5),
        "1",
        ") ".repeat(12),
        "))"
    );
    let toks2 = super::lexer::lex(rparen_case.as_bytes(), 1 << 20, &b, &mut charge).unwrap();
    assert_eq!(toks2.at(0).kind, super::lexer::TokenKind::LParen);
    assert_eq!(toks2.at(1).kind, super::lexer::TokenKind::DParenStart);
    let err = parse_err(&rparen_case);
    assert!(
        err.contains("Expected RPAREN, got EOF"),
        "expected unclosed outer subshell EOF error, got: {err}"
    );

    // Remaining-rparen exit inside loop (`depth > rem_rparens` after quoted `')'`) must also
    // skip only `depth - rem_rparens` parens rather than `min_depth_after_prefix - 2`.
    let quoted_rparen_case = std::format!(
        "{}{}{}{}",
        "(".repeat(10),
        "'))'1",
        ") ".repeat(7),
        "))"
    );
    let toks3 = super::lexer::lex(quoted_rparen_case.as_bytes(), 1 << 20, &b, &mut charge).unwrap();
    assert_eq!(toks3.at(0).kind, super::lexer::TokenKind::LParen);
    assert_eq!(toks3.at(1).kind, super::lexer::TokenKind::DParenStart);
}

#[test]
fn lexer_is_arith_subst_charges_work_and_scales_linearly() {
    crate::mem::testing::reset(CEIL);
    let b = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();

    // Single `$(( ... ))` scanning 20k bytes must actively charge work units and trip a tight cap.
    let long_subst = std::format!("echo $(({}1))", " ".repeat(20_000));
    let mut left = 200u64;
    let mut tight_charge = |u: u64| {
        if left < u {
            Err(())
        } else {
            left -= u;
            Ok(())
        }
    };
    assert!(matches!(
        Ast::parse(long_subst.as_bytes(), 1 << 20, &b, &mut tight_charge),
        Err(Error::Work)
    ));

    // Linear work scaling N vs 4N on single-level `$(( ... ))`.
    let measure = |n: usize| -> u64 {
        crate::mem::testing::reset(CEIL);
        let src = std::format!("echo $(({}1))", " ".repeat(n));
        let mut units = 0u64;
        let mut charge = |u: u64| {
            units += u;
            Ok(())
        };
        Ast::parse(src.as_bytes(), 1 << 20, &b, &mut charge).unwrap();
        units
    };
    let w_n = measure(5_000);
    let w_4n = measure(20_000);
    assert!(w_n >= 100, "expected charged work >= 100, got {w_n}");
    assert!(w_4n <= w_n * 5, "non-linear scaling: w(5k)={w_n}, w(20k)={w_4n}");

    // 199 nested `$((` over 4k bytes (within DEPTH_CAP = 200) must also trip a tight work cap.
    let nested_subst = std::format!(
        "{}{}1{}",
        "$((".repeat(199),
        " ".repeat(4_000),
        "))".repeat(199)
    );
    let mut left_nested = 1_000u64;
    let mut nested_charge = |u: u64| {
        if left_nested < u {
            Err(())
        } else {
            left_nested -= u;
            Ok(())
        }
    };
    assert!(matches!(
        Ast::parse(nested_subst.as_bytes(), 1 << 20, &b, &mut nested_charge),
        Err(Error::Work)
    ));
}
