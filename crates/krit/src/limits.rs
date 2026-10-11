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
/// lists, field-access and call links, unary operands, `if` conditions,
/// `match` subjects and arms, and type argument lists each open one level.
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
/// number of `else if` continuations, binary operators, and call and
/// field-access links above it in the syntax tree. Operator and postfix
/// chains nest to the left, so in `(a + b) + c` the operand `a` sits below
/// both operators; parenthesizing a chain therefore cannot hide its length.
/// The parser checks this depth for every point of the tree it builds, so it
/// bounds the height of every accepted syntax tree, and with it the recursion
/// of every later stage, including Core IR and the WebAssembly compilers,
/// which represent each `else if` continuation as one more nested block.
pub const MAX_STRUCTURAL_DEPTH: usize = 4096;

/// Maximum recursion of analysis and Core lowering over a syntax tree.
///
/// Both count one level per expression and one per block, so a block used as
/// an expression, or a function literal's body, counts twice where the
/// structural depth counts it once. That adds at most one level per syntactic
/// level, so every tree the parser accepts stays within this bound and only a
/// hand-built tree that bypassed the parser can exceed it.
pub(crate) const MAX_TREE_DEPTH: usize = MAX_STRUCTURAL_DEPTH + MAX_NESTING_DEPTH;

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
/// Measured on aarch64 macOS, the deepest program the parser accepts (an
/// `else if` chain at [`MAX_STRUCTURAL_DEPTH`]) needs about 4 MiB to check or
/// build in optimized builds and 57 MiB in unoptimized builds, dominated by
/// Core verification; [`MAX_NESTING_DEPTH`] levels of any construct need under
/// 2 MiB and about 8 MiB respectively.
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
             and call or field-access links above any point may reach at most \
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
