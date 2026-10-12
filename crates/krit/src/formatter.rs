use std::collections::{HashMap, HashSet};

use crate::{
    Block, Diagnostic, Expression, ExpressionKind, Pattern, PatternKind, Program, Source, Span,
    Statement, StatementKind, StringPart, TypeAnnotation, TypeKind,
    lexer::{Comment, lex_with_comments},
    parser,
    token::{TemplatePart, Token, TokenKind},
};

/// The formatter's soft line-width target.
pub const FORMAT_LINE_WIDTH: usize = 100;

/// Formats one parseable edition-2026 source file into canonical Krit source.
pub fn format_source(source: &Source) -> Result<String, Diagnostic> {
    let lexed = lex_with_comments(source)?;
    let program = parser::parse(source, lexed.tokens.clone())?;
    Ok(Formatter::new(lexed.tokens, lexed.comments, &program).format())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GroupKind {
    Block,
    Match,
    BracedCollection,
    List,
    Pattern,
    Parameters,
    Call,
    Grouping,
}

impl GroupKind {
    const fn allows_trailing_comma(self) -> bool {
        matches!(
            self,
            Self::Match | Self::BracedCollection | Self::List | Self::Parameters | Self::Call
        )
    }

    const fn uses_brace_padding(self) -> bool {
        matches!(self, Self::BracedCollection)
    }
}

#[derive(Clone, Copy, Debug)]
struct GroupInfo {
    close: usize,
    kind: GroupKind,
    has_content: bool,
    forced_multiline: bool,
    flat_width: usize,
}

#[derive(Clone, Copy, Debug)]
struct ActiveGroup {
    open: usize,
    close: usize,
    kind: GroupKind,
    multiline: bool,
}

struct Formatter {
    tokens: Vec<Token>,
    /// Tokens the canonical form leaves out: the `: name` of a construction
    /// field whose value is its own name, which is written punned.
    omitted: Vec<bool>,
    token_text: Vec<String>,
    comments: Vec<Comment>,
    comment_cursor: usize,
    groups: Vec<Option<GroupInfo>>,
    close_to_open: Vec<Option<usize>>,
    type_tokens: Vec<bool>,
    generic_commas: Vec<bool>,
    breaks_after: HashMap<usize, usize>,
    /// For the `else` token of each let-else, the token index of the `;`
    /// that ends the statement.
    let_else_arms: HashMap<usize, usize>,
    /// The `;` of each let-else whose arm was moved to its own, indented
    /// line, innermost last; the indentation ends after it.
    let_else_indents: Vec<usize>,
    active_groups: Vec<ActiveGroup>,
    closing_group: Option<ActiveGroup>,
    writer: Writer,
    pending_break: usize,
    previous_rendered: Option<usize>,
}

impl Formatter {
    fn new(tokens: Vec<Token>, mut comments: Vec<Comment>, program: &Program) -> Self {
        let token_text = tokens.iter().map(canonical_token_text).collect::<Vec<_>>();
        let syntax = SyntaxMap::new(program, &tokens);
        canonicalize_doc_comments(&mut comments, &syntax.doc_spans);
        let omitted = punned_tokens(&tokens, &comments, &syntax.pun_values);
        let (groups, close_to_open) = build_groups(
            &tokens,
            &token_text,
            &comments,
            &syntax.block_spans,
            &syntax.call_group_spans,
            &syntax.pattern_group_spans,
            &omitted,
        );
        Self {
            omitted,
            type_tokens: syntax.type_tokens,
            generic_commas: syntax.generic_commas,
            breaks_after: syntax.breaks_after,
            let_else_arms: syntax.let_else_arms,
            let_else_indents: Vec::new(),
            tokens,
            token_text,
            comments,
            comment_cursor: 0,
            groups,
            close_to_open,
            active_groups: Vec::new(),
            closing_group: None,
            writer: Writer::default(),
            pending_break: 0,
            previous_rendered: None,
        }
    }

    fn format(mut self) -> String {
        for index in 0..self.tokens.len() {
            if matches!(self.tokens[index].kind, TokenKind::Eof) {
                self.emit_comments_before(self.tokens[index].span.start);
                break;
            }

            if self.close_to_open[index].is_some() {
                self.insert_trailing_comma_if_needed(index);
            }
            self.emit_comments_before(self.tokens[index].span.start);

            if self.omitted[index] || self.should_skip_trailing_comma(index) {
                continue;
            }

            if self.close_to_open[index].is_some() {
                self.close_group(index);
            }

            self.emit_token(index);

            if self.groups[index].is_some() {
                self.open_group(index);
            }

            self.request_break_after(index);
        }

        while self.comment_cursor < self.comments.len() {
            self.emit_comment(self.comment_cursor);
            self.comment_cursor += 1;
        }

        self.writer.finish()
    }

    fn emit_comments_before(&mut self, position: usize) {
        while self
            .comments
            .get(self.comment_cursor)
            .is_some_and(|comment| comment.span.start < position)
        {
            self.emit_comment(self.comment_cursor);
            self.comment_cursor += 1;
        }
    }

    fn emit_comment(&mut self, index: usize) {
        let inline = self.comments[index].inline;
        let text = self.comments[index].text.clone();
        if inline && !self.writer.is_line_start() {
            self.writer.space();
            self.writer.write(&text);
            self.writer.ensure_newlines(self.pending_break.max(1));
            self.pending_break = 0;
        } else {
            self.apply_pending_break();
            if !self.writer.is_line_start() {
                self.writer.ensure_newlines(1);
            }
            self.writer.write(&text);
            self.writer.ensure_newlines(1);
        }
    }

    fn emit_token(&mut self, index: usize) {
        self.apply_pending_break();
        if self.needs_space(index) {
            self.writer.space();
        }
        if let TokenKind::Template(template) = &self.tokens[index].kind
            && let Some(lines) = &template.text_block
        {
            self.writer.write_text_block(lines);
        } else {
            self.writer.write(&self.token_text[index]);
        }
        self.previous_rendered = Some(index);
        self.closing_group = None;
    }

    fn open_group(&mut self, index: usize) {
        let info = self.groups[index].expect("opening delimiter should have group information");
        let projected_width = self.writer.column().saturating_add(info.flat_width);
        let multiline =
            info.has_content && (info.forced_multiline || projected_width > FORMAT_LINE_WIDTH);
        self.active_groups.push(ActiveGroup {
            open: index,
            close: info.close,
            kind: info.kind,
            multiline,
        });
        if multiline {
            self.writer.indent += 1;
            self.request_break(1);
        }
    }

    fn close_group(&mut self, index: usize) {
        let group = self
            .active_groups
            .pop()
            .expect("closing delimiter should have an active group");
        debug_assert_eq!(group.close, index);
        if group.multiline {
            self.writer.indent = self.writer.indent.saturating_sub(1);
            self.request_break(1);
        }
        self.closing_group = Some(group);
    }

    fn insert_trailing_comma_if_needed(&mut self, close: usize) {
        let Some(group) = self.active_groups.last().copied() else {
            return;
        };
        if group.close != close
            || !group.multiline
            || !group.kind.allows_trailing_comma()
            || !self.group_has_rendered_content(group)
            || self
                .previous_significant_token(close)
                .is_some_and(|index| matches!(self.tokens[index].kind, TokenKind::Comma))
        {
            return;
        }

        self.apply_pending_break();
        self.writer.write(",");
        self.request_break(1);
    }

    fn group_has_rendered_content(&self, group: ActiveGroup) -> bool {
        self.previous_rendered
            .is_some_and(|previous| previous > group.open && previous < group.close)
    }

    fn should_skip_trailing_comma(&self, index: usize) -> bool {
        if !matches!(self.tokens[index].kind, TokenKind::Comma) {
            return false;
        }
        let Some(group) = self.active_groups.last() else {
            return false;
        };
        !group.multiline
            && group.kind.allows_trailing_comma()
            && self.next_significant_token(index) == Some(group.close)
    }

    fn request_break_after(&mut self, index: usize) {
        if let Some(semicolon) = self.let_else_arms.get(&index).copied() {
            if self.let_else_arm_breaks(index, semicolon) {
                self.writer.indent += 1;
                self.let_else_indents.push(semicolon);
                self.request_break(1);
            }
            return;
        }
        if self.let_else_indents.last() == Some(&index) {
            self.let_else_indents.pop();
            self.writer.indent = self.writer.indent.saturating_sub(1);
        }
        if let Some(lines) = self.breaks_after.get(&self.tokens[index].span.end) {
            self.request_break(*lines);
            return;
        }

        let statement_end = matches!(self.tokens[index].kind, TokenKind::Semicolon);
        let multiline_item_end = matches!(self.tokens[index].kind, TokenKind::Comma)
            && !self.generic_commas[index]
            && self
                .active_groups
                .last()
                .is_some_and(|group| group.multiline);
        if statement_end || multiline_item_end {
            self.request_break(1);
        }
    }

    /// Whether the arm of the let-else whose `else` token was just written at
    /// `else_index` moves to its own line, indented one level: unless the
    /// rest of the statement, through the `;` at `semicolon`, fits on the
    /// current line exactly as it would render there.
    ///
    /// A comment or an always-multiline group (a block or a `match`) in the
    /// arm never fits, and neither does a delimited group that would wrap on
    /// its own, so an arm that stays on the line is rendered flat.
    fn let_else_arm_breaks(&self, else_index: usize, semicolon: usize) -> bool {
        let start = self.tokens[else_index].span.end;
        let end = self.tokens[semicolon].span.start;
        // Comments are in source order.
        let first_comment = self
            .comments
            .partition_point(|comment| comment.span.start < start);
        if self
            .comments
            .get(first_comment)
            .is_some_and(|comment| comment.span.end <= end)
        {
            return true;
        }
        let mut column = self.writer.column();
        let mut previous = else_index;
        for index in else_index + 1..=semicolon {
            // The arm is measured as it would be written on this line: without
            // the `: name` of a punned construction field, and without a
            // trailing comma, which a group that stays on the line drops.
            // Measuring the input's spelling instead would break an arm that
            // fits once written, and the next formatting would join it again.
            if self.omitted[index] || self.drops_trailing_comma(index) {
                continue;
            }
            if token_pair_needs_space(
                &self.tokens,
                &self.groups,
                &self.close_to_open,
                &self.type_tokens,
                previous,
                index,
                |_| false,
            ) {
                column += 1;
            }
            previous = index;
            column += self.token_text[index].chars().count();
            if let Some(group) = self.groups[index]
                && (group.forced_multiline
                    || column.saturating_add(group.flat_width) > FORMAT_LINE_WIDTH)
            {
                return true;
            }
        }
        column > FORMAT_LINE_WIDTH
    }

    /// Whether the token at `index` is a trailing comma, which a group that
    /// allows one drops when it stays on one line.
    fn drops_trailing_comma(&self, index: usize) -> bool {
        matches!(self.tokens[index].kind, TokenKind::Comma)
            && self
                .next_significant_token(index)
                .and_then(|close| self.close_to_open[close])
                .and_then(|open| self.groups[open])
                .is_some_and(|group| group.kind.allows_trailing_comma())
    }

    fn request_break(&mut self, lines: usize) {
        self.pending_break = self.pending_break.max(lines);
    }

    fn apply_pending_break(&mut self) {
        if self.pending_break > 0 {
            self.writer.ensure_newlines(self.pending_break);
            self.pending_break = 0;
        }
    }

    fn needs_space(&self, current: usize) -> bool {
        if self.writer.is_line_start() {
            return false;
        }
        let Some(previous) = self.previous_rendered else {
            return false;
        };
        token_pair_needs_space(
            &self.tokens,
            &self.groups,
            &self.close_to_open,
            &self.type_tokens,
            previous,
            current,
            |open| {
                self.closing_group
                    .filter(|group| group.open == open)
                    .or_else(|| {
                        self.active_groups
                            .iter()
                            .rev()
                            .find(|group| group.open == open)
                            .copied()
                    })
                    .is_some_and(|group| group.multiline)
            },
        )
    }

    fn previous_significant_token(&self, index: usize) -> Option<usize> {
        index.checked_sub(1)
    }

    fn next_significant_token(&self, index: usize) -> Option<usize> {
        let next = index + 1;
        (next < self.tokens.len() && !matches!(self.tokens[next].kind, TokenKind::Eof))
            .then_some(next)
    }
}

struct Writer {
    output: String,
    indent: usize,
    column: usize,
    line_start: bool,
    /// The indentation level the current line started at.
    line_indent: usize,
}

impl Default for Writer {
    fn default() -> Self {
        Self {
            output: String::new(),
            indent: 0,
            column: 0,
            line_start: true,
            line_indent: 0,
        }
    }
}

impl Writer {
    fn write(&mut self, text: &str) {
        if self.line_start {
            for _ in 0..self.indent * 4 {
                self.output.push(' ');
            }
            self.column = self.indent * 4;
            self.line_start = false;
            self.line_indent = self.indent;
        }
        self.output.push_str(text);
        self.column += text.chars().count();
    }

    /// Writes a text block whose content lines, without indentation, are
    /// `lines`: the opening `"""` here, then each line and the closing
    /// `"""` at the opening delimiter's indentation when it starts its
    /// line, or one level deeper than the line it opens on otherwise. An
    /// empty content line stays empty, so no trailing whitespace is added;
    /// every other line is written exactly as it was, after the new
    /// indentation.
    fn write_text_block(&mut self, lines: &[String]) {
        let starts_line = self.line_start;
        self.write("\"\"\"");
        let level = if starts_line {
            self.line_indent
        } else {
            self.line_indent + 1
        };
        let indentation = " ".repeat(level * 4);
        for line in lines {
            self.output.push('\n');
            if !line.is_empty() {
                self.output.push_str(&indentation);
                self.output.push_str(line);
            }
        }
        self.output.push('\n');
        self.output.push_str(&indentation);
        self.output.push_str("\"\"\"");
        self.column = indentation.len() + 3;
    }

    fn space(&mut self) {
        if !self.line_start && !self.output.ends_with(' ') {
            self.output.push(' ');
            self.column += 1;
        }
    }

    fn ensure_newlines(&mut self, count: usize) {
        let existing = self
            .output
            .as_bytes()
            .iter()
            .rev()
            .take_while(|byte| **byte == b'\n')
            .count();
        for _ in existing..count {
            self.output.push('\n');
        }
        self.column = 0;
        self.line_start = true;
    }

    const fn is_line_start(&self) -> bool {
        self.line_start
    }

    const fn column(&self) -> usize {
        self.column
    }

    fn finish(mut self) -> String {
        while self.output.ends_with('\n') {
            self.output.pop();
        }
        self.output.push('\n');
        self.output
    }
}

struct SyntaxMap {
    doc_spans: Vec<Span>,
    block_spans: HashSet<(usize, usize)>,
    call_group_spans: HashSet<(usize, usize)>,
    /// The parentheses and brackets of patterns, including those of
    /// let-else patterns and nested patterns, which are not followed by `=>`.
    pattern_group_spans: HashSet<(usize, usize)>,
    type_tokens: Vec<bool>,
    generic_commas: Vec<bool>,
    breaks_after: HashMap<usize, usize>,
    let_else_arms: HashMap<usize, usize>,
    pun_values: Vec<PunCandidate>,
}

/// A construction field written `name: name`, which the canonical form
/// writes punned as `name`.
#[derive(Clone, Copy, Debug)]
struct PunCandidate {
    /// The end of the field name.
    name_end: usize,
    /// The span of the value, a single identifier token.
    value: Span,
}

impl SyntaxMap {
    fn new(program: &Program, tokens: &[Token]) -> Self {
        let mut collector = SyntaxCollector::default();
        collector.program(program);

        let mut type_tokens = vec![false; tokens.len()];
        let mut generic_commas = vec![false; tokens.len()];
        for (index, token) in tokens.iter().enumerate() {
            type_tokens[index] = collector
                .type_spans
                .iter()
                .any(|span| token.span.start >= span.start && token.span.end <= span.end);
            generic_commas[index] = matches!(token.kind, TokenKind::Comma)
                && collector
                    .result_separator_spans
                    .iter()
                    .any(|span| token.span.start >= span.start && token.span.end <= span.end);
        }
        let call_group_spans = collector
            .call_spans
            .iter()
            .map(|span| {
                let open = tokens
                    .iter()
                    .find(|token| {
                        token.span.start >= span.start
                            && token.span.end <= span.end
                            && matches!(token.kind, TokenKind::LeftParen)
                    })
                    .expect("a parsed call has an argument opening delimiter");
                let close = tokens
                    .iter()
                    .find(|token| {
                        token.span.end == span.end && matches!(token.kind, TokenKind::RightParen)
                    })
                    .expect("a parsed call has an argument closing delimiter");
                (open.span.start, close.span.end)
            })
            .collect();
        // Tokens are in source order, so each token is found by binary search.
        let index_ending = |end: usize| {
            let index = tokens.partition_point(|token| token.span.end < end);
            debug_assert_eq!(tokens[index].span.end, end);
            index
        };
        let pattern_group_spans = collector
            .pattern_spans
            .iter()
            .map(|span| {
                // A binding pattern is a variant name, `(`, its payload, and
                // the `)` that ends it.
                let open =
                    &tokens[tokens.partition_point(|token| token.span.start < span.start) + 1];
                debug_assert!(matches!(open.kind, TokenKind::LeftParen));
                (open.span.start, span.end)
            })
            .chain(
                collector
                    .list_pattern_spans
                    .iter()
                    .map(|span| (span.start, span.end)),
            )
            .collect();
        let let_else_arms = collector
            .let_else_spans
            .iter()
            .map(|(else_span, statement_span)| {
                (
                    index_ending(else_span.end),
                    index_ending(statement_span.end),
                )
            })
            .collect();

        let mut breaks_after = collector.breaks_after;
        for (index, statement) in program.statements.iter().enumerate() {
            let lines = program
                .statements
                .get(index + 1)
                .map_or(1, |next| top_level_break(statement, next));
            breaks_after.insert(statement.span.end, lines);
        }

        fn top_level_break(current: &Statement, next: &Statement) -> usize {
            let has_function_declaration =
                is_declaration(&current.kind) || is_declaration(&next.kind);
            let changes_between_declaration_and_expression = !matches!(
                (&current.kind, &next.kind),
                (
                    StatementKind::Let { .. } | StatementKind::LetElse { .. },
                    StatementKind::Let { .. } | StatementKind::LetElse { .. }
                ) | (StatementKind::Expression(_), StatementKind::Expression(_))
            );
            if has_function_declaration || changes_between_declaration_and_expression {
                2
            } else {
                1
            }
        }

        const fn is_declaration(kind: &StatementKind) -> bool {
            matches!(
                kind,
                StatementKind::Function { .. }
                    | StatementKind::Webhook { .. }
                    | StatementKind::QueueConsumer { .. }
                    | StatementKind::ScheduleHandler { .. }
            )
        }

        Self {
            doc_spans: collector.doc_spans,
            block_spans: collector.block_spans,
            call_group_spans,
            pattern_group_spans,
            type_tokens,
            generic_commas,
            breaks_after,
            let_else_arms,
            pun_values: collector.pun_values,
        }
    }
}

/// Writes every line of an attached doc comment with exactly one space
/// between its `///` and its text (none when the text is empty), leaving the
/// text itself as written. Other comments, including `///` lines that no item
/// follows, keep their exact text.
///
/// Comments are in source order and doc comments never overlap, so one sweep
/// over both finds the doc comment, if any, that holds each comment.
fn canonicalize_doc_comments(comments: &mut [Comment], doc_spans: &[Span]) {
    let mut doc_spans = doc_spans.to_vec();
    doc_spans.sort_unstable_by_key(|span| span.start);
    let mut docs = doc_spans.iter().peekable();
    for comment in comments {
        while docs
            .next_if(|span| span.end <= comment.span.start)
            .is_some()
        {}
        let attached = docs
            .peek()
            .is_some_and(|span| comment.span.start >= span.start && comment.span.start < span.end);
        if !attached {
            continue;
        }
        if let Some(text) = parser::doc_line_text(&comment.text) {
            comment.text = if text.is_empty() {
                "///".to_owned()
            } else {
                format!("/// {text}")
            };
        }
    }
}

/// Marks the `:` and value tokens of every pun candidate, so the field is
/// written punned. A field with a comment between its name and the end of
/// its value keeps its `name: name` spelling, so no comment moves.
fn punned_tokens(tokens: &[Token], comments: &[Comment], candidates: &[PunCandidate]) -> Vec<bool> {
    let mut omitted = vec![false; tokens.len()];
    for candidate in candidates {
        let commented = {
            let first = comments.partition_point(|comment| comment.span.end <= candidate.name_end);
            comments
                .get(first)
                .is_some_and(|comment| comment.span.start < candidate.value.end)
        };
        if commented {
            continue;
        }
        let value = tokens.partition_point(|token| token.span.start < candidate.value.start);
        let is_value = tokens
            .get(value)
            .is_some_and(|token| token.span == candidate.value);
        let colon = value.checked_sub(1).filter(|colon| {
            tokens[*colon].kind == TokenKind::Colon
                && tokens[*colon].span.start >= candidate.name_end
        });
        if let (true, Some(colon)) = (is_value, colon) {
            omitted[colon] = true;
            omitted[value] = true;
        }
    }
    omitted
}

#[derive(Default)]
struct SyntaxCollector {
    /// The spans of the doc comments attached to items.
    doc_spans: Vec<Span>,
    block_spans: HashSet<(usize, usize)>,
    call_spans: Vec<Span>,
    type_spans: Vec<Span>,
    result_separator_spans: Vec<Span>,
    breaks_after: HashMap<usize, usize>,
    /// Each variant pattern with a payload: the pattern after `let` of a
    /// let-else, and every such pattern of a `match` arm.
    pattern_spans: Vec<Span>,
    /// Each `[]` or `[head, ..tail]` pattern of a `match` arm.
    list_pattern_spans: Vec<Span>,
    /// The `else` keyword and the whole statement of each let-else.
    let_else_spans: Vec<(Span, Span)>,
    pun_values: Vec<PunCandidate>,
}

/// One syntax-tree node awaiting collection.
enum SyntaxNode<'a> {
    Statement(&'a Statement),
    Block(&'a Block),
    Expression(&'a Expression),
    Annotation(&'a TypeAnnotation),
}

impl SyntaxCollector {
    /// Collects formatting facts with an explicit work stack, so neither deep
    /// nesting nor long `else if` and operator chains deepen native recursion.
    /// Every fact is order-independent.
    fn program(&mut self, program: &Program) {
        let mut pending = program
            .statements
            .iter()
            .map(SyntaxNode::Statement)
            .collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            match node {
                SyntaxNode::Statement(statement) => self.statement(statement, &mut pending),
                SyntaxNode::Block(block) => self.block(block, &mut pending),
                SyntaxNode::Expression(expression) => self.expression(expression, &mut pending),
                SyntaxNode::Annotation(annotation) => self.annotation(annotation, &mut pending),
            }
        }
    }

    fn statement<'a>(&mut self, statement: &'a Statement, pending: &mut Vec<SyntaxNode<'a>>) {
        match &statement.kind {
            StatementKind::Let {
                annotation, value, ..
            } => {
                if let Some(annotation) = annotation {
                    pending.push(SyntaxNode::Annotation(annotation));
                }
                pending.push(SyntaxNode::Expression(value));
            }
            StatementKind::LetElse {
                pattern,
                annotation,
                value,
                else_span,
                arm,
            } => {
                if pattern.binding.is_some() {
                    self.pattern_spans.push(pattern.span);
                }
                self.let_else_spans.push((*else_span, statement.span));
                if let Some(annotation) = annotation {
                    pending.push(SyntaxNode::Annotation(annotation));
                }
                pending.push(SyntaxNode::Expression(value));
                pending.push(SyntaxNode::Expression(&arm.value));
            }
            StatementKind::Function {
                parameters,
                return_type,
                body,
                doc,
                ..
            }
            | StatementKind::Webhook {
                parameters,
                return_type,
                body,
                doc,
                ..
            }
            | StatementKind::QueueConsumer {
                parameters,
                return_type,
                body,
                doc,
                ..
            }
            | StatementKind::ScheduleHandler {
                parameters,
                return_type,
                body,
                doc,
                ..
            } => {
                if let Some(doc) = doc {
                    self.doc_spans.push(doc.span);
                }
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        pending.push(SyntaxNode::Annotation(annotation));
                    }
                }
                if let Some(annotation) = return_type {
                    pending.push(SyntaxNode::Annotation(annotation));
                }
                pending.push(SyntaxNode::Block(body));
            }
            StatementKind::Expression(expression) => {
                pending.push(SyntaxNode::Expression(expression));
            }
        }
    }

    fn block<'a>(&mut self, block: &'a Block, pending: &mut Vec<SyntaxNode<'a>>) {
        self.block_spans.insert((block.span.start, block.span.end));
        for statement in &block.statements {
            pending.push(SyntaxNode::Statement(statement));
            self.breaks_after.insert(statement.span.end, 1);
        }
        if let Some(tail) = &block.tail {
            pending.push(SyntaxNode::Expression(tail));
        }
    }

    fn expression<'a>(&mut self, expression: &'a Expression, pending: &mut Vec<SyntaxNode<'a>>) {
        match &expression.kind {
            ExpressionKind::Literal(_) | ExpressionKind::Unit | ExpressionKind::Variable(_) => {}
            ExpressionKind::Interpolation(parts) => {
                pending.extend(parts.iter().filter_map(|part| match part {
                    StringPart::Hole { value, .. } => Some(SyntaxNode::Expression(value)),
                    StringPart::Text { .. } => None,
                }));
            }
            ExpressionKind::List(elements) => {
                pending.extend(elements.iter().map(SyntaxNode::Expression));
            }
            ExpressionKind::EntryList(entries) => {
                for entry in entries {
                    pending.push(SyntaxNode::Expression(&entry.key));
                    pending.push(SyntaxNode::Expression(&entry.value));
                }
            }
            ExpressionKind::Record(fields) => {
                pending.extend(
                    fields
                        .iter()
                        .map(|field| SyntaxNode::Expression(&field.value)),
                );
            }
            ExpressionKind::Construct { fields, .. } => {
                for field in fields {
                    // Exactly the identifier `name`, not `(name)`.
                    let is_own_name = matches!(
                        &field.value.kind,
                        ExpressionKind::Variable(name) if *name == field.name
                    ) && field.value.span.end - field.value.span.start
                        == field.name.len();
                    if is_own_name && !field.is_punned() {
                        self.pun_values.push(PunCandidate {
                            name_end: field.name_span.end,
                            value: field.value.span,
                        });
                    }
                    pending.push(SyntaxNode::Expression(&field.value));
                }
            }
            ExpressionKind::FieldAccess { value, .. } => {
                pending.push(SyntaxNode::Expression(value));
            }
            ExpressionKind::Block(block) => pending.push(SyntaxNode::Block(block)),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                pending.push(SyntaxNode::Expression(condition));
                pending.push(SyntaxNode::Block(consequent));
                pending.push(SyntaxNode::Expression(alternative));
            }
            ExpressionKind::Function {
                parameters,
                return_type,
                body,
            } => {
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        pending.push(SyntaxNode::Annotation(annotation));
                    }
                }
                if let Some(annotation) = return_type {
                    pending.push(SyntaxNode::Annotation(annotation));
                }
                pending.push(SyntaxNode::Block(body));
            }
            ExpressionKind::Call { callee, arguments } => {
                self.call_spans
                    .push(Span::new(callee.span.end, expression.span.end));
                pending.push(SyntaxNode::Expression(callee));
                pending.extend(arguments.iter().map(SyntaxNode::Expression));
            }
            ExpressionKind::Match { subject, arms } => {
                pending.push(SyntaxNode::Expression(subject));
                for arm in arms {
                    self.match_pattern(&arm.pattern);
                    pending.push(SyntaxNode::Expression(&arm.value));
                }
            }
            ExpressionKind::Unary { operand, .. } | ExpressionKind::Propagate { operand, .. } => {
                pending.push(SyntaxNode::Expression(operand));
            }
            ExpressionKind::Binary { left, right, .. } => {
                pending.push(SyntaxNode::Expression(left));
                pending.push(SyntaxNode::Expression(right));
            }
        }
    }

    /// Records the delimiters of `pattern` and of every pattern nested in it.
    /// Patterns are paths, so this walks one iteratively.
    fn match_pattern(&mut self, pattern: &Pattern) {
        let mut current = pattern;
        loop {
            match &current.kind {
                PatternKind::Binder(_) | PatternKind::Wildcard => return,
                PatternKind::EmptyList | PatternKind::Cons { .. } => {
                    self.list_pattern_spans.push(current.span);
                    return;
                }
                PatternKind::Variant { payload, .. } => {
                    let Some(payload) = payload else {
                        return;
                    };
                    self.pattern_spans.push(current.span);
                    current = payload;
                }
            }
        }
    }

    fn annotation<'a>(
        &mut self,
        annotation: &'a TypeAnnotation,
        pending: &mut Vec<SyntaxNode<'a>>,
    ) {
        self.type_spans.push(annotation.span);
        match &annotation.kind {
            TypeKind::List(element) | TypeKind::Option(element) => {
                pending.push(SyntaxNode::Annotation(element));
            }
            TypeKind::Result(value, error) => {
                self.result_separator_spans
                    .push(Span::new(value.span.end, error.span.start));
                pending.push(SyntaxNode::Annotation(value));
                pending.push(SyntaxNode::Annotation(error));
            }
            TypeKind::Record(fields) => {
                pending.extend(
                    fields
                        .iter()
                        .map(|field| SyntaxNode::Annotation(&field.annotation)),
                );
            }
            TypeKind::Int
            | TypeKind::Bool
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::HttpHeader
            | TypeKind::HttpRequest
            | TypeKind::HttpResponse
            | TypeKind::LogField
            | TypeKind::QueueJob
            | TypeKind::ScheduleEvent
            | TypeKind::DatabaseTransaction
            | TypeKind::Secret => {}
        }
    }
}

