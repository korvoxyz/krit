//! Guest string runtime for the bounded webhook ABI (WebAssembly policy 2).
//!
//! A Krit `String` value is an `i32` guest offset of an 8-byte canonical
//! `(ptr, len)` pair that describes immutable UTF-8 bytes. The helpers here
//! lower String concatenation, equality, and ordering as plain-loop core
//! functions that use only the MVP instructions the core validator admits.
//! Each helper is emitted at most once per module, and only when a live
//! function uses it, so a module without String operators is unchanged.
//!
//! Ordering is byte-lexicographic over UTF-8, exactly as the direct
//! evaluator orders `str` bytes: the first differing byte decides, and a
//! proper prefix orders before every longer string it begins. Equality is
//! the same comparison returning zero.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use krit::{BinaryOperator, CoreBlock, CoreModule, OperationKind, Type, ValueId};
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::{
    support::{LiveFunctions, for_each_nested_block, function_value_types},
    wit::{Scalar, Signature},
};

/// One generated string runtime function.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum StringHelper {
    /// Three-way byte-lexicographic comparison of two pairs: `-1`, `0`, or `1`.
    Compare,
    /// Concatenation of two pairs into a fresh pair in bump-allocated memory.
    Concat,
}

impl StringHelper {
    /// The helper that lowers `operator` over two String operands.
    pub(crate) const fn for_operator(operator: BinaryOperator) -> Option<Self> {
        match operator {
            BinaryOperator::Add => Some(Self::Concat),
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => Some(Self::Compare),
            BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder
            | BinaryOperator::And
            | BinaryOperator::Or => None,
        }
    }

    /// Both helpers take two String pairs and return one `i32`.
    pub(crate) fn signature() -> Signature {
        Signature {
            params: vec![Scalar::I32, Scalar::I32],
            results: vec![Scalar::I32],
        }
    }

    /// Encodes the helper body. `realloc_index` is the module's bounded bump
    /// allocator, whose failure paths trap before any byte is written.
    pub(crate) fn encode(self, realloc_index: u32) -> Function {
        match self {
            Self::Compare => encode_compare(),
            Self::Concat => encode_concat(realloc_index),
        }
    }
}

/// Collects the string helpers that live functions use, in index order.
pub(crate) fn used_helpers(module: &CoreModule, live: &LiveFunctions) -> BTreeSet<StringHelper> {
    let mut helpers = BTreeSet::new();
    for function in live.functions(module) {
        let types = function_value_types(function);
        collect_block_helpers(&function.body, &types, &mut helpers);
    }
    helpers
}

fn collect_block_helpers(
    block: &CoreBlock,
    types: &BTreeMap<ValueId, Arc<Type>>,
    helpers: &mut BTreeSet<StringHelper>,
) {
    for operation in &block.operations {
        if let OperationKind::Binary { left, operator, .. } = &operation.kind
            && types.get(left).map(AsRef::as_ref) == Some(&Type::String)
            && let Some(helper) = StringHelper::for_operator(*operator)
        {
            helpers.insert(helper);
        }
        for_each_nested_block(operation, |nested| {
            collect_block_helpers(nested, types, helpers);
        });
    }
}

/// Converts the `-1`/`0`/`1` result of [`StringHelper::Compare`] on the
/// stack into the Bool that `operator` produces.
pub(crate) fn emit_comparison_result(function: &mut Function, operator: BinaryOperator) {
    let instruction = match operator {
        BinaryOperator::Equal => {
            function.instruction(&Instruction::I32Eqz);
            return;
        }
        BinaryOperator::NotEqual => Instruction::I32Ne,
        BinaryOperator::Less => Instruction::I32LtS,
        BinaryOperator::LessEqual => Instruction::I32LeS,
        BinaryOperator::Greater => Instruction::I32GtS,
        BinaryOperator::GreaterEqual => Instruction::I32GeS,
        _ => unreachable!("only comparison operators use the compare helper"),
    };
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&instruction);
}

