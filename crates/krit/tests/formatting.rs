use std::{
    fs,
    path::{Path, PathBuf},
};

use krit::{Source, analyze, format_source, parse_source};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("krit crate should be inside the workspace")
        .to_path_buf()
}

fn collect_krit_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .expect("repository directory should be readable")
        .map(|entry| entry.expect("directory entry should be readable").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == "target" || name == ".git")
            {
                continue;
            }
            collect_krit_sources(&path, sources);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "krit")
        {
            sources.push(path);
        }
    }
}

fn analysis_code(source: &Source) -> Result<(), &'static str> {
    parse_source(source)
        .and_then(|program| analyze(&program))
        .map(|_| ())
        .map_err(|diagnostic| diagnostic.code())
}

#[test]
fn formatting_fixtures_are_canonical_and_idempotent() {
    let root = repository_root().join("conformance/format");
    for fixture in ["comments", "edition-2026", "webhook"] {
        let directory = root.join(fixture);
        let input = fs::read_to_string(directory.join("input.krit"))
            .expect("formatter input should be readable");
        let expected = fs::read_to_string(directory.join("formatted.krit"))
            .expect("formatter output should be readable");
        let source = Source::new(format!("<format-{fixture}>"), input);
        let formatted = format_source(&source).expect("fixture should format");

        assert_eq!(formatted, expected, "{fixture}");
        assert_eq!(
            format_source(&Source::new(
                format!("<formatted-{fixture}>"),
                formatted.as_str(),
            ))
            .expect("formatted fixture should format"),
            formatted,
            "{fixture}"
        );
        assert!(!formatted.contains('\r'), "{fixture}");
        assert!(!formatted.contains('\t'), "{fixture}");
        assert!(
            formatted
                .lines()
                .all(|line| !line.ends_with(' ') && !line.ends_with('\t')),
            "{fixture}"
        );
    }
}

#[test]
fn every_repository_source_round_trips_through_the_formatter() {
    let root = repository_root();
    let mut paths = Vec::new();
    collect_krit_sources(&root, &mut paths);
    assert!(!paths.is_empty());

    for path in paths {
        let text = fs::read_to_string(&path).expect("Krit source should be readable");
        let name = path
            .strip_prefix(&root)
            .expect("source should be in repository")
            .to_string_lossy()
            .into_owned();
        let source = Source::new(name.clone(), text);
        let before_parse = parse_source(&source);
        let formatted = format_source(&source);

        match (before_parse, formatted) {
            (Ok(_), Ok(formatted)) => {
                let is_noncanonical_fixture =
                    name.starts_with("conformance/format/") && name.ends_with("/input.krit");
                if !is_noncanonical_fixture {
                    assert_eq!(source.text(), formatted, "{name} should be canonical");
                }
                let formatted_source = Source::new(name.clone(), formatted.clone());
                parse_source(&formatted_source).expect("formatted source should parse");
                assert_eq!(
                    analysis_code(&source),
                    analysis_code(&formatted_source),
                    "{name}"
                );
                assert_eq!(
                    format_source(&formatted_source).expect("formatting should be idempotent"),
                    formatted,
                    "{name}"
                );
            }
            (Err(before), Err(after)) => assert_eq!(before.code(), after.code(), "{name}"),
            (before, after) => panic!(
                "formatter parse behavior changed for {name}: before={}, after={}",
                before.is_ok(),
                after.is_ok()
            ),
        }
    }
}

