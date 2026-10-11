//! Guest string runtime for the bounded webhook ABI (WebAssembly policy 2).
//!
//! A Krit `String` value is an `i32` guest offset of an 8-byte canonical
//! `(ptr, len)` pair that describes immutable UTF-8 bytes. The helpers here
//! lower String concatenation, equality, ordering, and `json_decode` into
//! `String` as plain-loop core functions that use only the MVP instructions
//! the core validator admits. Each helper is emitted at most once per
//! module, and only when a live function uses it, so a module without String
//! operators or JSON decoding is unchanged.
//!
//! Ordering is byte-lexicographic over UTF-8, exactly as the direct
//! evaluator orders `str` bytes: the first differing byte decides, and a
//! proper prefix orders before every longer string it begins. Equality is
//! the same comparison returning zero.
//!
//! JSON decoding accepts exactly what the direct evaluator accepts when the
//! decoded value is a string: one RFC 8259 JSON string, optionally
//! surrounded by JSON whitespace, with every escape decoded and surrogate
//! pairs combined into one scalar value. Anything else traps inside the
//! decoder, whose name the host recognizes to report `K4009`.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use krit::{BinaryOperator, Builtin, CoreBlock, CoreModule, OperationKind, Type, ValueId};
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::{
    support::{LiveFunctions, for_each_nested_block, function_value_types},
    wit::{Scalar, Signature},
};

/// Name-section name of the generated JSON string decoder.
///
/// The decoder traps only when its input is not one JSON string, so the host
/// reports an `unreachable` trap whose innermost frame has this name as
/// invalid JSON (`K4009`), the code the direct evaluator uses. No Krit
/// identifier can contain `:`, so no source function can take the name.
pub const JSON_DECODE_FUNCTION_NAME: &str = "krit:json-decode-string";

/// One generated string runtime function.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum StringHelper {
    /// Three-way byte-lexicographic comparison of two pairs: `-1`, `0`, or `1`.
    Compare,
    /// Concatenation of two pairs into a fresh pair in bump-allocated memory.
    Concat,
    /// Decoding of one JSON string text into a pair of its UTF-8 contents.
    JsonDecode,
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

    /// The operators take two String pairs and the decoder takes one; every
    /// helper returns one `i32`.
    pub(crate) fn signature(self) -> Signature {
        let params = match self {
            Self::Compare | Self::Concat => vec![Scalar::I32, Scalar::I32],
            Self::JsonDecode => vec![Scalar::I32],
        };
        Signature {
            params,
            results: vec![Scalar::I32],
        }
    }

    /// The helper's name-section name, when the host attributes its traps.
    ///
    /// Only the JSON decoder is named: the operators keep the module bytes
    /// they had before the decoder existed.
    pub(crate) const fn name(self) -> Option<&'static str> {
        match self {
            Self::Compare | Self::Concat => None,
            Self::JsonDecode => Some(JSON_DECODE_FUNCTION_NAME),
        }
    }

    /// Encodes the helper body. `realloc_index` is the module's bounded bump
    /// allocator, whose failure paths trap before any byte is written.
    pub(crate) fn encode(self, realloc_index: u32) -> Function {
        match self {
            Self::Compare => encode_compare(),
            Self::Concat => encode_concat(realloc_index),
            Self::JsonDecode => encode_json_decode(realloc_index),
        }
    }
}

/// Collects the string helpers that live functions use, in index order.
pub(crate) fn used_helpers(module: &CoreModule, live: &LiveFunctions) -> BTreeSet<StringHelper> {
    let mut decoders = BTreeSet::new();
    for function in live.functions(module) {
        collect_block_decoders(&function.body, &mut decoders);
    }
    let mut helpers = BTreeSet::new();
    for function in live.functions(module) {
        let types = function_value_types(function);
        collect_block_helpers(&function.body, &types, &decoders, &mut helpers);
    }
    helpers
}