const fn pair_word(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

/// Byte-aligned access: string bytes have no alignment guarantee.
const BYTES: MemArg = MemArg {
    offset: 0,
    align: 0,
    memory_index: 0,
};

/// `compare(left, right) -> i32`.
///
/// Skips equal 8-byte words of the shared prefix, then finds the first
/// differing byte (unsigned) within the remaining bytes; if the shared
/// prefix is equal, the shorter string orders first.
fn encode_compare() -> Function {
    const LEFT: u32 = 0;
    const RIGHT: u32 = 1;
    const LEFT_PTR: u32 = 2;
    const RIGHT_PTR: u32 = 3;
    const LEFT_LEN: u32 = 4;
    const RIGHT_LEN: u32 = 5;
    const SHARED: u32 = 6;
    const INDEX: u32 = 7;
    const LEFT_BYTE: u32 = 8;
    const RIGHT_BYTE: u32 = 9;

    let mut function = Function::new([(8, ValType::I32)]);
    let f = &mut function;
    for (pair, word, local) in [
        (LEFT, 0, LEFT_PTR),
        (RIGHT, 0, RIGHT_PTR),
        (LEFT, 4, LEFT_LEN),
        (RIGHT, 4, RIGHT_LEN),
    ] {
        f.instruction(&Instruction::LocalGet(pair));
        f.instruction(&Instruction::I32Load(pair_word(word)));
        f.instruction(&Instruction::LocalSet(local));
    }
    // shared = min(left_len, right_len)
    f.instruction(&Instruction::LocalGet(LEFT_LEN));
    f.instruction(&Instruction::LocalGet(RIGHT_LEN));
    f.instruction(&Instruction::LocalGet(LEFT_LEN));
    f.instruction(&Instruction::LocalGet(RIGHT_LEN));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::Select);
    f.instruction(&Instruction::LocalSet(SHARED));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(INDEX));

    // Equal words: `index <= shared` holds throughout, so `shared - index`
    // never wraps.
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(SHARED));
    f.instruction(&Instruction::LocalGet(INDEX));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(8));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::BrIf(1));
    for base in [LEFT_PTR, RIGHT_PTR] {
        f.instruction(&Instruction::LocalGet(base));
        f.instruction(&Instruction::LocalGet(INDEX));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(BYTES));
    }
    f.instruction(&Instruction::I64Ne);
    f.instruction(&Instruction::BrIf(1));
    increment(f, INDEX, 8);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    // First differing byte of the rest of the shared prefix.
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(INDEX));
    f.instruction(&Instruction::LocalGet(SHARED));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    for (base, byte) in [(LEFT_PTR, LEFT_BYTE), (RIGHT_PTR, RIGHT_BYTE)] {
        f.instruction(&Instruction::LocalGet(base));
        f.instruction(&Instruction::LocalGet(INDEX));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(BYTES));
        f.instruction(&Instruction::LocalSet(byte));
    }
    f.instruction(&Instruction::LocalGet(LEFT_BYTE));
    f.instruction(&Instruction::LocalGet(RIGHT_BYTE));
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::If(BlockType::Empty));
    sign_of_unsigned_difference(f, LEFT_BYTE, RIGHT_BYTE);
    f.instruction(&Instruction::Return);
    f.instruction(&Instruction::End);
    increment(f, INDEX, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    // Equal shared prefix: the shorter string orders first.
    sign_of_unsigned_difference(f, LEFT_LEN, RIGHT_LEN);
    f.instruction(&Instruction::End);
    function
}

