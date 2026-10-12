//! Lints: warnings about hazards in programs that are otherwise valid
//! (`K9000`-`K9999`, `spec/DIAGNOSTICS.md`).
//!
//! Lints run after analysis succeeds, over the syntax tree and the analysis
//! facts, so they see resolved names and inferred types. They never change
//! whether a program checks, runs, or builds.

use std::collections::BTreeSet;

use crate::{
    Analysis, BinaryOperator, Block, Builtin, ContractRecord, Diagnostic, Expression,
    ExpressionKind, MatchKind, Program, ResolvedName, Span, Statement, StatementKind, StringPart,
    SymbolId, SymbolKind, Type, ValueLiteral, VariantName,
};

/// Returns every lint warning for `program`, which `analysis` accepted.
pub(crate) fn lint(program: &Program, analysis: &Analysis) -> Vec<Diagnostic> {
    let mut facts = collect(program, analysis);
    // A match on a name bound to a `url_path_segment` call binds the encoded
    // segment too. Collection visits uses before declarations, so these are
    // resolved once every binding is known.
    for (subject, binder) in std::mem::take(&mut facts.segment_matches) {
        if facts.segments.contains(&subject) {
            facts.encoded.insert(binder);
        }
    }
    let mut warnings = Vec::new();
    for (field, value) in facts.request_parts {
        for span in unencoded_pieces(value, analysis, &facts.encoded) {
            warnings.push(unencoded_warning(field, span));
        }
    }
    warnings
}

/// The two `HttpRequest` fields K9013 guards.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestPart {
    Path,
    Query,
}

impl RequestPart {
    fn of(field: &str) -> Option<Self> {
        match field {
            "path" => Some(Self::Path),
            "query" => Some(Self::Query),
            _ => None,
        }
    }
}

/// The K9013 warning for an unencoded piece at `span` of a request's `field`.
fn unencoded_warning(field: RequestPart, span: Span) -> Diagnostic {
    match field {
        RequestPart::Path => Diagnostic::warning(
            "K9013",
            "this value is inserted into an `HttpRequest` path without percent-encoding",
            span,
        )
        .with_note(
            "encode untrusted text with `url_path_segment` and numbers with `int_to_string`, \
             binding each result to a name, so the value cannot add `/`, `..`, `?`, or `#` to \
             the path",
        ),
        RequestPart::Query => Diagnostic::warning(
            "K9013",
            "this value is inserted into an `HttpRequest` query without percent-encoding",
            span,
        )
        .with_note(
            "build the query with `url_query([\"name\": value])`, which percent-encodes every \
             name and value, or encode a number with `int_to_string`",
        ),
    }
}

/// What the K9013 check needs from the whole program.
struct Facts<'a> {
    /// The `path` and `query` values of every `HttpRequest` the program
    /// builds, by named construction or as a record literal typed as one.
    request_parts: Vec<(RequestPart, &'a Expression)>,
    /// Bindings whose value is the result of an encoding function: a `let`
    /// of a direct call to `url_path_segment`, `url_query`, or
    /// `int_to_string`, and the `Ok` binder of a match on a direct call to
    /// `url_path_segment` or on a name bound to one.
    encoded: BTreeSet<SymbolId>,
    /// The `let` bindings of a direct call to `url_path_segment`, whose value
    /// is the `Result` a match then takes apart.
    segments: BTreeSet<SymbolId>,
    /// For each match on a name, with an `Ok` binder, that name's binding and
    /// the binder.
    segment_matches: Vec<(SymbolId, SymbolId)>,
}