fn build_groups(
    tokens: &[Token],
    token_text: &[String],
    comments: &[Comment],
    block_spans: &HashSet<(usize, usize)>,
    call_group_spans: &HashSet<(usize, usize)>,
    pattern_group_spans: &HashSet<(usize, usize)>,
    omitted: &[bool],
) -> (Vec<Option<GroupInfo>>, Vec<Option<usize>>) {
    let mut pairs = vec![None; tokens.len()];
    let mut close_to_open = vec![None; tokens.len()];
    let mut stack = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        if is_open_delimiter(&token.kind) {
            stack.push(index);
        } else if is_close_delimiter(&token.kind) {
            let open = stack
                .pop()
                .expect("parseable source has balanced delimiters");
            debug_assert!(delimiters_match(&tokens[open].kind, &token.kind));
            pairs[open] = Some(index);
            close_to_open[index] = Some(open);
        }
    }
    debug_assert!(stack.is_empty());

    let mut groups: Vec<Option<GroupInfo>> = vec![None; tokens.len()];
    for open in (0..tokens.len()).rev() {
        let Some(close) = pairs[open] else {
            continue;
        };
        let kind = classify_group(
            open,
            close,
            tokens,
            block_spans,
            call_group_spans,
            pattern_group_spans,
        );
        let has_comment = comments.iter().any(|comment| {
            comment.span.start >= tokens[open].span.end
                && comment.span.end <= tokens[close].span.start
        });
        let has_tokens = close > open + 1;
        let has_content = has_comment || has_tokens;
        let forced_multiline =
            has_content && (has_comment || matches!(kind, GroupKind::Block | GroupKind::Match));
        groups[open] = Some(GroupInfo {
            close,
            kind,
            has_content,
            forced_multiline,
            flat_width: 0,
        });
    }

    // Tokens the flat width leaves out: omitted pun tokens, and trailing
    // commas, which a single-line group drops.
    let mut unrendered = omitted.to_vec();
    for info in groups.iter().flatten() {
        if info.kind.allows_trailing_comma()
            && info.close > 0
            && matches!(tokens[info.close - 1].kind, TokenKind::Comma)
        {
            unrendered[info.close - 1] = true;
        }
    }

    for open in (0..tokens.len()).rev() {
        let Some(info) = groups[open] else {
            continue;
        };
        let flat_width = canonical_flat_width(open, info.close, token_text, &unrendered);
        groups[open]
            .as_mut()
            .expect("opening delimiter should have group information")
            .flat_width = flat_width;
    }

    (groups, close_to_open)
}

