use std::{env, fs, path::PathBuf};

fn field<'a>(entry: &'a str, name: &str) -> &'a str {
    entry
        .split_once(name)
        .expect("missing CommandSpec field")
        .1
        .trim_start()
}

fn quoted(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|s| s.split_once('"').map(|(v, _)| v))
        .expect("expected a plain string literal")
}

fn main() {
    println!("cargo:rerun-if-changed=src/cmd");
    let mut modules = fs::read_dir("src/cmd")
        .unwrap()
        .map(|item| item.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "rs") && p.file_stem().unwrap() != "mod")
        .collect::<Vec<_>>();
    modules.sort();
    let mut out = String::new();
    let mut specs = Vec::new();
    for path in modules {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        assert!(stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        out.push_str(&format!("pub mod cmd_{stem} {{ include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/cmd/{stem}.rs\")); }}\n"));
        let source = fs::read_to_string(&path).unwrap();
        let (_, declaration) = source.split_once("pub const SPEC").expect("missing SPEC");
        let (_, entries) = declaration.split_once("= &[").expect("expected SPEC slice");
        let (entries, _) = entries.split_once("];").expect("unterminated SPEC slice");
        let mut count = 0;
        for (index, entry) in entries.split("CommandSpec {").skip(1).enumerate() {
            let name = quoted(field(entry, "name:"));
            assert!(
                field(entry, "kind:").starts_with("CommandKind::Standard")
                    || field(entry, "kind:").starts_with("CommandKind::Network")
            );
            let flags = field(entry, "flags:")
                .strip_prefix("&[")
                .unwrap()
                .split_once(']')
                .unwrap()
                .0;
            let flags = flags
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(quoted)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            specs.push((name.to_owned(), stem.to_owned(), index, flags));
            count += 1;
        }
        assert!(count != 0, "empty SPEC in {}", path.display());
    }
    specs.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        specs.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "duplicate command name"
    );

    out.push_str("pub static ALL_COMMANDS: &[&CommandSpec] = &[\n");
    for (_, stem, index, _) in &specs {
        out.push_str(&format!("    &cmd_{stem}::SPEC[{index}],\n"));
    }
    out.push_str("];\npub static COVERAGE_FEATURES: &[&str] = &[\n");
    let mut features = Vec::new();
    for (prefix, names) in BASE_FEATURES {
        features.extend(
            names
                .split_ascii_whitespace()
                .map(|n| format!("{prefix}:{n}")),
        );
    }
    for (name, _, _, flags) in &specs {
        features.extend(flags.iter().map(|flag| format!("cmd:flag:{name}:{flag}")));
    }
    assert!(features.len() <= u16::MAX as usize + 1);
    let mut sorted = features.iter().enumerate().collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.1.cmp(b.1));
    assert!(
        sorted.windows(2).all(|pair| pair[0].1 != pair[1].1),
        "duplicate coverage feature"
    );
    for feature in &features {
        out.push_str(&format!("    {feature:?},\n"));
    }
    out.push_str("];\npub fn coverage_id(feature: &str) -> Option<u16> {\n    match feature {\n");
    for (index, feature) in sorted {
        out.push_str(&format!("        {feature:?} => Some({index}),\n"));
    }
    out.push_str("        _ => None,\n    }\n}\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("registry.rs"),
        out,
    )
    .unwrap();
}

const BASE_FEATURES: &[(&str, &str)] = &[
    ("bash:cmd", "SimpleCommand If For CStyleFor While Until Case Subshell Group FunctionDef ArithmeticCommand ConditionalCommand"),
    ("bash:builtin", ": true false cd export unset exit local set break continue return eval shift getopts compgen complete compopt pushd popd dirs source . read mapfile readarray declare typeset readonly let command shopt exec test [ echo printf pwd alias unalias type hash ulimit umask trap times wait kill jobs fg bg disown suspend fc history help enable builtin caller"),
    ("bash:expansion", "default_value assign_default error_if_unset use_alternative pattern_removal pattern_replacement length substring case_modification transform indirection array_keys var_name_prefix tilde word_glob word_split"),
    ("awk:stmt", "expr_stmt print printf if while do_while for for_in break continue next nextfile exit return delete block"),
    ("awk:expr", "number string regex field variable array_access binary unary ternary call assignment pre_increment pre_decrement post_increment post_decrement in getline tuple"),
    ("sed:cmd", "substitute print printFirstLine delete deleteFirstLine append insert change hold holdAppend get getAppend exchange next nextAppend quit quitSilent transliterate lineNumber branch branchOnSubst branchOnNoSubst label zap group list printFilename version readFile readFileLine writeFile writeFirstLine execute"),
    ("jq:node", "Identity Field Index Slice Iterate Pipe Comma Literal Array Object Paren BinaryOp UnaryOp Cond Try Call VarBind VarRef Recurse Optional StringInterp UpdateOp Reduce Foreach Label Break Def"),
];