/// One syntax-tree node awaiting collection.
enum Pending<'a> {
    Statement(&'a Statement),
    Block(&'a Block),
    Expression(&'a Expression),
}

/// Collects the facts with an explicit work stack, so neither deep nesting
/// nor long `else if` and operator chains deepen native recursion.
fn collect<'a>(program: &'a Program, analysis: &Analysis) -> Facts<'a> {
    let mut facts = Facts {
        request_parts: Vec::new(),
        encoded: BTreeSet::new(),
        segments: BTreeSet::new(),
        segment_matches: Vec::new(),
    };
    let mut pending = program
        .statements
        .iter()
        .map(Pending::Statement)
        .collect::<Vec<_>>();
    while let Some(node) = pending.pop() {
        match node {
            Pending::Statement(statement) => match &statement.kind {
                StatementKind::Let { name, value, .. } => {
                    if let Some(builtin) = encoding_call(value, analysis)
                        && let Some(symbol) = analysis.symbol(statement.span, name, SymbolKind::Let)
                    {
                        facts.encoded.insert(symbol.id());
                        if builtin == Builtin::UrlPathSegment {
                            facts.segments.insert(symbol.id());
                        }
                    }
                    pending.push(Pending::Expression(value));
                }
                StatementKind::Function { body, .. }
                | StatementKind::Webhook { body, .. }
                | StatementKind::QueueConsumer { body, .. }
                | StatementKind::ScheduleHandler { body, .. } => {
                    pending.push(Pending::Block(body));
                }
                StatementKind::Expression(expression) => {
                    pending.push(Pending::Expression(expression));
                }
            },
            Pending::Block(block) => {
                pending.extend(block.statements.iter().map(Pending::Statement));
                if let Some(tail) = block.tail.as_deref() {
                    pending.push(Pending::Expression(tail));
                }
            }
            Pending::Expression(expression) => {
                collect_expression(expression, analysis, &mut facts, &mut pending);
            }
        }
    }
    facts
}

fn collect_expression<'a>(
    expression: &'a Expression,
    analysis: &Analysis,
    facts: &mut Facts<'a>,
    pending: &mut Vec<Pending<'a>>,
) {
    match &expression.kind {
        ExpressionKind::Literal(_) | ExpressionKind::Unit | ExpressionKind::Variable(_) => {}
        ExpressionKind::Interpolation(parts) => {
            pending.extend(parts.iter().filter_map(|part| match part {
                StringPart::Hole { value, .. } => Some(Pending::Expression(value)),
                StringPart::Text { .. } => None,
            }));
        }
        ExpressionKind::List(elements) => {
            pending.extend(elements.iter().map(Pending::Expression));
        }
        ExpressionKind::EntryList(entries) => {
            for entry in entries {
                pending.push(Pending::Expression(&entry.key));
                pending.push(Pending::Expression(&entry.value));
            }
        }
        ExpressionKind::Record(fields) => {
            if is_http_request_record(expression, analysis) {
                facts
                    .request_parts
                    .extend(fields.iter().filter_map(|field| {
                        RequestPart::of(&field.name).map(|part| (part, &field.value))
                    }));
            }
            pending.extend(fields.iter().map(|field| Pending::Expression(&field.value)));
        }
        ExpressionKind::Construct {
            contract, fields, ..
        } => {
            if *contract == ContractRecord::HttpRequest {
                facts
                    .request_parts
                    .extend(fields.iter().filter_map(|field| {
                        RequestPart::of(&field.name).map(|part| (part, &field.value))
                    }));
            }
            pending.extend(fields.iter().map(|field| Pending::Expression(&field.value)));
        }
        ExpressionKind::FieldAccess { value, .. } => pending.push(Pending::Expression(value)),
        ExpressionKind::Block(block) => pending.push(Pending::Block(block)),
        ExpressionKind::If {
            condition,
            consequent,
            alternative,
        } => {
            pending.push(Pending::Expression(condition));
            pending.push(Pending::Block(consequent));
            pending.push(Pending::Expression(alternative));
        }
        ExpressionKind::Function { body, .. } => pending.push(Pending::Block(body)),
        ExpressionKind::Call { callee, arguments } => {
            pending.push(Pending::Expression(callee));
            pending.extend(arguments.iter().map(Pending::Expression));
        }
        ExpressionKind::Match { subject, kind } => {
            pending.push(Pending::Expression(subject));
            match kind {
                MatchKind::List {
                    empty_case,
                    cons_case,
                    ..
                } => {
                    pending.push(Pending::Expression(empty_case));
                    pending.push(Pending::Expression(cons_case));
                }
                MatchKind::Variants { arms, .. } => {
                    let segment = encoding_call(subject, analysis) == Some(Builtin::UrlPathSegment);
                    let subject_binding = resolved_symbol(subject, analysis);
                    for arm in arms {
                        if arm.variant == VariantName::Ok
                            && let Some(name) = &arm.binding
                            && let Some(binder) = analysis.symbol(arm.span, name, SymbolKind::Match)
                        {
                            if segment {
                                facts.encoded.insert(binder.id());
                            } else if let Some(subject) = subject_binding {
                                facts.segment_matches.push((subject, binder.id()));
                            }
                        }
                        pending.push(Pending::Expression(&arm.value));
                    }
                }
            }
        }
        ExpressionKind::Unary { operand, .. } => pending.push(Pending::Expression(operand)),
        ExpressionKind::Binary { left, right, .. } => {
            pending.push(Pending::Expression(left));
            pending.push(Pending::Expression(right));
        }
    }
}