fn classify_group(
    open: usize,
    close: usize,
    tokens: &[Token],
    block_spans: &HashSet<(usize, usize)>,
    call_group_spans: &HashSet<(usize, usize)>,
    pattern_group_spans: &HashSet<(usize, usize)>,
) -> GroupKind {
    match tokens[open].kind {
        TokenKind::LeftBrace => {
            if block_spans.contains(&(tokens[open].span.start, tokens[close].span.end)) {
                GroupKind::Block
            } else if contains_top_level_fat_arrow(open, close, tokens) {
                GroupKind::Match
            } else {
                GroupKind::BracedCollection
            }
        }
        TokenKind::LeftBracket => {
            if token_after(close, tokens)
                .is_some_and(|token| matches!(token.kind, TokenKind::FatArrow))
                || pattern_group_spans.contains(&(tokens[open].span.start, tokens[close].span.end))
            {
                GroupKind::Pattern
            } else {
                GroupKind::List
            }
        }
        TokenKind::LeftParen => {
            if token_after(close, tokens)
                .is_some_and(|token| matches!(token.kind, TokenKind::FatArrow))
                || pattern_group_spans.contains(&(tokens[open].span.start, tokens[close].span.end))
            {
                GroupKind::Pattern
            } else if is_parameter_group(open, tokens) {
                GroupKind::Parameters
            } else if call_group_spans.contains(&(tokens[open].span.start, tokens[close].span.end))
            {
                GroupKind::Call
            } else {
                GroupKind::Grouping
            }
        }
        _ => unreachable!("only opening delimiters are classified"),
    }
}