/// Collects the values that name the `json_decode` built-in.
fn collect_block_decoders(block: &CoreBlock, decoders: &mut BTreeSet<ValueId>) {
    for operation in &block.operations {
        if matches!(operation.kind, OperationKind::Builtin(Builtin::JsonDecode)) {
            decoders.insert(operation.result);
        }
        for_each_nested_block(operation, |nested| {
            collect_block_decoders(nested, decoders);
        });
    }
}

fn collect_block_helpers(
    block: &CoreBlock,
    types: &BTreeMap<ValueId, Arc<Type>>,
    decoders: &BTreeSet<ValueId>,
    helpers: &mut BTreeSet<StringHelper>,
) {
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Binary { left, operator, .. }
                if types.get(left).map(AsRef::as_ref) == Some(&Type::String) =>
            {
                if let Some(helper) = StringHelper::for_operator(*operator) {
                    helpers.insert(helper);
                }
            }
            OperationKind::Call { callee, .. }
                if decoders.contains(callee) && operation.ty.as_ref() == &Type::String =>
            {
                helpers.insert(StringHelper::JsonDecode);
            }
            _ => {}
        }
        for_each_nested_block(operation, |nested| {
            collect_block_helpers(nested, types, decoders, helpers);
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
const BYTES: MemArg = byte_at(0);

/// Byte-aligned access `offset` bytes past the address on the stack.
const fn byte_at(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

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

/// Locals of [`encode_json_decode`]; local 0 is the input pair.
mod json_local {
    /// Address of the first non-whitespace byte (the opening quote).
    pub(super) const FIRST: u32 = 1;
    /// Address one past the last non-whitespace byte.
    pub(super) const LAST: u32 = 2;
    /// Address of the first content byte; after decoding, the result bytes.
    pub(super) const CONTENT: u32 = 3;
    /// Address of the closing quote, which ends the content.
    pub(super) const CLOSE: u32 = 4;
    /// Address of the next unread content byte.
    pub(super) const CURSOR: u32 = 5;
    /// Decoded bytes so far; after a pass, the decoded length.
    pub(super) const OUT: u32 = 6;
    /// Fresh destination of the decoded bytes (write pass only).
    pub(super) const DEST: u32 = 7;
    /// The byte being examined.
    pub(super) const BYTE: u32 = 8;
    /// The scalar value an escape denotes.
    pub(super) const POINT: u32 = 9;
    /// A high surrogate awaiting its low surrogate.
    pub(super) const HIGH: u32 = 10;
    /// UTF-8 width of `POINT`.
    pub(super) const WIDTH: u32 = 11;
    /// Value of one hex digit.
    pub(super) const DIGIT: u32 = 12;
    /// Address the next decoded byte is written to.
    pub(super) const TARGET: u32 = 13;
    /// The result pair.
    pub(super) const PAIR: u32 = 14;
    /// Plain bytes before the first byte that needs the byte path.
    pub(super) const SKIP: u32 = 15;
    /// Copy index for skipped plain bytes (write pass only).
    pub(super) const INDEX: u32 = 16;
    /// Number of declared `i32` locals, which follow the parameter.
    pub(super) const I32_COUNT: u32 = 16;
    /// Eight content bytes examined at once.
    pub(super) const WORD: u32 = 17;
    /// High bit set in each byte of `WORD` that may need the byte path.
    pub(super) const MASK: u32 = 18;
    /// Number of declared `i64` locals, which follow the `i32` locals.
    pub(super) const I64_COUNT: u32 = 2;
}

/// Whether a scan only validates and measures or also writes the bytes.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Pass {
    Measure,
    Write,
}

/// The two-character escapes of RFC 8259 section 7 and the byte each
/// denotes; `\u` is decoded separately.
const SHORT_ESCAPES: [(u8, i32); 8] = [
    (b'"', 0x22),
    (b'\\', 0x5c),
    (b'/', 0x2f),
    (b'b', 0x08),
    (b'f', 0x0c),
    (b'n', 0x0a),
    (b'r', 0x0d),
    (b't', 0x09),
];

/// `json_decode(text) -> pair` for a `String` result.
///
/// The text must be one JSON string surrounded only by JSON whitespace
/// (space, tab, line feed, carriage return), exactly as the direct evaluator
/// requires before it yields a string. Inside the quotes every byte below
/// 0x20 and every unescaped `"` is rejected, each escape is one of
/// `\" \\ \/ \b \f \n \r \t \uXXXX` (hex digits in either case), a high
/// surrogate escape must be followed immediately by a low surrogate escape
/// and the pair is combined into one scalar, and a lone surrogate is
/// rejected. Rejection is an `unreachable` trap in this function. Input bytes
/// are already valid UTF-8 (every String is), and escapes only produce
/// scalar values, so the result is valid UTF-8 too.
///
/// A first pass validates the whole text and measures the decoded length
/// without allocating, so invalid input never allocates and fails the same
/// way regardless of size. Escapes always shrink the text, so a decoded
/// length equal to the content length means there was no escape: the result
/// then shares the input's content bytes, which is sound because bytes are
/// never mutated. Otherwise the exact length is allocated through the
/// bounded realloc export and a second pass writes the decoded bytes into
/// that fresh allocation only.
fn encode_json_decode(realloc_index: u32) -> Function {
    use json_local::{CLOSE, CONTENT, DEST, FIRST, LAST, OUT, PAIR};
    const INPUT: u32 = 0;

    let mut function = Function::new([
        (json_local::I32_COUNT, ValType::I32),
        (json_local::I64_COUNT, ValType::I64),
    ]);
    let f = &mut function;
    f.instruction(&Instruction::LocalGet(INPUT));
    f.instruction(&Instruction::I32Load(pair_word(0)));
    f.instruction(&Instruction::LocalTee(FIRST));
    f.instruction(&Instruction::LocalGet(INPUT));
    f.instruction(&Instruction::I32Load(pair_word(4)));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(LAST));
    trim_whitespace(f);

    // At least the two quotes remain, and both ends are quotes.
    f.instruction(&Instruction::LocalGet(LAST));
    f.instruction(&Instruction::LocalGet(FIRST));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(2));
    f.instruction(&Instruction::I32LtU);
    reject_if(f);
    f.instruction(&Instruction::LocalGet(FIRST));
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::I32Const(i32::from(b'"')));
    f.instruction(&Instruction::I32Ne);
    reject_if(f);
    f.instruction(&Instruction::LocalGet(LAST));
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::LocalTee(CLOSE));
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::I32Const(i32::from(b'"')));
    f.instruction(&Instruction::I32Ne);
    reject_if(f);
    f.instruction(&Instruction::LocalGet(FIRST));
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(CONTENT));

    scan_json_string(f, Pass::Measure);

    // A shorter decoded length means at least one escape: decode a copy.
    f.instruction(&Instruction::LocalGet(OUT));
    f.instruction(&Instruction::LocalGet(CLOSE));
    f.instruction(&Instruction::LocalGet(CONTENT));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::If(BlockType::Empty));
    allocate(f, realloc_index, 1, Allocation::Local(OUT));
    f.instruction(&Instruction::LocalSet(DEST));
    scan_json_string(f, Pass::Write);
    f.instruction(&Instruction::LocalGet(DEST));
    f.instruction(&Instruction::LocalSet(CONTENT));
    f.instruction(&Instruction::End);

    allocate(f, realloc_index, 4, Allocation::Constant(8));
    f.instruction(&Instruction::LocalTee(PAIR));
    f.instruction(&Instruction::LocalGet(CONTENT));
    f.instruction(&Instruction::I32Store(pair_word(0)));
    f.instruction(&Instruction::LocalGet(PAIR));
    f.instruction(&Instruction::LocalGet(OUT));
    f.instruction(&Instruction::I32Store(pair_word(4)));
    f.instruction(&Instruction::LocalGet(PAIR));
    f.instruction(&Instruction::End);
    function
}

