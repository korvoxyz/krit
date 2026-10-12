//! Fixed depth limits that keep every compiler stage and the direct evaluator
//! within a bounded native stack.
//!
//! The parser enforces the two source limits, so every later stage only ever
//! sees programs whose recursion is bounded by them. Each later stage still
//! counts its own recursion and fails closed with `K1005` when handed a
//! program (for example a hand-built AST) that bypassed the parser.

use std::{io, panic, thread};

use crate::{Diagnostic, Span};

/// Maximum syntactic nesting depth of a source file (`K1005`).
///
/// Parenthesized expressions, blocks, list and record literals, call argument
/// lists, field-access, call, and `?` links, unary operands, `if` conditions,
/// `match` subjects and arms, let-else arms, the payloads of variant patterns,
/// and type argument lists each open one level, and a `match` arm's value
/// opens one more for each variant or list pattern nested in a payload of its
/// pattern, since Core IR decides each with one more nested match.
/// An `else if` continuation and a chained binary operator do not.
///
/// Analysis applies the same bound to the resolved type of every type
/// variable, since inferred types can otherwise grow without limit: one level
/// per binding (`let next = [previous];`), or many levels at once when a type
/// variable inside an earlier binding's type is resolved later.
pub const MAX_NESTING_DEPTH: usize = 256;

/// Maximum structural depth of a source file (`K1005`).
///
/// The structural depth of a point is its syntactic nesting depth plus the
/// number of `else if` continuations, binary operators, and call,
/// field-access, and `?` links above it in the syntax tree. Operator and
/// postfix chains nest to the left, so in `(a + b) + c` the operand `a` sits
/// below both operators; parenthesizing a chain therefore cannot hide its
/// length.
/// The parser checks this depth for every point of the tree it builds, so it
/// bounds the height of every accepted syntax tree, and with it the recursion
/// of every later stage, including Core IR and the WebAssembly compilers,
/// which represent each `else if` continuation as one more nested block.
pub const MAX_STRUCTURAL_DEPTH: usize = 4096;

/// Maximum recursion of analysis and Core lowering over a syntax tree:
/// 4,610 levels.
///
/// Both count one level per expression and one per block, which differs from
/// the structural depth in four ways:
///
/// - a top-level statement's expression has structural depth 0 but is the
///   first level of analysis (+1);
/// - lowering places the top-level statements inside the module
///   initialization block, one more level (+1, lowering only);
/// - a block reached from an expression (a block expression, a function
///   literal's body, or an `if` consequent) counts two levels for the one
///   syntactic level it opens (+1 per such block);
/// - an `else` block is a block expression below its `if`, so it counts three
///   levels for the one syntactic level it opens (+2 per `else` block).
///
/// Every other construct counts at most as much as the structural depth, and
/// parentheses count nothing. Each syntactic level therefore adds at most two
/// levels, so analysis reaches at most `MAX_STRUCTURAL_DEPTH +
/// 2 * MAX_NESTING_DEPTH + 1` levels and lowering one more. Every tree the
/// parser accepts stays within this bound, and only a hand-built tree that
/// bypassed the parser can exceed it.
pub(crate) const MAX_TREE_DEPTH: usize = MAX_STRUCTURAL_DEPTH + 2 * MAX_NESTING_DEPTH + 2;

/// Maximum nesting of Core IR blocks inside one function body.
///
/// Lowering adds at most one wrapper block per syntactic level (an `else`
/// block inside its alternative block), plus one for module initialization,
/// on top of the structural depth, so lowered programs stay well below this.
/// Core verification rejects deeper modules, which bounds the recursion of
/// every later Core walk, including both WebAssembly compilers.
pub const MAX_CORE_BLOCK_DEPTH: usize = 2 * MAX_STRUCTURAL_DEPTH;

/// Maximum number of active function calls in the direct evaluator (`K4010`).
pub const MAX_CALL_DEPTH: usize = 10_000;

/// Maximum nested expression and block evaluations in the direct evaluator
/// (`K4010`).
///
/// A typical recursive function needs three to six nested evaluations per
/// call, so ordinary recursion reaches [`MAX_CALL_DEPTH`] first. Function
/// bodies nested unusually deeply reach this bound first instead.
pub const MAX_EVALUATION_DEPTH: usize = 100_000;

/// Maximum nesting of list, record, and variant values in the direct
/// evaluator (`K4010`).
///
/// Printing, comparing, JSON-encoding, and releasing a value recurse once per
/// level of its nesting, so constructing a deeper value fails instead. A
/// value nests no deeper than its static type, which analysis bounds far
/// below this. `krit run` analyzes first, so only a program evaluated with
/// [`crate::execute`] and no analysis can reach it, for example a function
/// that wraps its argument in one more list on every call.
pub const MAX_VALUE_DEPTH: usize = 10_000;

/// Native stack reserved for the thread that runs the direct evaluator: 1 GiB.
///
/// Measured on aarch64 macOS across the recursive shapes the evaluator has
/// (calls, short-circuit and strict operators, blocks, lists, records,
/// matches, unary operators, `if` conditions, field access, closures, and
/// built-in calls), the most stack used per counted evaluation is about
/// 1.2 KiB in optimized builds and 5 KiB in unoptimized builds, so reaching
/// [`MAX_EVALUATION_DEPTH`] needs at most about 120 MiB and 500 MiB
/// respectively; 10,000 calls of a simple recursive function need about
/// 40 MiB and 150 MiB, and printing, comparing, and encoding a value at
/// [`MAX_VALUE_DEPTH`] needs under 15 MiB and 50 MiB. The reservation is
/// virtual address space: pages are committed only as a program actually
/// recurses.
pub const EVALUATOR_STACK_BYTES: usize = 1024 * 1024 * 1024;