fn contains_top_level_fat_arrow(open: usize, close: usize, tokens: &[Token]) -> bool {
    let mut depth = 0usize;
    for token in &tokens[open + 1..close] {
        if is_open_delimiter(&token.kind) {
            depth += 1;
        } else if is_close_delimiter(&token.kind) {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && matches!(token.kind, TokenKind::FatArrow) {
            return true;
        }
    }
    false
}

fn is_parameter_group(open: usize, tokens: &[Token]) -> bool {
    open > 0
        && (matches!(tokens[open - 1].kind, TokenKind::Fn)
            || open > 1
                && matches!(tokens[open - 2].kind, TokenKind::Fn)
                && matches!(tokens[open - 1].kind, TokenKind::Identifier(_)))
}

fn token_after(index: usize, tokens: &[Token]) -> Option<&Token> {
    tokens
        .get(index + 1)
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
}

fn canonical_token_text(token: &Token) -> String {
    match &token.kind {
        TokenKind::Identifier(value) | TokenKind::Integer(value) => value.clone(),
        TokenKind::String(value) => canonical_string(value),
        // A text block is laid out by `Writer::write_text_block`; within a
        // line it is as wide as its opening delimiter.
        TokenKind::Template(template) if template.text_block.is_some() => "\"\"\"".to_owned(),
        TokenKind::Template(template) => canonical_template(&template.parts),
        TokenKind::Let => "let".to_owned(),
        TokenKind::Fn => "fn".to_owned(),
        TokenKind::Webhook => "webhook".to_owned(),
        TokenKind::If => "if".to_owned(),
        TokenKind::Else => "else".to_owned(),
        TokenKind::Match => "match".to_owned(),
        TokenKind::Record => "record".to_owned(),
        TokenKind::True => "true".to_owned(),
        TokenKind::False => "false".to_owned(),
        TokenKind::LeftParen => "(".to_owned(),
        TokenKind::RightParen => ")".to_owned(),
        TokenKind::LeftBrace => "{".to_owned(),
        TokenKind::RightBrace => "}".to_owned(),
        TokenKind::LeftBracket => "[".to_owned(),
        TokenKind::RightBracket => "]".to_owned(),
        TokenKind::Comma => ",".to_owned(),
        TokenKind::Colon => ":".to_owned(),
        TokenKind::Semicolon => ";".to_owned(),
        TokenKind::Question => "?".to_owned(),
        TokenKind::Dot => ".".to_owned(),
        TokenKind::Equal => "=".to_owned(),
        TokenKind::EqualEqual => "==".to_owned(),
        TokenKind::Bang => "!".to_owned(),
        TokenKind::BangEqual => "!=".to_owned(),
        TokenKind::Less => "<".to_owned(),
        TokenKind::LessEqual => "<=".to_owned(),
        TokenKind::Greater => ">".to_owned(),
        TokenKind::GreaterEqual => ">=".to_owned(),
        TokenKind::Plus => "+".to_owned(),
        TokenKind::Minus => "-".to_owned(),
        TokenKind::Star => "*".to_owned(),
        TokenKind::Slash => "/".to_owned(),
        TokenKind::Percent => "%".to_owned(),
        TokenKind::AndAnd => "&&".to_owned(),
        TokenKind::OrOr => "||".to_owned(),
        TokenKind::DotDot => "..".to_owned(),
        TokenKind::FatArrow => "=>".to_owned(),
        TokenKind::ThinArrow => "->".to_owned(),
        TokenKind::Eof => String::new(),
    }
}

fn canonical_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    push_canonical_text(&mut output, value);
    output.push('"');
    output
}