/// Advances `FIRST` past leading and retreats `LAST` before trailing JSON
/// whitespace.
fn trim_whitespace(f: &mut Function) {
    use json_local::{BYTE, FIRST, LAST};

    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(FIRST));
    f.instruction(&Instruction::LocalGet(LAST));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    f.instruction(&Instruction::LocalGet(FIRST));
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::LocalSet(BYTE));
    push_is_whitespace(f, BYTE);
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::BrIf(1));
    increment(f, FIRST, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(LAST));
    f.instruction(&Instruction::LocalGet(FIRST));
    f.instruction(&Instruction::I32LeU);
    f.instruction(&Instruction::BrIf(1));
    f.instruction(&Instruction::LocalGet(LAST));
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::LocalSet(BYTE));
    push_is_whitespace(f, BYTE);
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::BrIf(1));
    increment(f, LAST, -1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Pushes whether the byte in `local` is RFC 8259 whitespace.
fn push_is_whitespace(f: &mut Function, local: u32) {
    for (index, byte) in b" \t\n\r".iter().enumerate() {
        f.instruction(&Instruction::LocalGet(local));
        f.instruction(&Instruction::I32Const(i32::from(*byte)));
        f.instruction(&Instruction::I32Eq);
        if index > 0 {
            f.instruction(&Instruction::I32Or);
        }
    }
}

/// Scans the content between the quotes, leaving the decoded length in
/// `OUT`. The write pass also stores each decoded byte at `DEST + OUT`.
///
/// Plain bytes are consumed eight at a time. At a word holding a control
/// character, quote, or backslash, the plain bytes before the first such
/// byte are consumed at once and that byte takes the byte path, as do the
/// last few bytes; a backslash right after an escape goes straight to the
/// byte path. `CURSOR <= CLOSE` holds throughout, so `CLOSE - CURSOR` never
/// wraps, and the byte at `CURSOR` is always readable (at `CLOSE` it is the
/// closing quote).
fn scan_json_string(f: &mut Function, pass: Pass) {
    use json_local::{
        BYTE, CLOSE, CONTENT, CURSOR, DEST, INDEX, MASK, OUT, SKIP, TARGET, WIDTH, WORD,
    };

    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(OUT));
    f.instruction(&Instruction::LocalGet(CONTENT));
    f.instruction(&Instruction::LocalSet(CURSOR));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));

    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::I32Const(i32::from(b'\\')));
    f.instruction(&Instruction::I32Eq);
    f.instruction(&Instruction::BrIf(0));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    push_remaining_at_least(f, 8);
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::BrIf(1));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I64Load(BYTES));
    f.instruction(&Instruction::LocalSet(WORD));
    push_special_byte_mask(f);
    f.instruction(&Instruction::LocalTee(MASK));
    f.instruction(&Instruction::I64Const(0));
    f.instruction(&Instruction::I64Ne);
    f.instruction(&Instruction::BrIf(1));
    if pass == Pass::Write {
        f.instruction(&Instruction::LocalGet(DEST));
        f.instruction(&Instruction::LocalGet(OUT));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(WORD));
        f.instruction(&Instruction::I64Store(BYTES));
    }
    increment(f, OUT, 8);
    increment(f, CURSOR, 8);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    // Stopped at a word with a special byte (not at the tail): its lowest
    // flagged byte is the first special one, and the bytes before it are
    // plain.
    push_remaining_at_least(f, 8);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(MASK));
    f.instruction(&Instruction::I64Ctz);
    f.instruction(&Instruction::I64Const(3));
    f.instruction(&Instruction::I64ShrU);
    f.instruction(&Instruction::I32WrapI64);
    f.instruction(&Instruction::LocalSet(SKIP));
    if pass == Pass::Write {
        f.instruction(&Instruction::LocalGet(DEST));
        f.instruction(&Instruction::LocalGet(OUT));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(TARGET));
        copy_bytes(f, TARGET, CURSOR, SKIP, INDEX);
    }
    f.instruction(&Instruction::LocalGet(OUT));
    f.instruction(&Instruction::LocalGet(SKIP));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(OUT));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::LocalGet(SKIP));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(CURSOR));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::LocalGet(CLOSE));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    if pass == Pass::Write {
        f.instruction(&Instruction::LocalGet(DEST));
        f.instruction(&Instruction::LocalGet(OUT));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(TARGET));
    }
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::LocalTee(BYTE));
    f.instruction(&Instruction::I32Const(i32::from(b'\\')));
    f.instruction(&Instruction::I32Eq);
    f.instruction(&Instruction::If(BlockType::Empty));
    decode_escape(f);
    push_utf8_width(f);
    f.instruction(&Instruction::LocalSet(WIDTH));
    if pass == Pass::Write {
        store_utf8(f);
    }
    f.instruction(&Instruction::LocalGet(OUT));
    f.instruction(&Instruction::LocalGet(WIDTH));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(OUT));
    f.instruction(&Instruction::Else);
    // An unescaped byte: never a control character or a quote.
    f.instruction(&Instruction::LocalGet(BYTE));
    f.instruction(&Instruction::I32Const(0x20));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::LocalGet(BYTE));
    f.instruction(&Instruction::I32Const(i32::from(b'"')));
    f.instruction(&Instruction::I32Eq);
    f.instruction(&Instruction::I32Or);
    reject_if(f);
    if pass == Pass::Write {
        f.instruction(&Instruction::LocalGet(TARGET));
        f.instruction(&Instruction::LocalGet(BYTE));
        f.instruction(&Instruction::I32Store8(BYTES));
    }
    increment(f, OUT, 1);
    increment(f, CURSOR, 1);
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Pushes whether at least `count` content bytes remain before `CLOSE`.
fn push_remaining_at_least(f: &mut Function, count: i32) {
    use json_local::{CLOSE, CURSOR};

    f.instruction(&Instruction::LocalGet(CLOSE));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(count));
    f.instruction(&Instruction::I32GeU);
}