/// The encoding function `expression` calls directly, if it is a call of
/// `url_path_segment`, `url_query`, or `int_to_string` whose callee resolves
/// to the built-in (a match binder may shadow these names).
fn encoding_call(expression: &Expression, analysis: &Analysis) -> Option<Builtin> {
    let ExpressionKind::Call { callee, .. } = &expression.kind else {
        return None;
    };
    if !matches!(callee.kind, ExpressionKind::Variable(_)) {
        return None;
    }
    match analysis.expression(callee.span)?.resolved_name()? {
        ResolvedName::Builtin(
            builtin @ (Builtin::UrlPathSegment | Builtin::UrlQuery | Builtin::IntToString),
        ) => Some(builtin),
        _ => None,
    }
}

/// The binding a name expression resolves to, or `None` for any other
/// expression or a built-in.
fn resolved_symbol(expression: &Expression, analysis: &Analysis) -> Option<SymbolId> {
    if !matches!(expression.kind, ExpressionKind::Variable(_)) {
        return None;
    }
    match analysis.expression(expression.span)?.resolved_name()? {
        ResolvedName::Symbol(id) => Some(id),
        ResolvedName::Builtin(_) => None,
    }
}

/// Whether a record literal builds an `HttpRequest`: it writes exactly the
/// contract's five fields, and analysis typed each one as the contract field
/// it names (an element type still unknown, as in `headers: []`, fits).
fn is_http_request_record(record: &Expression, analysis: &Analysis) -> bool {
    analysis
        .expression(record.span)
        .is_some_and(|fact| is_http_request_type(fact.ty()))
}

fn is_http_request_type(ty: &Type) -> bool {
    let Type::Record(fields) = ty else {
        return ty == &Type::HttpRequest;
    };
    let expected = ContractRecord::HttpRequest.fields();
    fields.len() == expected.len()
        && expected.iter().all(|contract| {
            fields
                .iter()
                .find(|field| field.name() == contract.name)
                .is_some_and(|field| match contract.name {
                    "headers" => is_header_list_type(field.ty()),
                    _ => fits(field.ty(), &Type::String),
                })
        })
}

fn is_header_list_type(ty: &Type) -> bool {
    match ty {
        Type::Variable(_) => true,
        Type::List(element) => match element.as_ref() {
            Type::Variable(_) | Type::HttpHeader | Type::LogField => true,
            Type::Record(fields) => {
                fields.len() == 2
                    && ["name", "value"].iter().all(|name| {
                        fields
                            .iter()
                            .any(|field| field.name() == *name && fits(field.ty(), &Type::String))
                    })
            }
            _ => false,
        },
        _ => false,
    }
}

/// Whether a field whose analyzed type is `actual` fits `expected`: equal,
/// or still unknown.
fn fits(actual: &Type, expected: &Type) -> bool {
    matches!(actual, Type::Variable(_)) || actual == expected
}