#[test]
fn deterministic_generated_corpus_preserves_formatting_properties() {
    let mut corpus = vec![
        (
            "nested-generics".to_owned(),
            "let value: Result<List<Option<Int>>, Result<String, List<Bool>>> = Ok([Some(1), None]);\n"
                .to_owned(),
        ),
        (
            "unary-chains".to_owned(),
            "let flags = !!!false;\nlet number = ---1;\n".to_owned(),
        ),
        (
            "if-else-comments".to_owned(),
            "let value = if true { // consequent\n1 // consequent tail\n} // before else\nelse if false { 2 } else { // alternative\n3\n};\n"
                .to_owned(),
        ),
        (
            "delimiter-and-comma-comments".to_owned(),
            "let values = [ // after open\n1, // after comma\n// before item\n2 // before trailing comma\n,];\nprintln( // after call open\nvalues, // trailing argument\n);\n"
                .to_owned(),
        ),
        (
            "empty-groups".to_owned(),
            "fn empty() -> Unit {}\nlet values: List<Int> = [];\nlet item = record {};\nempty();\n"
                .to_owned(),
        ),
        (
            "long-nested-groups".to_owned(),
            "println(Some(Ok([1 + 2, (3 + 4) * 5, 6 + 7, 8 + 9, 10 + 11, 12 + 13, 14 + 15, 16 + 17, 18 + 19])));\n"
                .to_owned(),
        ),
        (
            "unicode".to_owned(),
            "// Ελληνικά 日本語 🚀\nprintln(\"héllo λ 世界 🚀\"); // café\n".to_owned(),
        ),
        (
            "crlf".to_owned(),
            "fn identity(value:Int)->Int{\r\n\tvalue\r\n}\r\nprintln(identity(1));\r\n"
                .to_owned(),
        ),
        (
            "block-valued-callee".to_owned(),
            "let value = { fn(item: Int) -> Int { item } }( // call\n1\n);\n".to_owned(),
        ),
        (
            "group-after-declaration".to_owned(),
            "fn helper() -> Int { 1 }\n( // grouping\n1 + 2\n);\n".to_owned(),
        ),
        (
            "keyword-list-subjects".to_owned(),
            "if [129] { 1 } else { 2 };\nmatch [129] { [] => 0, [head, ..tail] => head };\n"
                .to_owned(),
        ),
        (
            "grouped-record-value".to_owned(),
            "let item = record { value: (1 + 2), nested: record { answer: (3 + 4) } };\n"
                .to_owned(),
        ),
        (
            "webhook-contract".to_owned(),
            "webhook fn handle(request:HttpRequest)->HttpResponse{record{status:200,headers:[],body:request.path}}\n"
                .to_owned(),
        ),
    ];

    let atoms = [
        "1",
        "true",
        "\"κρίτ\"",
        "(1 + 2)",
        "---3",
        "!!false",
        "[1, 2, 3,]",
        "record { value: 1, }",
        "Some(1)",
        "Ok(\"yes\")",
        "if true { 1 } else { 2 }",
        "{ let inner = 1; inner + 1 }",
        "fn(value: Int) -> Int { value }",
    ];
    for (index, atom) in atoms.iter().enumerate() {
        corpus.push((
            format!("atom-{index}"),
            format!("let generated_{index} = {atom};\n"),
        ));
    }

    for (index, operator) in ["+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">="]
        .iter()
        .enumerate()
    {
        corpus.push((
            format!("binary-{index}"),
            format!("let generated_{index} = (1 + 2) {operator} (3 + 4);\n"),
        ));
    }

    for (index, (annotation, value)) in [
        ("List<Option<Int>>", "[Some(1), None]"),
        ("Result<List<Option<Int>>, String>", "Ok([Some(1), None])"),
        ("Option<Result<List<Int>, String>>", "Some(Ok([1, 2, 3]))"),
        (
            "Result<Result<Int, String>, List<Bool>>",
            "Err([true, false])",
        ),
        (
            "Record { nested: Result<List<Int>, Option<String>> }",
            "record { nested: Ok([1, 2, 3]) }",
        ),
    ]
    .iter()
    .enumerate()
    {
        corpus.push((
            format!("annotation-{index}"),
            format!("let generated_{index}: {annotation} = {value};\n"),
        ));
    }

    for (index, trailing_comma) in ["", ","].iter().enumerate() {
        corpus.push((
            format!("optional-commas-{index}"),
            format!(
                "let generated_{index} = record {{ first: [1, 2, 3{trailing_comma}], second: Some((4 + 5)){trailing_comma} }};\n"
            ),
        ));
    }

    assert!(corpus.len() >= 40);
    for (name, text) in corpus {
        let source = Source::new(format!("<generated-{name}>"), text);
        parse_source(&source).unwrap_or_else(|diagnostic| {
            panic!("{name}: generated input should parse: {diagnostic:?}")
        });
        let before_analysis = analysis_code(&source);

        let formatted =
            format_source(&source).unwrap_or_else(|diagnostic| panic!("{name}: {diagnostic:?}"));
        let formatted_source = Source::new(format!("<formatted-{name}>"), formatted.clone());
        parse_source(&formatted_source).unwrap_or_else(|diagnostic| {
            panic!("{name}: formatted output should parse: {diagnostic:?}")
        });
        assert_eq!(
            analysis_code(&formatted_source),
            before_analysis,
            "{name}: analysis result code changed"
        );
        assert_eq!(
            format_source(&formatted_source)
                .unwrap_or_else(|diagnostic| panic!("{name}: {diagnostic:?}")),
            formatted,
            "{name}: formatting was not idempotent"
        );
    }
}