/// Pushes an `i64` with the high bit set in every byte of `WORD` that may be
/// below 0x20, a quote, or a backslash. It is zero exactly when no byte is,
/// and its lowest set bit always marks a byte that is.
///
/// `(x - 0x0101..01) & !x & 0x8080..80` flags the zero bytes of `x`, and
/// with `0x2020..20` subtracted, the bytes below 0x20: a borrow only ever
/// starts at such a byte and can only falsely flag more significant (later)
/// bytes, and bytes with the high bit set (UTF-8 lead and continuation
/// bytes) are masked out by `!x`. A quote or backslash is a zero byte of `x`
/// xor the repeated byte, and `!(x ^ c)` is `x ^ !c`.
fn push_special_byte_mask(f: &mut Function) {
    use json_local::WORD;
    const ONES: i64 = 0x0101_0101_0101_0101;
    const HIGH_BITS: i64 = ONES << 7;

    f.instruction(&Instruction::LocalGet(WORD));
    f.instruction(&Instruction::I64Const(ONES * 0x20));
    f.instruction(&Instruction::I64Sub);
    f.instruction(&Instruction::LocalGet(WORD));
    f.instruction(&Instruction::I64Const(-1));
    f.instruction(&Instruction::I64Xor);
    f.instruction(&Instruction::I64And);
    for byte in *b"\"\\" {
        let repeated = ONES * i64::from(byte);
        f.instruction(&Instruction::LocalGet(WORD));
        f.instruction(&Instruction::I64Const(repeated));
        f.instruction(&Instruction::I64Xor);
        f.instruction(&Instruction::I64Const(ONES));
        f.instruction(&Instruction::I64Sub);
        f.instruction(&Instruction::LocalGet(WORD));
        f.instruction(&Instruction::I64Const(!repeated));
        f.instruction(&Instruction::I64Xor);
        f.instruction(&Instruction::I64And);
        f.instruction(&Instruction::I64Or);
    }
    f.instruction(&Instruction::I64Const(HIGH_BITS));
    f.instruction(&Instruction::I64And);
}