/// A one-line interpolated string: its text written as in any string, and
/// each hole exactly as written, `\(name.field)`.
fn canonical_template(parts: &[TemplatePart]) -> String {
    let mut output = String::from("\"");
    for part in parts {
        match part {
            TemplatePart::Text { value, .. } => push_canonical_text(&mut output, value),
            TemplatePart::Hole { path, .. } => {
                output.push_str("\\(");
                for (index, (name, _)) in path.iter().enumerate() {
                    if index > 0 {
                        output.push('.');
                    }
                    output.push_str(name);
                }
                output.push(')');
            }
        }
    }
    output.push('"');
    output
}

/// Appends `value` as the inside of a canonical string literal.
fn push_canonical_text(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\0' => output.push_str("\\0"),
            character if character < '\u{20}' => {
                use std::fmt::Write;
                write!(output, "\\u{{{:x}}}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => output.push(character),
        }
    }
}

fn canonical_flat_width(
    open: usize,
    close: usize,
    token_text: &[String],
    unrendered: &[bool],
) -> usize {
    (open..=close)
        .filter(|index| !unrendered[*index])
        .map(|index| token_text[index].chars().count() + 1)
        .sum()
}

fn token_pair_needs_space(
    tokens: &[Token],
    groups: &[Option<GroupInfo>],
    close_to_open: &[Option<usize>],
    type_tokens: &[bool],
    previous: usize,
    current: usize,
    group_is_multiline: impl Fn(usize) -> bool,
) -> bool {
    let previous_kind = &tokens[previous].kind;
    let current_kind = &tokens[current].kind;

    if matches!(
        current_kind,
        TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::Dot
            | TokenKind::Colon
            | TokenKind::Question
    ) || matches!(previous_kind, TokenKind::Dot | TokenKind::DotDot)
    {
        return false;
    }

    if is_generic_punctuation(type_tokens, tokens, current)
        || (is_generic_punctuation(type_tokens, tokens, previous)
            && matches!(previous_kind, TokenKind::Less))
    {
        return false;
    }

    if matches!(current_kind, TokenKind::RightBrace) {
        return close_to_open[current]
            .and_then(|open| groups[open].map(|group| (open, group.kind)))
            .is_some_and(|(open, kind)| {
                !group_is_multiline(open)
                    && kind.uses_brace_padding()
                    && !matches!(previous_kind, TokenKind::LeftBrace)
            });
    }

    if matches!(previous_kind, TokenKind::LeftParen | TokenKind::LeftBracket) {
        return false;
    }

    if matches!(previous_kind, TokenKind::LeftBrace) {
        return groups[previous]
            .is_some_and(|group| !group_is_multiline(previous) && group.kind.uses_brace_padding());
    }

    if matches!(previous_kind, TokenKind::Comma | TokenKind::Colon) {
        return true;
    }

    if matches!(current_kind, TokenKind::LeftParen) {
        let kind = groups[current]
            .map(|group| group.kind)
            .unwrap_or(GroupKind::Grouping);
        return matches!(kind, GroupKind::Grouping)
            && (is_word(previous_kind)
                || is_operator(previous_kind) && !is_unary(tokens, previous));
    }

    if matches!(current_kind, TokenKind::LeftBracket) {
        return matches!(
            previous_kind,
            TokenKind::If | TokenKind::Match | TokenKind::Comma | TokenKind::Colon
        ) || is_operator(previous_kind);
    }

    if matches!(current_kind, TokenKind::LeftBrace) {
        return true;
    }

    if is_operator(current_kind) {
        return if is_unary(tokens, current) {
            !matches!(
                previous_kind,
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace
            ) && (!is_operator(previous_kind) || !is_unary(tokens, previous))
        } else {
            true
        };
    }

    if is_operator(previous_kind) {
        return !is_unary(tokens, previous);
    }

    is_word(previous_kind) && is_word(current_kind)
        || matches!(
            previous_kind,
            TokenKind::RightParen
                | TokenKind::RightBracket
                | TokenKind::RightBrace
                | TokenKind::Question
        ) && is_word(current_kind)
}

