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
/// Analysis applies the same bound to inferred types, which can otherwise
/// grow one level per binding (`let next = [previous];`).
pub const MAX_NESTING_DEPTH: usize = 256;

/// Maximum structural depth of a source file (`K1005`).
///
/// Structural depth is the syntactic nesting depth plus the number of
/// enclosing `else if` continuations and chained binary operators. It bounds
/// the recursion of every later stage, including Core IR and the WebAssembly
/// compilers, which represent each `else if` continuation as one more nested
/// block.
pub const MAX_STRUCTURAL_DEPTH: usize = 4096;

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

/// Native stack reserved for the thread that runs the direct evaluator: 1 GiB.
///
/// Measured on aarch64 macOS across the recursive shapes the evaluator has
/// (calls, operators, blocks, lists, records, matches, unary operators, field
/// access, and built-in calls), the most stack used per counted evaluation is
/// about 0.85 KiB in optimized builds and 4.4 KiB in unoptimized builds, so
/// reaching [`MAX_EVALUATION_DEPTH`] needs about 85 MiB and 430 MiB
/// respectively; 10,000 calls of a simple recursive function need about
/// 31 MiB and 150 MiB. The reservation is virtual address space: pages are
/// committed only as a program actually recurses.
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
            "nesting depth limit exceeded: nesting plus `else if` continuations and chained \
             operators may reach at most {MAX_STRUCTURAL_DEPTH} levels"
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
            "nesting depth limit exceeded: the syntax tree nests deeper than {MAX_STRUCTURAL_DEPTH} levels"
        ),
        span,
    )
}