/// Decodes the escape at `CURSOR` (a backslash) into the scalar `POINT` and
/// advances `CURSOR` past it, reading only bytes before `CLOSE`.
///
/// One `br_table` over the escaped byte dispatches to a short escape, to the
/// `\u` decoder, or to rejection, so every escape costs the same to select.
fn decode_escape(f: &mut Function) {
    use json_local::{BYTE, CLOSE, CURSOR, HIGH, POINT};

    // The escaped character lies before the closing quote.
    f.instruction(&Instruction::LocalGet(CLOSE));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(2));
    f.instruction(&Instruction::I32LtU);
    reject_if(f);
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Load8U(byte_at(1)));
    f.instruction(&Instruction::LocalSet(BYTE));
    increment(f, CURSOR, 2);

    // Blocks, outermost first: done, unicode, invalid, then one per short
    // escape with `SHORT_ESCAPES[0]` innermost. Seen from the `br_table`,
    // short escape `i` is label `i`, invalid is `SHORT`, and unicode is
    // `SHORT + 1`.
    const SHORT: u32 = SHORT_ESCAPES.len() as u32;
    const FIRST: u8 = b'"';
    const LAST: u8 = b'u';
    let targets = (FIRST..=LAST)
        .map(|byte| {
            if byte == b'u' {
                return SHORT + 1;
            }
            SHORT_ESCAPES
                .iter()
                .zip(0..)
                .find_map(|((escape, _), label)| (*escape == byte).then_some(label))
                .unwrap_or(SHORT)
        })
        .collect::<Vec<_>>();
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Block(BlockType::Empty));
    for _ in SHORT_ESCAPES {
        f.instruction(&Instruction::Block(BlockType::Empty));
    }
    f.instruction(&Instruction::LocalGet(BYTE));
    f.instruction(&Instruction::I32Const(i32::from(FIRST)));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::BrTable(Cow::Owned(targets), SHORT));
    // After the block of short escape `i` ends, `done` is `SHORT - i` out.
    for ((_, point), label) in SHORT_ESCAPES.into_iter().zip(0..) {
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::I32Const(point));
        f.instruction(&Instruction::LocalSet(POINT));
        f.instruction(&Instruction::Br(SHORT + 1 - label));
    }
    f.instruction(&Instruction::End);
    // Any other escaped byte.
    f.instruction(&Instruction::Unreachable);
    f.instruction(&Instruction::End);

    decode_hex4(f);
    // A low surrogate cannot start a pair.
    push_in_range(f, POINT, 0xdc00, 0x400);
    reject_if(f);
    push_in_range(f, POINT, 0xd800, 0x400);
    f.instruction(&Instruction::If(BlockType::Empty));
    // A high surrogate must be followed at once by `\u` and a low surrogate.
    f.instruction(&Instruction::LocalGet(POINT));
    f.instruction(&Instruction::LocalSet(HIGH));
    f.instruction(&Instruction::LocalGet(CLOSE));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(6));
    f.instruction(&Instruction::I32LtU);
    reject_if(f);
    for (offset, expected) in [(0, b'\\'), (1, b'u')] {
        f.instruction(&Instruction::LocalGet(CURSOR));
        f.instruction(&Instruction::I32Load8U(byte_at(offset)));
        f.instruction(&Instruction::I32Const(i32::from(expected)));
        f.instruction(&Instruction::I32Ne);
        reject_if(f);
    }
    increment(f, CURSOR, 2);
    decode_hex4(f);
    push_in_range(f, POINT, 0xdc00, 0x400);
    f.instruction(&Instruction::I32Eqz);
    reject_if(f);
    // point = 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
    f.instruction(&Instruction::LocalGet(HIGH));
    f.instruction(&Instruction::I32Const(0xd800));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(10));
    f.instruction(&Instruction::I32Shl);
    f.instruction(&Instruction::LocalGet(POINT));
    f.instruction(&Instruction::I32Const(0xdc00));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::I32Const(0x1_0000));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(POINT));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Decodes the four hex digits at `CURSOR` into `POINT` and advances