fn is_generic_punctuation(type_tokens: &[bool], tokens: &[Token], index: usize) -> bool {
    type_tokens[index] && matches!(tokens[index].kind, TokenKind::Less | TokenKind::Greater)
}

fn is_unary(tokens: &[Token], index: usize) -> bool {
    match tokens[index].kind {
        TokenKind::Bang => true,
        TokenKind::Minus => index
            .checked_sub(1)
            .is_none_or(|previous| !can_end_expression(&tokens[previous].kind)),
        _ => false,
    }
}

fn is_word(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Integer(_)
            | TokenKind::String(_)
            | TokenKind::Template(_)
            | TokenKind::Let
            | TokenKind::Fn
            | TokenKind::Webhook
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::Match
            | TokenKind::Record
            | TokenKind::True
            | TokenKind::False
    )
}

fn is_operator(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Equal
            | TokenKind::EqualEqual
            | TokenKind::Bang
            | TokenKind::BangEqual
            | TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::FatArrow
            | TokenKind::ThinArrow
    )
}

fn can_end_expression(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Integer(_)
            | TokenKind::String(_)
            | TokenKind::Template(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::Question
    )
}

fn is_open_delimiter(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LeftParen | TokenKind::LeftBrace | TokenKind::LeftBracket
    )
}

fn is_close_delimiter(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RightParen | TokenKind::RightBrace | TokenKind::RightBracket
    )
}

fn delimiters_match(open: &TokenKind, close: &TokenKind) -> bool {
    matches!(
        (open, close),
        (TokenKind::LeftParen, TokenKind::RightParen)
            | (TokenKind::LeftBrace, TokenKind::RightBrace)
            | (TokenKind::LeftBracket, TokenKind::RightBracket)
    )
}

#[cfg(test)]
mod tests {
    use crate::{Source, analyze, parse_source};

    use super::*;

    fn format(text: &str) -> String {
        format_source(&Source::new("test.krit", text)).expect("source should format")
    }

    #[test]
    fn formats_long_else_if_chains_and_rejects_excessive_nesting() {
        // Runs on the default test stack: the chain is parsed and collected
        // iteratively.
        let mut text = String::from("let n = 1;\nlet value = if n == 0 { 0 }");
        for branch in 1..1_000 {
            text.push_str(&format!(" else if n == {branch} {{ {branch} }}"));
        }
        text.push_str(" else { -1 };\n");
        let formatted = format(&text);
        assert_eq!(format(&formatted), formatted);
        assert_eq!(formatted.matches("} else if n == ").count(), 999);

        let deep = format!("let value = {}1{};\n", "[".repeat(2_000), "]".repeat(2_000));
        let error =
            crate::run_on_thread("formatter-test", crate::COMPILER_STACK_BYTES, move || {
                format_source(&Source::new("deep.krit", deep))
            })
            .expect("formatter test thread should start")
            .expect_err("deep nesting should not format");
        assert_eq!(error.code(), "K1005");
    }

    #[test]
    fn formats_core_syntax_and_is_idempotent() {
        let source =
            "fn add(a:Int,b:Int)->Int{a+b}\r\nlet values=[1,2,3,];\r\nprintln(add(values[0],2));";
        let error = format_source(&Source::new("test.krit", source))
            .expect_err("unsupported indexing should remain a syntax error");
        assert_eq!(error.code(), "K1108");

        let source = "fn add(a:Int,b:Int)->Int{a+b}\r\nlet values=[1,2,3,];\r\nprintln(add(1,2));";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn add(a: Int, b: Int) -> Int {\n    a + b\n}\n\nlet values = [1, 2, 3];\n\nprintln(add(1, 2));\n"
        );
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn formats_postfix_question_marks_as_tight_call_links() {
        let source = "fn f(a,pick,flag)->Option<Int>{let b=a ?;let c=pick( ) ? (1) ? .x;\n\
                      if flag ? {Some(- b ?? - c)}else{Some(a? -1)}}";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn f(a, pick, flag) -> Option<Int> {\n    let b = a?;\n    let c = pick()?(1)?.x;\n    \
             if flag? {\n        Some(-b?? - c)\n    } else {\n        Some(a? - 1)\n    }\n}\n"
        );
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn formats_nested_and_wildcard_patterns_compactly() {
        let source = "fn f(r,items)->Int{match r{Ok( Some( v ) )=>v,Ok(None)=>match items{\n\
                      [ _ , .. rest ]=>0,[ ]=>1},Err( _ )=>2}}";
        let expected = "fn f(r, items) -> Int {\n    match r {\n        Ok(Some(v)) => v,\n        \
                        Ok(None) => match items {\n            [_, ..rest] => 0,\n            \
                        [] => 1,\n        },\n        Err(_) => 2,\n    }\n}\n";
        assert_eq!(format(source), expected);
        assert_eq!(format(expected), expected);
        // A nested list pattern is a pattern, not a list literal: it neither
        // gains a trailing comma nor a space before a nested payload.
        let nested = "fn f(r) -> Int {\n    match r {\n        Ok([head, ..tail]) => head,\n        \
                      Ok([]) => 0,\n        Err(Some(_)) => 1,\n        Err(None) => 2,\n    }\n}\n";
        assert_eq!(format(nested), nested);
    }

    #[test]
    fn formats_let_else_on_one_line_or_breaks_after_else() {
        let source = "fn f(r,o)->Int{let Ok ( v ):Result<Int,String> =r else Err( e )=>0;\n\
                      let None=o?else Some(x)=>x;v}";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn f(r, o) -> Int {\n    let Ok(v): Result<Int, String> = r else Err(e) => 0;\n    \
             let None = o? else Some(x) => x;\n    v\n}\n"
        );
        assert_eq!(format(&formatted), formatted);

        // The arm stays on the line exactly when the rest of the statement
        // fits within 100 columns there.
        let fits = format!(
            "fn f(r) -> Int {{\n    let Ok(value) = r else Err(error) => {};\n    value\n}}\n",
            "x".repeat(100 - 4 - "let Ok(value) = r else Err(error) => ;".len())
        );
        assert_eq!(format(&fits), fits);
        let long_name = "x".repeat(101 - 4 - "let Ok(value) = r else Err(error) => ;".len());
        assert_eq!(
            format(&format!(
                "fn f(r) -> Int {{ let Ok(value) = r else Err(error) => {long_name}; value }}"
            )),
            format!(
                "fn f(r) -> Int {{\n    let Ok(value) = r else\n        Err(error) => {long_name};\n    \
                 value\n}}\n"
            )
        );

        // A block or a comment in the arm always moves it to its own line,
        // and a group that would wrap on its own line moves it too.
        let formatted = format(
            "fn f(r) -> Int { let Ok(v) = r else Err(e) => { println(e); 0 }; v }\n\
             fn g(r) -> Int { let Ok(v) = r else // fallback\nErr(e) => 0; v }",
        );
        assert_eq!(
            formatted,
            "fn f(r) -> Int {\n    let Ok(v) = r else\n        Err(e) => {\n            println(e);\n            \
             0\n        };\n    v\n}\n\nfn g(r) -> Int {\n    let Ok(v) = r else // fallback\n        \
             Err(e) => 0;\n    v\n}\n"
        );
        assert_eq!(format(&formatted), formatted);
        let call = format!("handle({})", "a".repeat(60));
        let formatted = format(&format!(
            "fn f(r) -> Int {{ let Ok(value) = r else Err(error) => {call}; value }}"
        ));
        assert!(
            formatted.contains(&format!("else\n        Err(error) => {call};\n")),
            "{formatted}"
        );
        assert_eq!(format(&formatted), formatted);

