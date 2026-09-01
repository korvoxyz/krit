use std::collections::{HashMap, HashSet};

use crate::{
    Block, Diagnostic, Expression, ExpressionKind, MatchKind, Program, Source, Span, Statement,
    StatementKind, TypeAnnotation, TypeKind,
    lexer::{Comment, lex_with_comments},
    parser,
    token::{Token, TokenKind},
};

/// The formatter's soft line-width target.
pub const FORMAT_LINE_WIDTH: usize = 100;

/// Formats one parseable edition-2026 source file into canonical Krit source.
pub fn format_source(source: &Source) -> Result<String, Diagnostic> {
    let lexed = lex_with_comments(source)?;
    let program = parser::parse(lexed.tokens.clone())?;
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
    token_text: Vec<String>,
    comments: Vec<Comment>,
    comment_cursor: usize,
    groups: Vec<Option<GroupInfo>>,
    close_to_open: Vec<Option<usize>>,
    type_tokens: Vec<bool>,
    generic_commas: Vec<bool>,
    breaks_after: HashMap<usize, usize>,
    active_groups: Vec<ActiveGroup>,
    closing_group: Option<ActiveGroup>,
    writer: Writer,
    pending_break: usize,
    previous_rendered: Option<usize>,
}

impl Formatter {
    fn new(tokens: Vec<Token>, comments: Vec<Comment>, program: &Program) -> Self {
        let token_text = tokens.iter().map(canonical_token_text).collect::<Vec<_>>();
        let syntax = SyntaxMap::new(program, &tokens);
        let (groups, close_to_open) = build_groups(
            &tokens,
            &token_text,
            &comments,
            &syntax.block_spans,
            &syntax.call_group_spans,
        );
        Self {
            type_tokens: syntax.type_tokens,
            generic_commas: syntax.generic_commas,
            breaks_after: syntax.breaks_after,
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

            if self.should_skip_trailing_comma(index) {
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
        self.writer.write(&self.token_text[index]);
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
}

impl Default for Writer {
    fn default() -> Self {
        Self {
            output: String::new(),
            indent: 0,
            column: 0,
            line_start: true,
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
        }
        self.output.push_str(text);
        self.column += text.chars().count();
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
    block_spans: HashSet<(usize, usize)>,
    call_group_spans: HashSet<(usize, usize)>,
    type_tokens: Vec<bool>,
    generic_commas: Vec<bool>,
    breaks_after: HashMap<usize, usize>,
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

        let mut breaks_after = collector.breaks_after;
        for (index, statement) in program.statements.iter().enumerate() {
            let lines = program
                .statements
                .get(index + 1)
                .map_or(1, |next| top_level_break(statement, next));
            breaks_after.insert(statement.span.end, lines);
        }

        fn top_level_break(current: &Statement, next: &Statement) -> usize {
            let has_function_declaration = matches!(
                current.kind,
                StatementKind::Function { .. } | StatementKind::Webhook { .. }
            ) || matches!(
                next.kind,
                StatementKind::Function { .. } | StatementKind::Webhook { .. }
            );
            let changes_between_declaration_and_expression = !matches!(
                (&current.kind, &next.kind),
                (StatementKind::Let { .. }, StatementKind::Let { .. })
                    | (StatementKind::Expression(_), StatementKind::Expression(_))
            );
            if has_function_declaration || changes_between_declaration_and_expression {
                2
            } else {
                1
            }
        }

        Self {
            block_spans: collector.block_spans,
            call_group_spans,
            type_tokens,
            generic_commas,
            breaks_after,
        }
    }
}

#[derive(Default)]
struct SyntaxCollector {
    block_spans: HashSet<(usize, usize)>,
    call_spans: Vec<Span>,
    type_spans: Vec<Span>,
    result_separator_spans: Vec<Span>,
    breaks_after: HashMap<usize, usize>,
}

impl SyntaxCollector {
    fn program(&mut self, program: &Program) {
        for statement in &program.statements {
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Let {
                annotation, value, ..
            } => {
                if let Some(annotation) = annotation {
                    self.annotation(annotation);
                }
                self.expression(value);
            }
            StatementKind::Function {
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::Webhook {
                parameters,
                return_type,
                body,
                ..
            } => {
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        self.annotation(annotation);
                    }
                }
                if let Some(annotation) = return_type {
                    self.annotation(annotation);
                }
                self.block(body);
            }
            StatementKind::Expression(expression) => self.expression(expression),
        }
    }

    fn block(&mut self, block: &Block) {
        self.block_spans.insert((block.span.start, block.span.end));
        for statement in &block.statements {
            self.statement(statement);
            self.breaks_after.insert(statement.span.end, 1);
        }
        if let Some(tail) = &block.tail {
            self.expression(tail);
        }
    }

    fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
            ExpressionKind::List(elements) => {
                for element in elements {
                    self.expression(element);
                }
            }
            ExpressionKind::Record(fields) => {
                for field in fields {
                    self.expression(&field.value);
                }
            }
            ExpressionKind::FieldAccess { value, .. } => self.expression(value),
            ExpressionKind::Block(block) => self.block(block),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                self.expression(condition);
                self.block(consequent);
                self.expression(alternative);
            }
            ExpressionKind::Function {
                parameters,
                return_type,
                body,
            } => {
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        self.annotation(annotation);
                    }
                }
                if let Some(annotation) = return_type {
                    self.annotation(annotation);
                }
                self.block(body);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.call_spans
                    .push(Span::new(callee.span.end, expression.span.end));
                self.expression(callee);
                for argument in arguments {
                    self.expression(argument);
                }
            }
            ExpressionKind::Match { subject, kind } => {
                self.expression(subject);
                match kind {
                    MatchKind::List {
                        empty_case,
                        cons_case,
                        ..
                    } => {
                        self.expression(empty_case);
                        self.expression(cons_case);
                    }
                    MatchKind::Variants { arms, .. } => {
                        for arm in arms {
                            self.expression(&arm.value);
                        }
                    }
                }
            }
            ExpressionKind::Unary { operand, .. } => self.expression(operand),
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
        }
    }

    fn annotation(&mut self, annotation: &TypeAnnotation) {
        self.type_spans.push(annotation.span);
        match &annotation.kind {
            TypeKind::List(element) | TypeKind::Option(element) => self.annotation(element),
            TypeKind::Result(value, error) => {
                self.result_separator_spans
                    .push(Span::new(value.span.end, error.span.start));
                self.annotation(value);
                self.annotation(error);
            }
            TypeKind::Record(fields) => {
                for field in fields {
                    self.annotation(&field.annotation);
                }
            }
            TypeKind::Int
            | TypeKind::Bool
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::HttpHeader
            | TypeKind::HttpRequest
            | TypeKind::HttpResponse
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
        let kind = classify_group(open, close, tokens, block_spans, call_group_spans);
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

    let mut optional_trailing_commas = vec![false; tokens.len()];
    for info in groups.iter().flatten() {
        if info.kind.allows_trailing_comma()
            && info.close > 0
            && matches!(tokens[info.close - 1].kind, TokenKind::Comma)
        {
            optional_trailing_commas[info.close - 1] = true;
        }
    }

    for open in (0..tokens.len()).rev() {
        let Some(info) = groups[open] else {
            continue;
        };
        let flat_width =
            canonical_flat_width(open, info.close, token_text, &optional_trailing_commas);
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
            {
                GroupKind::Pattern
            } else {
                GroupKind::List
            }
        }
        TokenKind::LeftParen => {
            if token_after(close, tokens)
                .is_some_and(|token| matches!(token.kind, TokenKind::FatArrow))
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
    output.push('"');
    output
}

fn canonical_flat_width(
    open: usize,
    close: usize,
    token_text: &[String],
    optional_trailing_commas: &[bool],
) -> usize {
    (open..=close)
        .filter(|index| !optional_trailing_commas[*index])
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
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
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
            | TokenKind::True
            | TokenKind::False
            | TokenKind::RightParen
            | TokenKind::RightBracket
            | TokenKind::RightBrace
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
    fn formats_core_syntax_and_is_idempotent() {
        let source =
            "fn add(a:Int,b:Int)->Int{a+b}\r\nlet values=[1,2,3,];\r\nprintln(add(values[0],2));";
        let error = format_source(&Source::new("test.krit", source))
            .expect_err("unsupported indexing should remain a syntax error");
        assert_eq!(error.code(), "K1002");

        let source = "fn add(a:Int,b:Int)->Int{a+b}\r\nlet values=[1,2,3,];\r\nprintln(add(1,2));";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn add(a: Int, b: Int) -> Int {\n    a + b\n}\n\nlet values = [1, 2, 3];\n\nprintln(add(1, 2));\n"
        );
        assert_eq!(format(&formatted), formatted);
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
}