/// `CURSOR` past them, reading only bytes before `CLOSE`.
fn decode_hex4(f: &mut Function) {
    use json_local::{BYTE, CLOSE, CURSOR, DIGIT, POINT};

    f.instruction(&Instruction::LocalGet(CLOSE));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(4));
    f.instruction(&Instruction::I32LtU);
    reject_if(f);
    for offset in 0..4 {
        f.instruction(&Instruction::LocalGet(CURSOR));
        f.instruction(&Instruction::I32Load8U(byte_at(offset)));
        f.instruction(&Instruction::LocalTee(BYTE));
        f.instruction(&Instruction::I32Const(i32::from(b'0')));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalTee(DIGIT));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        // Setting bit 5 maps exactly `A`-`F` and `a`-`f` onto `a`-`f`.
        f.instruction(&Instruction::LocalGet(BYTE));
        f.instruction(&Instruction::I32Const(0x20));
        f.instruction(&Instruction::I32Or);
        f.instruction(&Instruction::I32Const(i32::from(b'a')));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalTee(DIGIT));
        f.instruction(&Instruction::I32Const(6));
        f.instruction(&Instruction::I32GeU);
        reject_if(f);
        increment(f, DIGIT, 10);
        f.instruction(&Instruction::End);
        if offset == 0 {
            f.instruction(&Instruction::LocalGet(DIGIT));
        } else {
            f.instruction(&Instruction::LocalGet(POINT));
            f.instruction(&Instruction::I32Const(4));
            f.instruction(&Instruction::I32Shl);
            f.instruction(&Instruction::LocalGet(DIGIT));
            f.instruction(&Instruction::I32Or);
        }
        f.instruction(&Instruction::LocalSet(POINT));
    }
    increment(f, CURSOR, 4);
}