        // A value that wraps keeps a short arm after its closing delimiter.
        let argument = "a".repeat(90);
        let formatted = format(&format!(
            "fn f() -> Int {{ let Ok(value) = load({argument}) else Err(error) => 0; value }}"
        ));
        assert_eq!(
            formatted,
            format!(
                "fn f() -> Int {{\n    let Ok(value) = load(\n        {argument},\n    ) else Err(error) => 0;\n    \
                 value\n}}\n"
            )
        );
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn let_else_arms_are_measured_as_written_with_punning_and_no_trailing_comma() {
        // The arm fits once its `body: body` field is written punned and its
        // trailing comma dropped, so it stays on the line in both spellings;
        // measuring the input's spelling would break it in one formatting and
        // join it again in the next.
        let fits = "fn f(key: String, body: String) -> HttpResponse {\n    \
                    let Ok(value) = load(keyyyyyyyyyyyyyy) else Err(error) => HttpResponse { status: 500, body };\n    \
                    HttpResponse { status: 200, body: value }\n}\n";
        assert!(fits.lines().all(|line| line.len() <= 100), "{fits}");
        for spelling in ["body: body", "body: body,", "body,"] {
            let source = fits.replace("body };", &format!("{spelling} }};"));
            assert_eq!(format(&source), fits, "{spelling}");
        }
        assert_eq!(format(fits), fits);

        // One column more breaks after `else` in every spelling.
        let breaks = fits.replace("keyyyyyyyyyyyyyy", "keyyyyyyyyyyyyyyy");
        let expected = breaks.replace(
            " else Err(error) => HttpResponse",
            " else\n        Err(error) => HttpResponse",
        );
        for spelling in ["body: body", "body: body,", "body"] {
            let source = breaks.replace("body };", &format!("{spelling} }};"));
            let formatted = format(&source);
            assert_eq!(formatted, expected, "{spelling}");
            assert_eq!(format(&formatted), formatted, "{spelling}");
        }
    }