/// The spans of the pieces of a request `path` or `query` value that no
/// encoding function produced.
///
/// Only an interpolated string or a `+` chain is examined; any other value,
/// such as a literal, a name, or a call, is not split into pieces and never
/// warns. A piece is a hole of an interpolated string (also one inside a
/// chain) or an operand of the chain. Literal pieces are safe; so are direct
/// calls of `url_path_segment`, `url_query`, or `int_to_string`, and names
/// that resolve to a binding in `encoded`. Every other piece is reported, at
/// its hole or operand.
fn unencoded_pieces(
    value: &Expression,
    analysis: &Analysis,
    encoded: &BTreeSet<SymbolId>,
) -> Vec<Span> {
    let is_string_addition = |expression: &Expression| {
        matches!(
            expression.kind,
            ExpressionKind::Binary {
                operator: BinaryOperator::Add,
                ..
            }
        )
    };
    if !matches!(value.kind, ExpressionKind::Interpolation(_)) && !is_string_addition(value) {
        return Vec::new();
    }
    let mut pieces = Vec::new();
    let mut pending = vec![(value, value.span)];
    while let Some((piece, span)) = pending.pop() {
        match &piece.kind {
            ExpressionKind::Binary {
                operator: BinaryOperator::Add,
                left,
                right,
            } => {
                pending.push((right, right.span));
                pending.push((left, left.span));
            }
            ExpressionKind::Interpolation(parts) => {
                for part in parts.iter().rev() {
                    if let StringPart::Hole { value, span } = part {
                        pending.push((value, *span));
                    }
                }
            }
            ExpressionKind::Literal(ValueLiteral::String(_)) => {}
            _ if resolved_symbol(piece, analysis).is_some_and(|id| encoded.contains(&id)) => {}
            _ if encoding_call(piece, analysis).is_some() => {}
            _ => pieces.push(span),
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use crate::{Severity, Source, Span, analyze_with_warnings, parse_source};

    /// The K9013 warnings of `text`, which must check, as the source text
    /// of each warning's span.
    fn unencoded(text: &str) -> Vec<&str> {
        let program = parse_source(&Source::new("test.krit", text)).expect("source should parse");
        let report = analyze_with_warnings(&program);
        report.result.expect("source should check");
        report
            .warnings
            .iter()
            .map(|warning| {
                assert_eq!(warning.code(), "K9013");
                assert_eq!(warning.severity(), Severity::Warning);
                assert_eq!(warning.notes().len(), 1);
                assert!(!warning.notes()[0].contains(['\n', '\r']));
                let Span { start, end } = warning.span();
                &text[start..end]
            })
            .collect()
    }

    #[test]
    fn warns_at_each_unencoded_piece_of_a_request_path_or_query() {
        assert_eq!(
            unencoded(
                r#"
fn fetch(owner: String, request: HttpRequest, search: String) -> HttpRequest {
    HttpRequest {
        method: "GET",
        path: "/repos/\(owner)/issues/" + request.body + ("/" + owner),
        query: "q=" + search,
    }
}
"#
            ),
            ["\\(owner)", "request.body", "owner", "search"]
        );
        // A record literal typed as an `HttpRequest`, before and after it
        // meets one.
        assert_eq!(
            unencoded(
                r#"
fn fetch(id: String) -> HttpRequest {
    let unused = record { method: "GET", path: "/a/" + id, query: "", headers: [], body: "" };
    record { method: "GET", path: "/b/\(id)", query: "", headers: [], body: "" }
}
"#
            ),
            ["id", "\\(id)"]
        );
    }

    #[test]
    fn the_message_and_note_name_the_part_and_its_encoder() {
        let text = "fn f(x: String) -> HttpRequest { HttpRequest { method: \"GET\", path: \"/\" + x, \
                    query: \"q=\\(x)\" } }";
        let program = parse_source(&Source::new("test.krit", text)).expect("source should parse");
        let warnings = analyze_with_warnings(&program).warnings;
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].message().contains("`HttpRequest` path"));
        assert!(warnings[0].notes()[0].contains("url_path_segment"));
        assert!(warnings[1].message().contains("`HttpRequest` query"));
        assert!(warnings[1].notes()[0].contains("url_query"));
    }

    #[test]
    fn encoded_pieces_and_whole_values_never_warn() {
        assert!(
            unencoded(
                r#"
fn fetch(owner: String, number: Int, request: HttpRequest) -> HttpRequest {
    let issue = int_to_string(number);
    let query = url_query(["state": "open"]);
    match url_path_segment(owner) {
        Ok(segment) => HttpRequest {
            method: "GET",
            path: "/repos/\(segment)/issues/\(issue)/" + int_to_string(number) + """
                /labels
                """,
            query: query + "&" + url_query(["q": owner]) + "&page=" + int_to_string(1),
        },
        Err(reason) => HttpRequest { method: "GET", path: request.path, query: request.query },
    }
}

fn literal() -> HttpRequest {
    HttpRequest { method: "GET", path: "/a" + "/b", query: "x=1" }
}

fn bound(owner: String) -> HttpRequest {
    let encoded = url_path_segment(owner);
    match encoded {
        Ok(segment) => HttpRequest { method: "GET", path: "/repos/\(segment)" },
        Err(reason) => HttpRequest { method: "GET", path: "/" },
    }
}

fn response(owner: String) -> HttpResponse {
    HttpResponse { status: 200, body: "owner " + owner }
}

fn other(owner: String) -> String {
    let shaped = record { path: "/" + owner, query: owner };
    shaped.path
}
"#
            )
            .is_empty()
        );
    }

    #[test]
    fn only_bindings_of_encoding_calls_count_as_encoded() {
        // A shadowing binding, an `Err` binder, a match on another call, and a
        // user function that a match binder names like a built-in.
        assert_eq!(
            unencoded(
                r#"
fn fetch(owner: String, convert) -> List<HttpRequest> {
    let issue = int_to_string(1);
    let copied = int_to_string(2);
    let alias = copied;
    let first = {
        let issue = owner;
        match url_path_segment(owner) {
            Ok(segment) => HttpRequest { method: "GET", path: "/\(issue)/\(segment)" },
            Err(reason) => HttpRequest { method: "GET", path: "/\(reason)" },
        }
    };
    let second = match Some(owner) {
        Some(value) => HttpRequest { method: "GET", path: "/\(value)/\(alias)" },
        None => HttpRequest { method: "GET", path: "/" },
    };
    let checked = Ok(owner);
    let fourth = match checked {
        Ok(text) => HttpRequest { method: "GET", path: "/\(text)" },
        Err(reason) => HttpRequest { method: "GET", path: reason },
    };
    let third = match Some(convert) {
        Some(int_to_string) => HttpRequest { method: "GET", path: "/" + int_to_string(3) },
        None => HttpRequest { method: "GET", path: "/" },
    };
    [first, second, third, fourth]
}
"#
            ),
            [
                "\\(issue)",
                "\\(reason)",
                "\\(value)",
                "\\(alias)",
                "\\(text)",
                "int_to_string(3)"
            ]
        );
    }

    #[test]
    fn lints_run_only_when_analysis_succeeds() {
        let program = parse_source(&Source::new(
            "test.krit",
            "fn f(x: String) -> HttpRequest { HttpRequest { method: \"GET\", path: \"/\" + x } }\n\
             let broken = 1 + \"a\";",
        ))
        .expect("source should parse");
        let report = analyze_with_warnings(&program);
        assert!(report.result.is_err());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn long_chains_are_examined_without_deep_recursion() {
        let pieces = (0..4_000).map(|_| "x").collect::<Vec<_>>().join(" + ");
        let text = format!(
            "fn f(x: String) -> HttpRequest {{ HttpRequest {{ method: \"GET\", path: \"/\" + {pieces} }} }}"
        );
        let warnings = crate::run_on_thread("lint-test", crate::COMPILER_STACK_BYTES, move || {
            let program =
                parse_source(&Source::new("test.krit", text)).expect("source should parse");
            analyze_with_warnings(&program).warnings.len()
        })
        .expect("lint thread should start");
        assert_eq!(warnings, 4_000);
    }
}