/// Pushes whether `start <= local < start + length` (unsigned).
fn push_in_range(f: &mut Function, local: u32, start: i32, length: i32) {
    f.instruction(&Instruction::LocalGet(local));
    f.instruction(&Instruction::I32Const(start));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(length));
    f.instruction(&Instruction::I32LtU);
}

/// Pushes the UTF-8 width of the scalar `POINT`: 1, 2, 3, or 4 bytes.
fn push_utf8_width(f: &mut Function) {
    use json_local::POINT;

    f.instruction(&Instruction::I32Const(1));
    for boundary in [0x80, 0x800, 0x1_0000] {
        f.instruction(&Instruction::LocalGet(POINT));
        f.instruction(&Instruction::I32Const(boundary));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::I32Add);
    }
}

/// Stores the UTF-8 encoding of `POINT`, `WIDTH` bytes long, at `TARGET`.
fn store_utf8(f: &mut Function) {
    use json_local::WIDTH;

    for width in 1..=3 {
        f.instruction(&Instruction::LocalGet(WIDTH));
        f.instruction(&Instruction::I32Const(i32::from(width)));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        store_utf8_width(f, width);
        f.instruction(&Instruction::Else);
    }
    store_utf8_width(f, 4);
    for _ in 1..=3 {
        f.instruction(&Instruction::End);
    }
}

/// Stores the `width`-byte UTF-8 encoding of `POINT` at `TARGET`: a lead
/// byte carrying the high bits, then six bits per continuation byte.
fn store_utf8_width(f: &mut Function, width: u8) {
    use json_local::{POINT, TARGET};

    let lead = match width {
        1 => 0x00,
        2 => 0xc0,
        3 => 0xe0,
        _ => 0xf0,
    };
    for index in 0..width {
        let shift = i32::from(6 * (width - 1 - index));
        f.instruction(&Instruction::LocalGet(TARGET));
        f.instruction(&Instruction::LocalGet(POINT));
        if shift > 0 {
            f.instruction(&Instruction::I32Const(shift));
            f.instruction(&Instruction::I32ShrU);
        }
        if index == 0 {
            if lead != 0 {
                f.instruction(&Instruction::I32Const(lead));
                f.instruction(&Instruction::I32Or);
            }
        } else {
            f.instruction(&Instruction::I32Const(0x3f));
            f.instruction(&Instruction::I32And);
            f.instruction(&Instruction::I32Const(0x80));
            f.instruction(&Instruction::I32Or);
        }
        f.instruction(&Instruction::I32Store8(byte_at(u64::from(index))));
    }
}

/// Traps when the condition on the stack holds: the input is not one JSON
/// string. The host reports a trap in the named decoder as `K4009`.
fn reject_if(f: &mut Function) {
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::Unreachable);
    f.instruction(&Instruction::End);
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