/// `concat(left, right) -> pair`.
///
/// An empty operand returns the other operand's pair unchanged, which is
/// sound because pairs and their bytes are never mutated. Otherwise the
/// joined bytes and their new pair are allocated through the bounded
/// realloc export: a length overflow or exhausted guest memory traps before
/// anything is written, and every write targets freshly allocated memory.
fn encode_concat(realloc_index: u32) -> Function {
    const LEFT: u32 = 0;
    const RIGHT: u32 = 1;
    const LEFT_LEN: u32 = 2;
    const RIGHT_LEN: u32 = 3;
    const TOTAL: u32 = 4;
    const DATA: u32 = 5;
    const PAIR: u32 = 6;
    const INDEX: u32 = 7;
    const SOURCE: u32 = 8;
    const DESTINATION: u32 = 9;

    let mut function = Function::new([(8, ValType::I32)]);
    let f = &mut function;
    for (length, pair, other) in [(RIGHT_LEN, RIGHT, LEFT), (LEFT_LEN, LEFT, RIGHT)] {
        f.instruction(&Instruction::LocalGet(pair));
        f.instruction(&Instruction::I32Load(pair_word(4)));
        f.instruction(&Instruction::LocalTee(length));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(other));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
    }
    // total = left_len + right_len, trapping on 32-bit overflow.
    f.instruction(&Instruction::LocalGet(LEFT_LEN));
    f.instruction(&Instruction::LocalGet(RIGHT_LEN));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalTee(TOTAL));
    f.instruction(&Instruction::LocalGet(LEFT_LEN));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::Unreachable);
    f.instruction(&Instruction::End);
    allocate(f, realloc_index, 1, Allocation::Local(TOTAL));
    f.instruction(&Instruction::LocalSet(DATA));

    f.instruction(&Instruction::LocalGet(LEFT));
    f.instruction(&Instruction::I32Load(pair_word(0)));
    f.instruction(&Instruction::LocalSet(SOURCE));
    f.instruction(&Instruction::LocalGet(DATA));
    f.instruction(&Instruction::LocalSet(DESTINATION));
    copy_bytes(f, DESTINATION, SOURCE, LEFT_LEN, INDEX);

    f.instruction(&Instruction::LocalGet(RIGHT));
    f.instruction(&Instruction::I32Load(pair_word(0)));
    f.instruction(&Instruction::LocalSet(SOURCE));
    f.instruction(&Instruction::LocalGet(DATA));
    f.instruction(&Instruction::LocalGet(LEFT_LEN));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(DESTINATION));
    copy_bytes(f, DESTINATION, SOURCE, RIGHT_LEN, INDEX);

    allocate(f, realloc_index, 4, Allocation::Constant(8));
    f.instruction(&Instruction::LocalTee(PAIR));
    f.instruction(&Instruction::LocalGet(DATA));
    f.instruction(&Instruction::I32Store(pair_word(0)));
    f.instruction(&Instruction::LocalGet(PAIR));
    f.instruction(&Instruction::LocalGet(TOTAL));
    f.instruction(&Instruction::I32Store(pair_word(4)));
    f.instruction(&Instruction::LocalGet(PAIR));
    f.instruction(&Instruction::End);
    function
}

enum Allocation {
    Constant(i32),
    Local(u32),
}

/// Pushes `realloc(0, 0, align, size)`: a fresh allocation, or a trap.
fn allocate(function: &mut Function, realloc_index: u32, align: i32, size: Allocation) {
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(align));
    match size {
        Allocation::Constant(size) => function.instruction(&Instruction::I32Const(size)),
        Allocation::Local(local) => function.instruction(&Instruction::LocalGet(local)),
    };
    function.instruction(&Instruction::Call(realloc_index));
}

/// Copies `length` bytes from `source` to `destination`, a word at a time
/// and then byte by byte. The ranges never overlap: the destination is
/// always freshly allocated.
fn copy_bytes(function: &mut Function, destination: u32, source: u32, length: u32, index: u32) {
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(length));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Const(8));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::BrIf(1));
    offset_address(function, destination, index);
    offset_address(function, source, index);
    function.instruction(&Instruction::I64Load(BYTES));
    function.instruction(&Instruction::I64Store(BYTES));
    increment(function, index, 8);
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::LocalGet(length));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    offset_address(function, destination, index);
    offset_address(function, source, index);
    function.instruction(&Instruction::I32Load8U(BYTES));
    function.instruction(&Instruction::I32Store8(BYTES));
    increment(function, index, 1);
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
}

fn offset_address(function: &mut Function, base: u32, index: u32) {
    function.instruction(&Instruction::LocalGet(base));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Add);
}

fn increment(function: &mut Function, local: u32, amount: i32) {
    function.instruction(&Instruction::LocalGet(local));
    function.instruction(&Instruction::I32Const(amount));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(local));
}

/// Pushes `(left > right) - (left < right)` for unsigned `i32` locals.
fn sign_of_unsigned_difference(function: &mut Function, left: u32, right: u32) {
    function.instruction(&Instruction::LocalGet(left));
    function.instruction(&Instruction::LocalGet(right));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::LocalGet(left));
    function.instruction(&Instruction::LocalGet(right));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::I32Sub);
}
