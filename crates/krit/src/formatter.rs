use std::collections::{HashMap, HashSet};

use crate::{
    BinaryOperator, Block, Diagnostic, Expression, ExpressionKind, Pattern, PatternKind, Program,
    Source, Span, Statement, StatementKind, StringPart, TypeAnnotation, TypeKind,
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
    Ok(Formatter::new(source.text(), lexed.tokens, lexed.comments, &program).format())
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

/// A binary operator chain: operands joined by the operators of one
/// precedence level, such as `a + b - c` or `x && y && z`. Each operand may
/// hold chains of its own, of a higher level or in parentheses.
#[derive(Clone, Debug)]
struct OperatorChain {
    /// The first token of the first operand.
    first: usize,
    /// The last token of the last operand.
    last: usize,
    /// The operator tokens, in source order.
    operators: Vec<usize>,
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
    chains: Vec<OperatorChain>,
    /// The chains that start at each token, outermost first.
    chain_starts: HashMap<usize, Vec<usize>>,
    /// The chains that end at each token.
    chain_ends: HashMap<usize, Vec<usize>>,
    /// The chain each operator token belongs to.
    chain_operators: HashMap<usize, usize>,
    /// For each token, the innermost chain whose operands hold it.
    innermost_chains: Vec<Option<usize>>,
    /// For each chain, the innermost chain whose operands hold it.
    parent_chains: Vec<Option<usize>>,
    /// Whether each chain breaks before every operator; decided when its
    /// first token is written.
    broken_chains: Vec<bool>,
    /// The indentation each broken chain adds to its continuation lines.
    chain_indents: Vec<usize>,
    active_groups: Vec<ActiveGroup>,
    closing_group: Option<ActiveGroup>,
    writer: Writer,
    pending_break: usize,
    previous_rendered: Option<usize>,
}

impl Formatter {
    fn new(text: &str, tokens: Vec<Token>, mut comments: Vec<Comment>, program: &Program) -> Self {
        let token_text = tokens
            .iter()
            .map(|token| canonical_token_text(token, text))
            .collect::<Vec<_>>();
        let syntax = SyntaxMap::new(program, &tokens);
        let mut chain_starts = HashMap::<usize, Vec<usize>>::new();
        let mut chain_ends = HashMap::<usize, Vec<usize>>::new();
        let mut chain_operators = HashMap::new();
        for (id, chain) in syntax.chains.iter().enumerate() {
            chain_starts.entry(chain.first).or_default().push(id);
            chain_ends.entry(chain.last).or_default().push(id);
            for operator in &chain.operators {
                chain_operators.insert(*operator, id);
            }
        }
        // Chains that start together nest: the one that ends last holds the
        // others, and is decided first.
        for starting in chain_starts.values_mut() {
            starting.sort_unstable_by_key(|id| std::cmp::Reverse(syntax.chains[*id].last));
        }
        // Chains nest or are disjoint, so marking each chain's tokens, outer
        // chains first, leaves every token marked with its innermost chain.
        let mut innermost_chains = vec![None; tokens.len()];
        let mut outer_first = (0..syntax.chains.len()).collect::<Vec<_>>();
        outer_first.sort_unstable_by_key(|id| {
            let chain = &syntax.chains[*id];
            (chain.first, std::cmp::Reverse(chain.last))
        });
        let mut parent_chains = vec![None; syntax.chains.len()];
        for id in outer_first {
            let chain = &syntax.chains[id];
            parent_chains[id] = innermost_chains[chain.first];
            innermost_chains[chain.first..=chain.last].fill(Some(id));
        }
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
            broken_chains: vec![false; syntax.chains.len()],
            chain_indents: vec![0; syntax.chains.len()],
            chains: syntax.chains,
            chain_starts,
            chain_ends,
            chain_operators,
            innermost_chains,
            parent_chains,
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
            let breaks_before = self.break_before_chain_operator(index);
            self.emit_comments_before(self.tokens[index].span.start);
            if breaks_before && let Some(next) = self.tokens.get(index + 1) {
                // A comment after the operator moves before it, so the
                // operator still leads its operand's line.
                self.emit_comments_before(next.span.start);
            }

            if self.omitted[index] || self.should_skip_trailing_comma(index) {
                continue;
            }

            if self.close_to_open[index].is_some() {
                self.close_group(index);
            }

            self.decide_chains_starting_at(index);
            self.emit_token(index);

            if self.groups[index].is_some() {
                self.open_group(index);
            }

            self.request_break_after(index);
            self.end_chains_at(index);
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

    /// Decides, outermost first, whether each chain that starts at `index`
    /// breaks: when it holds a comment outside its delimited groups, or when
    /// a line written with the chain unbroken would pass the width limit
    /// (`line_end`). A chain inside a single-line group never breaks, since
    /// that group fits on its line.
    fn decide_chains_starting_at(&mut self, index: usize) {
        let Some(chains) = self.chain_starts.get(&index).cloned() else {
            return;
        };
        if self
            .active_groups
            .last()
            .is_some_and(|group| !group.multiline)
        {
            return;
        }
        self.apply_pending_break();
        let column = if self.writer.is_line_start() {
            self.writer.indent * 4
        } else {
            self.writer.column() + usize::from(self.needs_space(index))
        };
        for chain in chains {
            let last = self.chains[chain].last;
            self.broken_chains[chain] = self.chain_has_comment(chain)
                || self.line_end(index, last, column, None, Some(chain)) > FORMAT_LINE_WIDTH;
        }
    }

    /// Whether a comment stands between two tokens of the chain outside every
    /// delimited group within it.
    fn chain_has_comment(&self, chain: usize) -> bool {
        let OperatorChain { first, last, .. } = self.chains[chain];
        if !self.comment_between(self.tokens[first].span.end, self.tokens[last].span.start) {
            return false;
        }
        let mut depth = 0usize;
        for index in first..last {
            if is_open_delimiter(&self.tokens[index].kind) {
                depth += 1;
            } else if is_close_delimiter(&self.tokens[index].kind) {
                depth = depth.saturating_sub(1);
            }
            if depth == 0
                && self.comment_between(
                    self.tokens[index].span.end,
                    self.tokens[index + 1].span.start,
                )
            {
                return true;
            }
        }
        false
    }

    /// Whether a comment lies entirely within the source range
    /// `start..end`.
    fn comment_between(&self, start: usize, end: usize) -> bool {
        // Comments are in source order.
        let first = self
            .comments
            .partition_point(|comment| comment.span.start < start);
        self.comments
            .get(first)
            .is_some_and(|comment| comment.span.end <= end)
    }

    /// Before an operator of a broken chain, breaks the line, so the operator
    /// leads the continuation line. The first operator indents the chain's
    /// continuation lines, until the chain ends, one level deeper than the
    /// line the chain starts on, and one more for each broken chain that
    /// starts with it and holds it, so a chain that is the first operand of
    /// another stays apart from the other's operators. Returns whether the
    /// line breaks.
    fn break_before_chain_operator(&mut self, index: usize) -> bool {
        let Some(chain) = self.chain_operators.get(&index).copied() else {
            return false;
        };
        if !self.broken_chains[chain] {
            return false;
        }
        if self.chains[chain].operators.first() == Some(&index) {
            let first = self.chains[chain].first;
            let holders = self.chain_starts[&first]
                .iter()
                .take_while(|starting| **starting != chain)
                .filter(|starting| self.broken_chains[**starting])
                .count();
            self.chain_indents[chain] = 1 + holders;
            self.writer.indent += 1 + holders;
        }
        self.request_break(1);
        true
    }

    /// Ends the continuation indentation of each broken chain that ends at
    /// `index`. A broken chain that is the condition of an `if` or the
    /// subject of a `match` puts the `{` that follows on its own line, so the
    /// continuation lines stay apart from the block's first line.
    fn end_chains_at(&mut self, index: usize) {
        let Some(chains) = self.chain_ends.get(&index) else {
            return;
        };
        let indent = chains
            .iter()
            .filter(|chain| self.broken_chains[**chain])
            .map(|chain| self.chain_indents[*chain])
            .sum::<usize>();
        if indent == 0 {
            return;
        }
        self.writer.indent = self.writer.indent.saturating_sub(indent);
        let opens_body = self.tokens.get(index + 1).is_some_and(|token| {
            matches!(token.kind, TokenKind::LeftBrace)
                && self.groups[index + 1]
                    .is_some_and(|group| matches!(group.kind, GroupKind::Block | GroupKind::Match))
        });
        if opens_body {
            self.request_break(1);
        }
    }

    /// The widest column that the lines holding the tokens `from..=through`
    /// reach when those tokens are written as unbroken as they can be,
    /// starting at `column` after the token `previous` (none for the line's
    /// first token), or the first column found past the width limit.
    ///
    /// Within `from..=through`, a group that breaks wherever it stands, or
    /// that is too wide to stay on one line from where it opens, breaks as
    /// `open_group` decides: its line ends after its opening delimiter, and
    /// the measurement goes on from its closing delimiter, at the start of a
    /// line. When measuring the chain `chain`, a too-wide group counts as
    /// passing the limit instead when `breaks_a_chain` says it breaks that
    /// chain or one within it: one of three or more operands, so its
    /// operands stay one per line, or one whose last operand holds an
    /// argument list or parenthesized expression, so the operator leads that
    /// operand's line instead of the arguments wrapping after it. A text
    /// block, like a breaking group, goes on after its closing `"""`.
    ///
    /// After `through`, the last line goes on up to its next possible break:
    /// a `,`, `;`, or let-else `else`, which stays on the line; the opening
    /// delimiter of a group, which decides for itself; an operator of a
    /// broken chain or the closing delimiter of a multiline group, which
    /// starts the next line, counting the trailing comma that such a group
    /// gains; or a comment.
    fn line_end(
        &self,
        from: usize,
        through: usize,
        mut column: usize,
        mut previous: Option<usize>,
        chain: Option<usize>,
    ) -> usize {
        // The indentation of the line the measurement starts on, from which a
        // text block that does not start a line takes its own.
        let line_indent = if self.writer.is_line_start() {
            self.writer.indent
        } else {
            self.writer.line_indent
        };
        let mut widest = 0;
        let mut index = from;
        while column <= FORMAT_LINE_WIDTH && index < self.tokens.len() {
            let token = &self.tokens[index];
            if matches!(token.kind, TokenKind::Eof)
                || previous.is_some_and(|previous| {
                    self.comment_between(self.tokens[previous].span.end, token.span.start)
                })
            {
                break;
            }
            let after = index > through;
            if after {
                if self
                    .chain_operators
                    .get(&index)
                    .is_some_and(|chain| self.broken_chains[*chain])
                {
                    break;
                }
                let closing = self.close_to_open[index].and_then(|open| {
                    self.active_groups
                        .iter()
                        .rev()
                        .find(|group| group.open == open)
                });
                if let Some(group) = closing
                    && group.multiline
                {
                    if group.kind.allows_trailing_comma()
                        && previous.is_some_and(|previous| {
                            !matches!(self.tokens[previous].kind, TokenKind::Comma)
                        })
                    {
                        column += 1;
                    }
                    break;
                }
            }
            if self.omitted[index] || (!after && self.drops_trailing_comma(index)) {
                index += 1;
                continue;
            }
            let starts_line = previous.is_none() && self.writer.is_line_start();
            if let Some(previous) = previous
                && token_pair_needs_space(
                    &self.tokens,
                    &self.groups,
                    &self.close_to_open,
                    &self.type_tokens,
                    previous,
                    index,
                    |_| false,
                )
            {
                column += 1;
            }
            column += self.token_text[index].chars().count();
            previous = Some(index);
            if after {
                let line_may_break = self.groups[index].is_some()
                    || matches!(token.kind, TokenKind::Comma) && !self.generic_commas[index]
                    || matches!(token.kind, TokenKind::Semicolon | TokenKind::Else)
                    || self.breaks_after.contains_key(&token.span.end)
                    || matches!(&token.kind, TokenKind::Template(template) if template.text_block.is_some());
                if line_may_break {
                    break;
                }
                index += 1;
                continue;
            }
            if let TokenKind::Template(template) = &token.kind
                && template.text_block.is_some()
            {
                // `Writer::write_text_block` puts the closing `"""` here.
                if column > FORMAT_LINE_WIDTH {
                    break;
                }
                widest = widest.max(column);
                let level = if starts_line {
                    line_indent
                } else {
                    line_indent + 1
                };
                column = level * 4 + 3;
            } else if let Some(group) = self.groups[index] {
                let too_wide = group.has_content && column + group.flat_width > FORMAT_LINE_WIDTH;
                if too_wide && !group.forced_multiline && self.breaks_a_chain(index, chain) {
                    return FORMAT_LINE_WIDTH + 1;
                }
                if group.forced_multiline || too_wide {
                    if column > FORMAT_LINE_WIDTH {
                        break;
                    }
                    // The group's closing delimiter starts a line at the
                    // current indentation, since nothing before it on this
                    // line changes the indentation.
                    widest = widest.max(column);
                    column = self.writer.indent * 4 + self.token_text[group.close].chars().count();
                    previous = Some(group.close);
                    index = group.close;
                }
            }
            index += 1;
        }
        widest.max(column)
    }

    /// Whether a group opening at `index` that is too wide for its line
    /// breaks the chain `measured` or a chain within it that holds the
    /// group: one with three or more operands, or one whose last operand
    /// holds the group when the group opens with `(`, an argument list or a
    /// parenthesized expression, as in `"text " + int_to_string(` ... `)`.
    /// A chain of two keeps every other group that wraps joined to its other
    /// operand: one in its first operand (`f(` ... `) + x`), or a list,
    /// entry list, record, or named construction in its last (`x == [` ...
    /// `]`).
    fn breaks_a_chain(&self, index: usize, measured: Option<usize>) -> bool {
        let Some(measured) = measured else {
            return false;
        };
        let parenthesized = matches!(self.tokens[index].kind, TokenKind::LeftParen);
        let mut holder = self.innermost_chains[index];
        while let Some(chain) = holder {
            let operators = &self.chains[chain].operators;
            if operators.len() > 1
                || parenthesized && operators.last().is_some_and(|last| index > *last)
            {
                return true;
            }
            if chain == measured {
                break;
            }
            holder = self.parent_chains[chain];
        }
        false
    }

    fn open_group(&mut self, index: usize) {
        let info = self.groups[index].expect("opening delimiter should have group information");
        let column = self.writer.column();
        let projected_width = column.saturating_add(info.flat_width);
        // Beyond its flat width, a group breaks when the line through it and
        // what follows it on the line would pass the width limit, such as
        // `?.body;` after a call. Patterns stay compact, and a group inside
        // a single-line group fits with it.
        let multiline = info.has_content
            && (info.forced_multiline
                || projected_width > FORMAT_LINE_WIDTH
                || info.kind != GroupKind::Pattern
                    && self
                        .active_groups
                        .last()
                        .is_none_or(|group| group.multiline)
                    && self.line_end(index + 1, info.close, column, Some(index), None)
                        > FORMAT_LINE_WIDTH);
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
    chains: Vec<OperatorChain>,
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

        let chains = collector
            .chain_spans
            .iter()
            .map(|chain| OperatorChain {
                first: tokens.partition_point(|token| token.span.start < chain.start),
                last: index_ending(chain.end),
                operators: chain
                    .operators_after
                    .iter()
                    .map(|end| tokens.partition_point(|token| token.span.start < *end))
                    .collect(),
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
            chains,
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
    chain_spans: Vec<ChainSpan>,
}

/// The source extent of one binary operator chain.
struct ChainSpan {
    /// The start of the first operand.
    start: usize,
    /// The end of the last operand.
    end: usize,
    /// For each operator, in source order, the end of the operand before it.
    operators_after: Vec<usize>,
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
            ExpressionKind::Binary {
                operator, right, ..
            } => self.operator_chain(expression, *operator, right.span.end, pending),
        }
    }

    /// Records the chain whose last operator is that of `root`: `root` and
    /// the unparenthesized operations of the same precedence level along its
    /// left spine, which a left-associative parse nests there. Their right
    /// operands and the first operand are collected as nodes of their own,
    /// so the spine is walked with a loop.
    fn operator_chain<'a>(
        &mut self,
        root: &'a Expression,
        operator: BinaryOperator,
        end: usize,
        pending: &mut Vec<SyntaxNode<'a>>,
    ) {
        let level = precedence(operator);
        let mut operators_after = Vec::new();
        let mut node = root;
        while let ExpressionKind::Binary {
            left,
            operator,
            right,
        } = &node.kind
        {
            // An operation in parentheses spans them, so it starts before
            // its left operand; it is an operand, not part of this chain.
            let parenthesized = node.span.start != left.span.start;
            if precedence(*operator) != level || (parenthesized && !std::ptr::eq(node, root)) {
                break;
            }
            operators_after.push(left.span.end);
            pending.push(SyntaxNode::Expression(right));
            node = left;
        }
        pending.push(SyntaxNode::Expression(node));
        operators_after.reverse();
        self.chain_spans.push(ChainSpan {
            start: node.span.start,
            end,
            operators_after,
        });
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

/// The binding strength of a binary operator; the operators of one level
/// form one chain.
const fn precedence(operator: BinaryOperator) -> u8 {
    match operator {
        BinaryOperator::Or => 1,
        BinaryOperator::And => 2,
        BinaryOperator::Equal | BinaryOperator::NotEqual => 3,
        BinaryOperator::Less
        | BinaryOperator::LessEqual
        | BinaryOperator::Greater
        | BinaryOperator::GreaterEqual => 4,
        BinaryOperator::Add | BinaryOperator::Subtract => 5,
        BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Remainder => 6,
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

/// The canonical text of `token`, read from the source `text` it was lexed
/// from.
fn canonical_token_text(token: &Token, text: &str) -> String {
    match &token.kind {
        TokenKind::Identifier(value) => value.clone(),
        // An integer keeps its spelling, digit separators included
        // (`1_000_000`): the lexer accepted it, so it denotes the same value.
        TokenKind::Integer(_) => text[token.span.start..token.span.end].to_owned(),
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
        // Inside the broken grouping the sum is 101 columns wide even at the
        // top level, so it breaks before each `+` as well.
        let broken_sum = |indent: &str| format!("1{}", format!("\n{indent}+ 1").repeat(24));

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
            format!(
                "fn helper() -> Int {{\n    1\n}}\n\n(\n    {}\n);\n",
                broken_sum("        ")
            )
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
                "fn outer() -> Int {{\n    fn helper() -> Int {{\n        1\n    }}\n    (\n        {}\n    );\n    0\n}}\n",
                broken_sum("            ")
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

    /// Asserts that `formatted` is a fixed point and that each of its lines
    /// fits within the width limit.
    fn assert_canonical_and_fits(formatted: &str) {
        assert_eq!(format(formatted), formatted);
        for line in formatted.lines() {
            assert!(
                line.chars().count() <= FORMAT_LINE_WIDTH,
                "{line}\n{formatted}"
            );
        }
    }

    #[test]
    fn breaks_long_plus_chains_before_every_operator() {
        // A chain that fits stays on its line, in every position.
        let short =
            "fn f(a: String, b: String) -> String {\n    println(a + b);\n    a + b + a\n}\n";
        assert_eq!(format(short), short);

        let operands = [
            "\"/repos/\"",
            "owner",
            "\"/\"",
            "repository",
            "\"/issues/\"",
            "number",
        ];
        let flat = operands.join(" + ");
        let source = format!(
            "fn f(owner: String, repository: String, number: String) -> HttpRequest {{\n\
             let path = {flat} + {flat};\n\
             println({flat} + {flat});\n\
             HttpRequest {{ method: \"GET\", path: {flat} + {flat} }}\n}}\n"
        );
        let formatted = format(&source);
        let lines = |indent: &str| {
            let mut text = operands[0].to_owned();
            for operand in operands[1..].iter().chain(&operands) {
                text.push_str(&format!("\n{indent}+ {operand}"));
            }
            text
        };
        assert_eq!(
            formatted,
            format!(
                "fn f(owner: String, repository: String, number: String) -> HttpRequest {{\n    \
                 let path = {};\n    \
                 println(\n        {},\n    );\n    \
                 HttpRequest {{\n        method: \"GET\",\n        path: {},\n    }}\n}}\n",
                lines("        "),
                lines("            "),
                lines("            "),
            )
        );
        assert_canonical_and_fits(&formatted);
    }

    #[test]
    fn breaks_condition_chains_and_puts_the_block_brace_on_its_own_line() {
        let condition = (0..6)
            .map(|index| format!("string_contains(path, \"/segment-{index}\")"))
            .collect::<Vec<_>>();
        let formatted = format(&format!(
            "fn f(path: String) -> Int {{ if {} {{ 1 }} else {{ 2 }} }}\n",
            condition.join(" && ")
        ));
        assert_eq!(
            formatted,
            format!(
                "fn f(path: String) -> Int {{\n    if {}\n    {{\n        1\n    }} else {{\n        2\n    }}\n}}\n",
                condition.join("\n        && ")
            )
        );
        assert_canonical_and_fits(&formatted);

        // A chain of two operands keeps a group that wraps joined to it.
        let list = (0..12)
            .map(|index| format!("\"item-{index}\""))
            .collect::<Vec<_>>();
        let formatted = format(&format!(
            "fn f(items: List<String>) -> Bool {{ items == [{}] }}\n",
            list.join(", ")
        ));
        assert!(
            formatted.contains("    items == [\n        \"item-0\",\n"),
            "{formatted}"
        );
        assert!(formatted.contains("    ]\n}\n"), "{formatted}");
        assert_canonical_and_fits(&formatted);
    }

    #[test]
    fn a_chain_of_two_keeps_a_wrapping_group_joined_while_its_last_line_fits() {
        let body = "b".repeat(40);
        let wrapped = format!(
            "json_encode(\n        HttpResponse {{ status: 200, body: \"{body}\" }},\n    )"
        );
        let suffix = |length: usize| format!("\"{}\"", "s".repeat(length));
        // `) + "…"` is 99 columns wide: the chain stays joined.
        let source = format!(
            "fn f() -> String {{ json_encode(HttpResponse {{ status: 200, body: \"{body}\" }}) + {} }}\n",
            suffix(89)
        );
        let formatted = format(&source);
        assert_eq!(
            formatted,
            format!("fn f() -> String {{\n    {wrapped} + {}\n}}\n", suffix(89))
        );
        assert_canonical_and_fits(&formatted);
        // At 101 columns the chain breaks before its operator instead.
        let formatted = format(&source.replace(&suffix(89), &suffix(91)));
        assert_eq!(
            formatted,
            format!(
                "fn f() -> String {{\n    {wrapped}\n        + {}\n}}\n",
                suffix(91)
            )
        );
        // The literal alone is too long for its line, which cannot break.
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn a_chain_of_two_breaks_before_an_argument_list_that_wraps_in_its_last_operand() {
        // Each `"…" + call(…)` passes the limit, so the call's argument list
        // would wrap; the chain breaks before `+` instead, as a chain of three
        // does, in a let value, a call argument, and a construction field.
        let text = |length: usize| format!("\"{}\"", "t".repeat(length));
        let source = format!(
            "fn f(name: String, count: Int) -> Result<HttpResponse, String> {{\n\
             let path = {} + url_path_segment(name)?;\n\
             println({} + int_to_string(count));\n\
             let size = {} + int_to_string(string_byte_len(name));\n\
             let reply = HttpResponse {{ status: 200, body: {} + url_path_segment(name)? }};\n\
             let grouped = {} + (count * count);\n\
             Ok(reply)\n}}\n",
            text(70),
            text(80),
            text(60),
            text(70),
            text(80),
        );
        let formatted = format(&source);
        assert_eq!(
            formatted,
            format!(
                "fn f(name: String, count: Int) -> Result<HttpResponse, String> {{\n    \
                 let path = {}\n        + url_path_segment(name)?;\n    \
                 println(\n        {}\n            + int_to_string(count),\n    );\n    \
                 let size = {}\n        + int_to_string(string_byte_len(name));\n    \
                 let reply = HttpResponse {{\n        status: 200,\n        body: {}\n            \
                 + url_path_segment(name)?,\n    }};\n    \
                 let grouped = {}\n        + (count * count);\n    \
                 Ok(reply)\n}}\n",
                text(70),
                text(80),
                text(60),
                text(70),
                text(80),
            )
        );
        assert_canonical_and_fits(&formatted);
        // When the call's arguments do not fit on the continuation line
        // either, they wrap there.
        let argument = "a".repeat(80);
        let formatted = format(&format!(
            "fn f({argument}: Int) -> String {{ \"count: \" + int_to_string({argument}) }}\n"
        ));
        assert_eq!(
            formatted,
            format!(
                "fn f(\n    {argument}: Int,\n) -> String {{\n    \"count: \"\n        \
                 + int_to_string(\n            {argument},\n        )\n}}\n"
            )
        );
        assert_canonical_and_fits(&formatted);
        // A list, entry list, or construction stays joined to the other
        // operand of a chain of two.
        let items = (0..12)
            .map(|index| format!("\"item-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let formatted = format(&format!(
            "fn f(items: List<String>) -> Bool {{ items == [{items}] }}\n"
        ));
        assert!(formatted.contains("    items == [\n"), "{formatted}");
        assert_canonical_and_fits(&formatted);
    }

    #[test]
    fn a_comment_after_a_chain_operator_moves_before_it() {
        let source = "fn f(a: String, b: String) -> String {\n\
                      let s = a +\n// note\nb;\n\
                      let t = a + // after the operator\nb + a;\n\
                      let u = a // before\n+ // after\nb;\n\
                      let v = a == b + // inner\na;\n\
                      s + t + u\n}\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn f(a: String, b: String) -> String {\n    \
             let s = a\n        // note\n        + b;\n    \
             let t = a // after the operator\n        + b\n        + a;\n    \
             let u = a // before\n        // after\n        + b;\n    \
             let v = a\n        == b // inner\n            + a;\n    \
             s + t + u\n}\n"
        );
        assert_eq!(format(&formatted), formatted);
        assert_eq!(comment_texts(&formatted), comment_texts(source));
    }

    #[test]
    fn breaks_chains_of_each_precedence_level_one_level_at_a_time() {
        let product = ["weight"; 8].join(" * ");
        let formatted = format(&format!(
            "fn f(weight: Int, bonus: Int) -> Int {{ {product} + {product} - bonus }}\n"
        ));
        assert_eq!(
            formatted,
            format!(
                "fn f(weight: Int, bonus: Int) -> Int {{\n    {product}\n        + {product}\n        - bonus\n}}\n"
            )
        );
        assert_canonical_and_fits(&formatted);

        // Parentheses start a chain of their own.
        let formatted = format(&format!(
            "fn f(weight: Int, bonus: Int) -> Int {{ ({product} + {product}) * bonus }}\n"
        ));
        assert_eq!(
            formatted,
            format!(
                "fn f(weight: Int, bonus: Int) -> Int {{\n    (\n        {product}\n            + {product}\n    ) * bonus\n}}\n"
            )
        );
        assert_canonical_and_fits(&formatted);
    }

    #[test]
    fn a_comment_inside_a_chain_breaks_it_and_keeps_its_place() {
        let source = "fn f(owner: String, title: String) -> String {\n\
                      let heading = \"Issue by \" + owner // who\n+ \" titled \" + title;\n\
                      let detail = \"Read\"\n// then decided\n+ owner;\n\
                      heading + detail + record { value: owner // inside a group\n}.value\n}\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "fn f(owner: String, title: String) -> String {\n    \
             let heading = \"Issue by \"\n        + owner // who\n        + \" titled \"\n        + title;\n    \
             let detail = \"Read\"\n        // then decided\n        + owner;\n    \
             heading + detail + record {\n        value: owner, // inside a group\n    }.value\n}\n"
        );
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn keeps_integer_digit_separators_as_written() {
        let source =
            "let a=1_000_000;\nlet b = [1_0, 2_00, -4_096, 0_7];\nprintln(a * 1_000 + 86_400);\n";
        let formatted = format(source);
        assert_eq!(
            formatted,
            "let a = 1_000_000;\nlet b = [1_0, 2_00, -4_096, 0_7];\n\nprintln(a * 1_000 + 86_400);\n"
        );
        assert_eq!(format(&formatted), formatted);
        assert_eq!(output(&formatted), output(source));
        assert_eq!(output(source), "1000086400\n");
    }

    #[test]
    fn breaks_a_group_when_what_follows_it_passes_the_limit() {
        // Each group fits by its own width, but the line through what follows
        // it would be 101 columns wide: a return type, `) else`, and `.body;`.
        let function = "f".repeat(34);
        let signature = format!(
            "fn {function}(owner: String, number: Int) -> Result<Option<String>, String> {{"
        );
        let value = "v".repeat(50);
        let let_else = format!("    let Ok({value}) = config_string(\"reference.repo\") else");
        let name = "n".repeat(46);
        let construction =
            format!("let {name} = HttpResponse {{ status: 200, body: \"text\" }}.body;");
        for line in [&signature, &let_else, &construction] {
            assert_eq!(line.len(), FORMAT_LINE_WIDTH + 1, "{line}");
        }
        let formatted = format(&format!(
            "{signature} Ok(None) }}\n\
             fn g(r: Result<String, String>) -> String {{\n{let_else} Err(e) => e;\n    {value}\n}}\n\
             {construction}\n"
        ));
        assert_eq!(
            formatted,
            format!(
                "fn {function}(\n    owner: String,\n    number: Int,\n) -> Result<Option<String>, String> {{\n    \
                 Ok(None)\n}}\n\n\
                 fn g(r: Result<String, String>) -> String {{\n    let Ok({value}) = config_string(\n        \
                 \"reference.repo\",\n    ) else Err(e) => e;\n    {value}\n}}\n\n\
                 let {name} = HttpResponse {{\n    status: 200,\n    body: \"text\",\n}}.body;\n"
            )
        );
        assert_canonical_and_fits(&formatted);
        // One column less, each group stays on its line; the let-else arm
        // moves to the next one.
        let fits = format!(
            "{}\n    Ok(None)\n}}\n\nfn g(r: Result<String, String>) -> String {{\n{}\n        Err(e) => e;\n    {}\n}}\n\n{}\n",
            signature.replacen(&function, &function[1..], 1),
            let_else.replacen(&value, &value[1..], 1),
            &value[1..],
            construction.replacen(&name, &name[1..], 1),
        );
        assert_eq!(format(&fits), fits);
    }

    #[test]
    fn leaves_lines_that_cannot_break_as_they_are() {
        let long = "x".repeat(100);
        let source = format!(
            "let notice = \"{long}\";\nlet user = record {{ name: \"Ada\" }};\n\
             let greeting = \"\\(user.name) {long}\";\n\n\
             // {long}\n\
             fn f(r: Result<String, String>) -> String {{\n    match r {{\n        Ok(text) => \"{long}\",\n        \
             Err(reason) => reason,\n    }}\n}}\n"
        );
        assert_eq!(format(&source), source);
        // A call around one holds it on a line of its own.
        let formatted = format(&format!("println(\"{long}\");\n"));
        assert_eq!(formatted, format!("println(\n    \"{long}\",\n);\n"));
        assert_eq!(format(&formatted), formatted);
    }

    #[test]
    fn formatted_lines_fit_wherever_a_break_exists() {
        // Chains of short operands in every position a value can take, at a
        // range of widths around the limit: every line fits once formatted.
        let contexts: [fn(&str) -> String; 8] = [
            |chain| format!("let value = {chain};\n"),
            |chain| format!("println({chain});\n"),
            |chain| format!("let r = HttpResponse {{ status: 200, body: {chain} }};\n"),
            |chain| format!("let l = [\"head\", {chain}];\n"),
            |chain| format!("let e = [\"key\": {chain}];\n"),
            |chain| {
                format!(
                    "fn f(r: Result<String, String>) -> String {{ match r {{ Ok(v) => {chain}, Err(e) => e }} }}\n"
                )
            },
            |chain| {
                format!(
                    "fn f(r: Result<String, String>) -> String {{ let Ok(v) = r else Err(e) => {chain}; v }}\n"
                )
            },
            |chain| {
                format!(
                    "fn f(v: String) -> String {{ if string_is_empty({chain}) {{ v }} else {{ {chain} }} }}\n"
                )
            },
        ];
        for count in 1..14 {
            let chain = (0..count)
                .map(|index| format!("\"part-{index}\" + int_to_string({index})"))
                .collect::<Vec<_>>()
                .join(" + ");
            for context in contexts {
                let source = format!("let v = \"v\";\n{}", context(&chain));
                let formatted = format(&source);
                assert_canonical_and_fits(&formatted);
                assert_eq!(output(&formatted), output(&source));
            }
        }
        // A two-operand string build whose last operand is a call, at widths
        // around the limit: the chain breaks before `+`, and the call's
        // argument list never wraps, since its continuation line fits.
        for length in 40..=75 {
            let chain = format!(
                "\"{}\" + int_to_string(string_byte_len(v))",
                "t".repeat(length)
            );
            for context in contexts {
                let formatted = format(&format!("let v = \"v\";\n{}", context(&chain)));
                assert_canonical_and_fits(&formatted);
                assert!(!formatted.contains("int_to_string(\n"), "{formatted}");
            }
        }
    }

    /// The text of each comment, in order, with an attached doc comment's
    /// one optional space after `///` left out.
    fn comment_texts(text: &str) -> Vec<String> {
        lex_with_comments(&Source::new("comments.krit", text))
            .expect("source should lex")
            .comments
            .into_iter()
            .map(|comment| match parser::doc_line_text(&comment.text) {
                Some(line) => format!("///{line}"),
                None => comment.text,
            })
            .collect()
    }

    #[test]
    fn every_repository_source_formats_idempotently_and_keeps_its_comments() {
        crate::run_on_thread(
            "formatter-repository",
            crate::COMPILER_STACK_BYTES,
            format_every_repository_source_twice,
        )
        .expect("formatter test thread should start");
    }

    fn format_every_repository_source_twice() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut directories = vec![root.clone()];
        let mut formatted_count = 0;
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(&directory).expect("directory should be readable") {
                let path = entry.expect("entry should be readable").path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if path.is_dir() {
                    if name != "target" && name != ".git" {
                        directories.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "krit") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("source should be readable");
                let label = path.display().to_string();
                // Sources that do not parse, such as rejected conformance
                // cases, have nothing to format.
                let Ok(once) = format_source(&Source::new(label.clone(), text.as_str())) else {
                    continue;
                };
                let twice = format_source(&Source::new(label.clone(), once.as_str()))
                    .unwrap_or_else(|error| {
                        panic!("{label}: formatted source should format: {error:?}")
                    });
                assert_eq!(twice, once, "{label}: formatting twice changed it");
                assert_eq!(
                    comment_texts(&once),
                    comment_texts(&text),
                    "{label}: comments changed"
                );
                formatted_count += 1;
            }
        }
        assert!(
            formatted_count > 200,
            "only {formatted_count} sources formatted"
        );
    }
}