    #[test]
    fn preserves_comments_in_source_order() {
        let source = "// head\nlet value=1+2;// inline\n// middle\nprintln( // call\nvalue// argument\n);// tail\n";
        let formatted = format(source);
        assert_eq!(formatted.matches("//").count(), 6);
        assert!(formatted.contains("// head"));
        assert!(formatted.contains("; // inline"));
        assert!(formatted.contains("// middle"));
        assert!(formatted.contains("( // call"));
        assert!(formatted.contains("value, // argument"));
        assert!(formatted.contains("); // tail"));
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn preserves_precedence_and_static_analysis_outcome() {
        let source = Source::new(
            "test.krit",
            "let value: Int = (1 + 2) * 3 - -4;\nlet broken: Bool = value;\n",
        );
        let before = parse_source(&source).and_then(|program| analyze(&program));
        let formatted = format_source(&source).expect("source should format");
        let formatted_source = Source::new("test.krit", formatted);
        let after = parse_source(&formatted_source).and_then(|program| analyze(&program));

        assert_eq!(
            before.expect_err("source should fail").code(),
            after.expect_err("formatted source should fail").code()
        );
    }

    #[test]
    fn canonicalizes_string_escapes() {
        assert_eq!(
            format(r#"println("\u{48}ello\t\"Krit\"\\");"#),
            "println(\"Hello\\t\\\"Krit\\\"\\\\\");\n"
        );
    }

    #[test]
    fn normalizes_crlf_and_tabs_around_comments() {
        assert_eq!(
            format("// heading\r\n\tlet value=1;\t// result\r\n"),
            "// heading\nlet value = 1; // result\n"
        );
    }

    #[test]
    fn keeps_standalone_comments_on_their_own_line() {
        assert_eq!(
            format("if true { 1 }\n// between branches\nelse { 2 };\n"),
            "if true {\n    1\n}\n// between branches\nelse {\n    2\n};\n"
        );
    }

    #[test]
    fn wraps_long_delimited_groups_with_trailing_commas() {
        let source = "fn combine(first_value:Int,second_value:Int,third_value:Int,fourth_value:Int,fifth_value:Int,sixth_value:Int,seventh_value:Int,eighth_value:Int,ninth_value:Int,tenth_value:Int)->List<Int>{[first_value,second_value,third_value,fourth_value,fifth_value,sixth_value,seventh_value,eighth_value,ninth_value,tenth_value]}";
        let formatted = format(source);

        assert!(formatted.contains("fn combine(\n"));
        assert!(formatted.contains("    tenth_value: Int,\n) -> List<Int>"));
        assert!(formatted.contains("    [\n        first_value,\n"));
        assert!(formatted.contains("        tenth_value,\n    ]"));
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn distinguishes_grouping_after_function_declarations_from_calls() {
        let long_sum = "1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1";

        let top_level_comment = format("fn helper() -> Int { 1 }\n\n( // grouped\n    1 + 2\n);\n");
        assert_eq!(
            top_level_comment,
            "fn helper() -> Int {\n    1\n}\n\n( // grouped\n    1 + 2\n);\n"
        );
        parse_source(&Source::new("top-level-comment.krit", top_level_comment))
            .expect("formatted top-level grouping should parse");

        let top_level_width = format(&format!("fn helper() -> Int {{ 1 }}\n\n({long_sum});\n"));
        assert_eq!(
            top_level_width,
            format!("fn helper() -> Int {{\n    1\n}}\n\n(\n    {long_sum}\n);\n")
        );
        parse_source(&Source::new("top-level-width.krit", top_level_width))
            .expect("formatted top-level grouping should parse");

        let nested_comment = format(
            "fn outer() -> Int {\n    fn helper() -> Int { 1 }\n    ( // grouped\n        1 + 2\n    );\n    0\n}\n",
        );
        assert_eq!(
            nested_comment,
            "fn outer() -> Int {\n    fn helper() -> Int {\n        1\n    }\n    ( // grouped\n        1 + 2\n    );\n    0\n}\n"
        );
        parse_source(&Source::new("nested-comment.krit", nested_comment))
            .expect("formatted nested grouping should parse");

        let nested_width = format(&format!(
            "fn outer() -> Int {{\n    fn helper() -> Int {{ 1 }}\n    ({long_sum});\n    0\n}}\n"
        ));
        assert_eq!(
            nested_width,
            format!(
                "fn outer() -> Int {{\n    fn helper() -> Int {{\n        1\n    }}\n    (\n        {long_sum}\n    );\n    0\n}}\n"
            )
        );
        parse_source(&Source::new("nested-width.krit", nested_width))
            .expect("formatted nested grouping should parse");
    }

    #[test]
    fn preserves_calls_after_block_valued_callees() {
        let formatted =
            format("let value = { fn(value: Int) -> Int { value } }( // call\n    1\n);\n");
        assert_eq!(
            formatted,
            "let value = {\n    fn(value: Int) -> Int {\n        value\n    }\n}( // call\n    1,\n);\n"
        );
        parse_source(&Source::new("block-call.krit", formatted))
            .expect("formatted block-valued callee call should parse");
    }

    #[test]
    fn canonical_group_width_is_independent_of_input_trailing_commas() {
        let growth = "let vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv: List<Int> = [1, 2, 3];\nprintln(match vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv { [] => 0, [head, ..tail] => head });\n";
        let expected_growth = "let vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv: List<Int> = [1, 2, 3];\n\nprintln(match vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv {\n    [] => 0,\n    [head, ..tail] => head,\n});\n";
        let growth_pass_one = format(growth);
        assert_eq!(growth_pass_one, expected_growth);
        assert_eq!(format(&growth_pass_one), growth_pass_one);

        let shrink = "let wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww = record { one: [1, 2, 3,], two: [4, 5, 6,] };\nprintln(wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww);\n";
        let expected_shrink = "let wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww = record { one: [1, 2, 3], two: [4, 5, 6] };\n\nprintln(wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww);\n";
        let shrink_pass_one = format(shrink);
        assert_eq!(shrink_pass_one, expected_shrink);
        assert_eq!(format(&shrink_pass_one), shrink_pass_one);
    }

    #[test]
    fn spaces_grouped_values_and_keyword_list_subjects() {
        assert_eq!(
            format(
                "let r = f(3, (1 + 2));\nlet wrapped = record { value: (1 + 2) };\nif [129] { 1 } else { 2 };\nmatch [129] { [] => 0, [head, ..tail] => head };\n"
            ),
            "let r = f(3, (1 + 2));\nlet wrapped = record { value: (1 + 2) };\n\nif [129] {\n    1\n} else {\n    2\n};\nmatch [129] {\n    [] => 0,\n    [head, ..tail] => head,\n};\n"
        );
    }

    #[test]
    fn writes_construction_fields_punned_exactly_when_the_value_is_the_name() {
        assert_eq!(
            format(
                "let status=1;let body=\"x\";\nlet r=HttpResponse{status:status,body:body,};\nlet s = HttpResponse { status: (status), body: body2 };\n"
            ),
            "let status = 1;\nlet body = \"x\";\nlet r = HttpResponse { status, body };\nlet s = HttpResponse { status: (status), body: body2 };\n"
        );
        // `record { ... }` has no punning.
        assert_eq!(
            format("let r = record { body: body };\n"),
            "let r = record { body: body };\n"
        );
        // A comment between the name and the value keeps both.
        let commented = format("let r = HttpResponse { status: // why\nstatus };\n");
        assert_eq!(
            commented,
            "let r = HttpResponse {\n    status: // why\n    status,\n};\n"
        );
        assert_eq!(format(&commented), commented);
    }

    #[test]
    fn lays_out_entry_lists_like_lists() {
        assert_eq!(
            format("let f=[\"x-a\" : a,\"x-b\":b,];\n"),
            "let f = [\"x-a\": a, \"x-b\": b];\n"
        );
        let long = format(
            "let f = [\"x-request-identifier\": \"0123456789abcdef\", \"x-forwarded-for\": \"203.0.113.7\", \"accept\": \"json\"];\n",
        );
        assert_eq!(
            long,
            "let f = [\n    \"x-request-identifier\": \"0123456789abcdef\",\n    \"x-forwarded-for\": \"203.0.113.7\",\n    \"accept\": \"json\",\n];\n"
        );
        assert_eq!(format(&long), long);
        // A comment inside keeps the list multiline, one entry per line.
        let commented = format("let f = [\"a\": a, // first\n\"b\": b];\n");
        assert_eq!(
            commented,
            "let f = [\n    \"a\": a, // first\n    \"b\": b,\n];\n"
        );
        assert_eq!(format(&commented), commented);
    }

    #[test]
    fn writes_the_unit_value_compactly() {
        assert_eq!(
            format("let u = ( );\nprintln(Ok(( )), [(),( )]);\n"),
            "let u = ();\n\nprintln(Ok(()), [(), ()]);\n"
        );
    }

    #[test]
    fn normalizes_only_the_space_after_attached_doc_comment_slashes() {
        let source = "///Adds.\n///\n/// \n///   kept indentation\nfn add() -> Int { 1 }\n\n\
                      ////banner\n///dangling before a let\nlet x = 1; ///inline\n///at the end\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "/// Adds.\n///\n///\n///   kept indentation\nfn add() -> Int {\n    1\n}\n\n\
             ////banner\n///dangling before a let\nlet x = 1; ///inline\n///at the end\n"
        );
        assert_eq!(format(&formatted), formatted);

        // Formatting keeps every doc comment attached to its item.
        let program = |text: &str| {
            crate::parse_source(&Source::new("doc.krit", text)).expect("source should parse")
        };
        let before = program(source);
        let after = program(&formatted);
        assert_eq!(
            before.statements[0].doc().map(|doc| doc.text.clone()),
            after.statements[0].doc().map(|doc| doc.text.clone())
        );
        assert_eq!(
            after.statements[0].doc().map(|doc| doc.text.as_str()),
            Some("Adds.\n\n\n  kept indentation")
        );
    }

    #[test]
    fn breaks_long_constructions_one_field_per_line() {
        let source = "let r = HttpRequest { method: \"POST\", path: \"/repos/example/issues/1/comments\", headers: [HttpHeader { name: \"accept\", value: \"application/json\" }], body: body };\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "let r = HttpRequest {\n    method: \"POST\",\n    path: \"/repos/example/issues/1/comments\",\n    headers: [HttpHeader { name: \"accept\", value: \"application/json\" }],\n    body,\n};\n"
        );
        assert_eq!(format(&formatted), formatted);
        // The punned spelling is what decides the width: this fits once
        // `body: body` becomes `body`, and a value that cannot be punned
        // does not.
        let method = "m".repeat(48);
        let fits = format(&format!(
            "let r = HttpRequest {{ method: \"{method}\", path, body: body }};\n"
        ));
        assert_eq!(
            fits,
            format!("let r = HttpRequest {{ method: \"{method}\", path, body }};\n")
        );
        let breaks = format(&format!(
            "let r = HttpRequest {{ method: \"{method}\", path, body: body2 }};\n"
        ));
        assert_eq!(breaks.lines().count(), 5, "{breaks}");
    }

    /// The output of running `text`, which must check.
    fn output(text: &str) -> String {
        let mut output = Vec::new();
        crate::run_source(&Source::new("test.krit", text), &mut output).expect("source should run");
        String::from_utf8(output).expect("output should be UTF-8")
    }

    #[test]
    fn keeps_holes_and_canonicalizes_the_text_of_interpolated_strings() {
        let source = "let user=record{name:\"Ada\"};\nprintln(\"\\u{48}i,\\t\\(user.name)\\\\(x)\\u{21}\"  );\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "let user = record { name: \"Ada\" };\n\nprintln(\"Hi,\\t\\(user.name)\\\\(x)!\");\n"
        );
        assert_eq!(format(&formatted), formatted);
        assert_eq!(output(&formatted), output(source));
    }

    #[test]
    fn re_indents_text_blocks_and_preserves_their_content() {
        let source = "fn p(t: String) -> String {\n\"\"\"\n\t\tTitle: \\(t)\n\t\t  deeper \"quoted\"   \n\n\t\t\"\"\"\n}\n\
                      let a = p(\"\"\"\n  x\n  \"\"\");\nprintln(json_encode(a));\n\
                      println(json_encode(HttpResponse{status:1,body:\"\"\"\n                  far\n                  \"\"\"}.body));\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn p(t: String) -> String {\n    \"\"\"\n    Title: \\(t)\n      deeper \"quoted\"   \n\n    \"\"\"\n}\n\n\
             let a = p(\"\"\"\n    x\n    \"\"\");\n\n\
             println(json_encode(a));\n\
             println(json_encode(HttpResponse { status: 1, body: \"\"\"\n    far\n    \"\"\" }.body));\n"
        );
        assert_eq!(format(&formatted), formatted);
        assert_eq!(output(&formatted), output(source));
        assert_eq!(
            output(source),
            "\"Title: x\\n  deeper \\\"quoted\\\"   \\n\"\n\"far\"\n"
        );
    }

    #[test]
    fn text_blocks_in_crlf_sources_format_to_line_feeds() {
        let source =
            "let t = \"\"\"\r\n      a\r\n\r\n        b\r\n      \"\"\";\r\nprintln(t);\r\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "let t = \"\"\"\n    a\n\n      b\n    \"\"\";\n\nprintln(t);\n"
        );
        assert_eq!(output(&formatted), output(source));
        assert_eq!(output(source), "a\n\n  b\n");
    }

    #[test]
    fn a_text_block_in_a_broken_group_indents_from_its_own_line() {
        let long = "x".repeat(60);
        let source = format!(
            "fn f(a: String, b: String, c: String) -> String {{ a }}\n\
             let v = f(\"{long}\", \"{long}\", \"\"\"\n  tail\n  \"\"\");\n"
        );
        let formatted = format(&source);
        assert!(
            formatted.ends_with(&format!(
                "let v = f(\n    \"{long}\",\n    \"{long}\",\n    \"\"\"\n    tail\n    \"\"\",\n);\n"
            )),
            "{formatted}"
        );
        assert_eq!(format(&formatted), formatted);
    }
}