/// Native stack reserved for the thread that runs the compiler pipeline
/// (lexing, parsing, analysis, Core lowering and verification, formatting,
/// and WebAssembly compilation): 256 MiB.
///
/// Measured on aarch64 macOS, the deepest syntax trees the parser accepts
/// ([`MAX_NESTING_DEPTH`] nested `else` blocks or block expressions, each
/// called until the innermost point reaches [`MAX_STRUCTURAL_DEPTH`], which
/// analysis and lowering recurse through about 4,600 levels deep) need
/// about 11 MiB to check, explain, or build in optimized builds and 81 MiB in
/// unoptimized builds; an `else if` chain at [`MAX_STRUCTURAL_DEPTH`] needs
/// about 4 MiB and 57 MiB, dominated by Core verification; and
/// [`MAX_NESTING_DEPTH`] levels of any one construct need under 2 MiB and
/// about 9 MiB respectively.
pub const COMPILER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Runs `work` to completion on a new thread named `name` with `stack_bytes`
/// of native stack, re-raising any panic on the calling thread.
///
/// Use [`COMPILER_STACK_BYTES`] for the compiler pipeline and
/// [`EVALUATOR_STACK_BYTES`] for the direct evaluator. Only creating the
/// thread can fail, for example when the address space cannot be reserved.
pub fn run_on_thread<T: Send>(
    name: &str,
    stack_bytes: usize,
    work: impl FnOnce() -> T + Send,
) -> io::Result<T> {
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .name(name.to_owned())
            .stack_size(stack_bytes)
            .spawn_scoped(scope, work)?;
        Ok(worker
            .join()
            .unwrap_or_else(|payload| panic::resume_unwind(payload)))
    })
}

pub(crate) fn nesting_error(span: Span) -> Diagnostic {
    Diagnostic::new(
        "K1005",
        format!(
            "nesting depth limit exceeded: constructs may nest at most {MAX_NESTING_DEPTH} levels"
        ),
        span,
    )
}

pub(crate) fn structural_error(span: Span) -> Diagnostic {
    Diagnostic::new(
        "K1005",
        format!(
            "nesting depth limit exceeded: nesting plus the `else if` continuations, operators, \
             and call, field-access, or `?` links above any point may reach at most \
             {MAX_STRUCTURAL_DEPTH} levels"
        ),
        span,
    )
}

/// Reports a syntax tree that nests deeper than any parsed program can; only
/// hand-built input reaches this.
pub(crate) fn tree_depth_error(span: Span) -> Diagnostic {
    Diagnostic::new(
        "K1005",
        format!(
            "nesting depth limit exceeded: the syntax tree nests deeper than {MAX_TREE_DEPTH} levels"
        ),
        span,
    )
}

/// Generators for the deepest programs the parser accepts, shared by the tests
/// of every stage that recurses over syntax trees.
#[cfg(test)]
pub(crate) mod deepest {
    use super::{MAX_NESTING_DEPTH, MAX_STRUCTURAL_DEPTH};

    /// How each nested level of a [`source`] program holds the next.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Level {
        /// `if false { g } else { let h = <next>; g }`: an `else` block, the
        /// construct that adds the most levels of analysis and lowering
        /// recursion per syntactic level.
        Else,
        /// `{ let h = <next>; g }`: a block expression.
        Block,
    }

    /// Returns a well-typed program that prints `0`, whose innermost point
    /// has nesting level [`MAX_NESTING_DEPTH`] and structural depth
    /// [`MAX_STRUCTURAL_DEPTH`] plus `extra_links`.
    ///
    /// It nests [`MAX_NESTING_DEPTH`] levels of `level`, and each level is
    /// called as many times as the nesting limit allows (level `j`, counted
    /// from 1, may open `257 - j` levels of call links) until the structural
    /// depth is reached. Every call applies `g`, a curried function of 200
    /// parameters, so every level is a function and the outermost one, called
    /// 200 times, is `0`.
    pub(crate) fn source(level: Level, extra_links: usize) -> String {
        const ARITY: usize = 200;
        let mut text = String::from("fn g(x0) { ");
        for parameter in 1..ARITY {
            text.push_str(&format!("fn(x{parameter}) {{ "));
        }
        text.push('0');
        text.push_str(&" }".repeat(ARITY));
        text.push('\n');

        let mut remaining = MAX_STRUCTURAL_DEPTH - MAX_NESTING_DEPTH + extra_links;
        let links = (1..=MAX_NESTING_DEPTH)
            .map(|nesting| {
                let count = ARITY.min(MAX_NESTING_DEPTH + 1 - nesting).min(remaining);
                remaining -= count;
                count
            })
            .collect::<Vec<_>>();
        assert_eq!(remaining, 0, "the links should fit under the nesting limit");

        let mut expression = String::from("0");
        for count in links.into_iter().rev() {
            let level = match level {
                Level::Else => format!("if false {{ g }} else {{ let h = {expression}; g }}"),
                Level::Block => format!("{{ let h = {expression}; g }}"),
            };
            expression = format!("{level}{}", "(1)".repeat(count));
        }
        text.push_str(&format!("let value = {expression};\nprintln(value);\n"));
        text
    }
}
