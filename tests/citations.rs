//! Cuttlebone's code cites the upstream lines whose rules it mirrors
//! (`compiler/src/type_check/expressions.rs:2918-2938` and so on, relative to
//! .upstream/inko). After a rebase onto a new release, those lines move or
//! change. This test fails when they do, so whoever rebases rereads each
//! cited rule, compares it with the code citing it, and updates both the
//! citation and the text below.
use std::path::Path;

/// Each citation, and text that must appear within the cited lines.
const CITATIONS: &[(&str, &str)] = &[
    (
        "ast/src/lexer.rs:1150",
        "self.column += value.graphemes(true).count() as u32;",
    ),
    ("ast/src/lexer.rs:512-516", "fn advance_line"),
    (
        "ast/src/lexer.rs:518-520",
        "self.column += value.graphemes(true).count() as u32;",
    ),
    ("compiler/src/diagnostics.rs:25-50", "BorrowValueType,"),
    (
        "compiler/src/modules_parser.rs:119-124",
        "format!(\"the module '{}' couldn't be found\", dep),",
    ),
    (
        "inko/src/command/check.rs:62-70",
        "compiler.print_diagnostics();",
    ),
    (
        "compiler/src/hir.rs:21-29",
        "const ARRAY_LIT_VAR: &str = \"$array\";",
    ),
    ("compiler/src/mir/passes.rs:1836", ".unwrap()"),
    (
        "compiler/src/compiler.rs:258-261",
        "self.check_with(file, |_, _, _| true)",
    ),
    (
        "compiler/src/compiler.rs:51-73",
        "fn module_name_from_path(config: &Config, file: &Path) -> ModuleName {",
    ),
    (
        "compiler/src/type_check/define_types.rs:69-81",
        "if let Some(id) = self.db().builtin_type(&name) {",
    ),
    (
        "types/src/lib.rs:6258-6264",
        "CHECKED_INT_RESULT_NAME.to_string(),",
    ),
    ("compiler/src/compiler.rs:799-814", "fn prepare(&mut self)"),
    (
        "compiler/src/compiler.rs:816",
        "sync_if_needed(&self.state.config.dependencies)?;",
    ),
    (
        "compiler/src/pkg/manifest.rs:11-34",
        "let mut paths = vec![PathBuf::from(MANIFEST_FILE)];",
    ),
    (
        "compiler/src/pkg/sync.rs:22-31",
        "let (data, _data_lock) = data_dir()?;",
    ),
    ("compiler/src/pkg/util.rs:11-23", "create_dir_all(&path)"),
    (
        "compiler/src/type_check/methods.rs:1030-1094",
        "node.constructor_id = Some(constructor);",
    ),
    (
        "compiler/src/type_check/methods.rs:1286-1294",
        "let source = MethodSource::Inherited(trait_ins, method);",
    ),
    (
        "compiler/src/type_check/methods.rs:198-204",
        "method.new_argument(",
    ),
    (
        "compiler/src/type_check/expressions.rs:1335-1342",
        "hir::Expression::infer_recover(old)",
    ),
    (
        "compiler/src/type_check/expressions.rs:1371-1380",
        ".value_type_as_owned(self.db())",
    ),
    (
        "compiler/src/type_check/expressions.rs:1594-1605",
        "node.resolved_type = last_type;",
    ),
    (
        "compiler/src/type_check/expressions.rs:1618-1622",
        "self.pattern(&mut node.pattern, input_type, &mut pattern);",
    ),
    (
        "compiler/src/type_check/expressions.rs:2424-2451",
        "closure.new_argument(",
    ),
    (
        "compiler/src/type_check/expressions.rs:2484-2491",
        "parent: Some(scope),",
    ),
    (
        "compiler/src/type_check/expressions.rs:2579",
        "Some(Symbol::Type(id)) if receiver => {",
    ),
    (
        "compiler/src/type_check/expressions.rs:2714",
        "return TypeRef::module(id);",
    ),
    (
        "compiler/src/type_check/expressions.rs:2918-2938",
        "if new_scope.break_in_loop.get() {",
    ),
    (
        "compiler/src/type_check/expressions.rs:2940-2956",
        "scope = current.parent;",
    ),
    (
        "compiler/src/type_check/expressions.rs:2940-2968",
        "fn break_expression(",
    ),
    (
        "compiler/src/type_check/expressions.rs:2940-2985",
        "fn next_expression(",
    ),
    (
        "compiler/src/type_check/expressions.rs:3025-3027",
        "node.value = Some(hir::Expression::infer_recover(old));",
    ),
    (
        "compiler/src/type_check/expressions.rs:3771-3776",
        "let method = match rec_id.lookup_method(",
    ),
    (
        "compiler/src/type_check/expressions.rs:3819-3830",
        "Some(Symbol::Type(id)) if as_receiver => {",
    ),
    (
        "compiler/src/type_check/expressions.rs:3826",
        "Some(Symbol::Type(id)) if as_receiver => {",
    ),
    (
        "compiler/src/type_check/expressions.rs:4337-4426",
        "fn try_expression(",
    ),
    (
        "compiler/src/type_check/expressions.rs:4407-4414",
        "(ThrowKind::Option(_), ThrowKind::Result(ret_ok, _)) => {",
    ),
    (
        "compiler/src/type_check/expressions.rs:4499",
        "(typ, node.is_self())",
    ),
    (
        "compiler/src/type_check/expressions.rs:458-499",
        "fn check_mutability(",
    ),
    (
        "compiler/src/type_check/expressions.rs:4888-4922",
        "fn lookup_field_with_receiver(",
    ),
    (
        "compiler/src/type_check/expressions.rs:727-736",
        "Expressions { state, module: module.module_id }.run(module);",
    ),
    (
        "types/src/lib.rs:1025-1043",
        "for &req in &typ.required_traits {",
    ),
    ("types/src/lib.rs:6075-6105", "pub fn lookup_method("),
    (
        "types/src/lib.rs:6107-6119",
        "TypeEnum::TypeParameter(id) | TypeEnum::RigidTypeParameter(id) => {",
    ),
    (
        "compiler/src/hir.rs:39-90",
        "fn documentation_for(&mut self, location: &Location)",
    ),
    (
        "compiler/src/hir.rs:1320-1340",
        "while let Some(ast::TopLevelExpression::Comment(c)) = nodes.peek()",
    ),
    (
        "compiler/src/format.rs:578-620",
        "imports.sort_by(|a, b| a.path.cmp(&b.path));",
    ),
    (
        "compiler/src/type_check/expressions.rs:2544-2772",
        "if let Some((var, typ, _)) =",
    ),
    (
        "compiler/src/state.rs:98-135",
        ".join(format!(\"v{}\", dep.version.major))",
    ),
    (
        "types/src/lib.rs:3249-3262",
        "if let Some(id) = self.original_method(db) {",
    ),
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Every `path.rs:N` or `path.rs:N-M` citation of an upstream file in
/// src/ and tests/.
fn citations_in_code() -> Vec<String> {
    let mut found = Vec::new();

    for dir in ["src", "src/bin", "tests"] {
        for entry in std::fs::read_dir(root().join(dir)).unwrap() {
            let path = entry.unwrap().path();

            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("citations.rs") {
                continue;
            }

            let text = std::fs::read_to_string(&path).unwrap();

            for (i, _) in text.match_indices(".rs:") {
                let start = text[..i]
                    .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '/'))
                    .map_or(0, |j| j + 1);
                let rest = &text[i + 4..];
                let end = rest
                    .find(|c: char| !(c.is_ascii_digit() || c == '-'))
                    .unwrap_or(rest.len());
                let citation = format!(
                    "{}.rs:{}",
                    &text[start..i],
                    rest[..end].trim_end_matches('-')
                );
                let upstream = ["ast/", "compiler/", "inko/", "types/", "location/"]
                    .iter()
                    .any(|p| citation.starts_with(p));

                if upstream && end > 0 {
                    found.push(citation);
                }
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

#[test]
fn every_citation_in_the_code_is_listed_here() {
    let listed: Vec<_> = CITATIONS.iter().map(|(c, _)| c.to_string()).collect();
    let missing: Vec<_> = citations_in_code()
        .into_iter()
        .filter(|c| !listed.contains(c))
        .collect();

    assert!(missing.is_empty(), "add these to CITATIONS: {:#?}", missing);
}

#[test]
fn every_listed_citation_is_still_used() {
    let used = citations_in_code();
    let stale: Vec<_> = CITATIONS
        .iter()
        .filter(|(c, _)| !used.contains(&c.to_string()))
        .collect();

    assert!(
        stale.is_empty(),
        "remove these from CITATIONS: {:#?}",
        stale
    );
}

#[test]
fn every_citation_still_points_at_the_rule_it_cites() {
    let mut wrong = Vec::new();

    for (citation, text) in CITATIONS {
        let (file, lines) = citation.rsplit_once(':').unwrap();
        let (start, end) = match lines.split_once('-') {
            Some((a, b)) => (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()),
            None => (lines.parse().unwrap(), lines.parse().unwrap()),
        };
        let source = std::fs::read_to_string(root().join(".upstream/inko").join(file)).unwrap();
        let cited: Vec<_> = source
            .lines()
            .skip(start - 1)
            .take(end - start + 1)
            .collect();

        if !cited.iter().any(|l| l.contains(text)) {
            wrong.push(format!("{} no longer contains {:?}", citation, text));
        }
    }

    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
