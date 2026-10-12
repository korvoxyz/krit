//! Guest standard library for the bounded webhook ABI (WebAssembly policy 2).
//!
//! These generated core functions lower the standard-library calls that need
//! more than a load (`string_byte_len` and `string_is_empty` read a pair's
//! length inline): `int_to_string`, the three string searches, and the two
//! URL builders. They are [`StringHelper`](crate::strings::StringHelper)s, so
//! each is emitted at most once per module, only when a live function calls
//! it, after every other definition, using only MVP instructions.
//!
//! Every function computes exactly what the direct evaluator computes: the
//! searches compare UTF-8 bytes, and the URL builders implement the shared
//! definitions in [`krit_capability::request_target`] byte for byte,
//! including the `Err` texts of `url_path_segment`, which are written from
//! [`PathSegmentError::message`] itself. Builders allocate their results
//! through the module's bounded `realloc`, which traps before anything is
//! written when guest memory is exhausted; the predicates never allocate.

use krit_capability::request_target::PathSegmentError;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

use crate::strings::{
    Allocation, BYTES, allocate, byte_at, increment, offset_address, pair_word, push_in_range,
};

/// The variant tags of a `Result` value's first byte.
const OK_TAG: i32 = 0;
const ERR_TAG: i32 = 1;

/// Size and alignment of a `Result<String, String>` value: the tag byte, then
/// the payload pair at offset 4.
const RESULT_SIZE: i32 = 12;
const RESULT_PAYLOAD: u64 = 4;

/// Bytes of one element of a list of name/value records: the name pair, then
/// the value pair.
const ENTRY_SIZE: i32 = 16;

/// The refusals of `url_path_segment`, indexed by the reason code the guest
/// computes, in the order the host rule's reduction checks them.
const REFUSALS: [PathSegmentError; 4] = [
    PathSegmentError::Empty,
    PathSegmentError::ForbiddenByte,
    PathSegmentError::EscapeLike,
    PathSegmentError::DotSegment,
];

const fn reason_code(error: PathSegmentError) -> i32 {
    match error {
        PathSegmentError::Empty => 0,
        PathSegmentError::ForbiddenByte => 1,
        PathSegmentError::EscapeLike => 2,
        PathSegmentError::DotSegment => 3,
    }
}

/// Where a search may find its part in the text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Search {
    /// At offset zero only: `string_starts_with`.
    Prefix,
    /// At `text_len - part_len` only: `string_ends_with`.
    Suffix,
    /// At any offset from zero to `text_len - part_len`: `string_contains`.
    Anywhere,
}

/// `int_to_string(value: i64) -> pair`.
///
/// One 28-byte allocation holds the pair and, after it, room for the longest
/// text (`-9223372036854775808`, 20 bytes). Digits are written from the end:
/// the magnitude is `0 - value` as an unsigned 64-bit number for a negative
/// value, which is exact for `i64::MIN`, and unsigned division by ten peels
/// one digit at a time, so zero is written as `0`.
pub(crate) fn encode_int_to_string(realloc_index: u32) -> Function {
    const VALUE: u32 = 0;
    const BLOCK: u32 = 1;
    const CURSOR: u32 = 2;
    const MAGNITUDE: u32 = 3;
    const PAIR_SIZE: i32 = 8;
    const MAX_DIGITS: i32 = 20;
    const END: i32 = PAIR_SIZE + MAX_DIGITS;

    let mut function = Function::new([(2, ValType::I32), (1, ValType::I64)]);
    let f = &mut function;
    allocate(f, realloc_index, 4, Allocation::Constant(END));
    f.instruction(&Instruction::LocalTee(BLOCK));
    f.instruction(&Instruction::I32Const(END));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(CURSOR));

    // magnitude = value < 0 ? 0 - value : value (as an unsigned number)
    f.instruction(&Instruction::I64Const(0));
    f.instruction(&Instruction::LocalGet(VALUE));
    f.instruction(&Instruction::I64Sub);
    f.instruction(&Instruction::LocalGet(VALUE));
    f.instruction(&Instruction::LocalGet(VALUE));
    f.instruction(&Instruction::I64Const(0));
    f.instruction(&Instruction::I64LtS);
    f.instruction(&Instruction::Select);
    f.instruction(&Instruction::LocalSet(MAGNITUDE));

    f.instruction(&Instruction::Loop(BlockType::Empty));
    increment(f, CURSOR, -1);
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::LocalGet(MAGNITUDE));
    f.instruction(&Instruction::I64Const(10));
    f.instruction(&Instruction::I64RemU);
    f.instruction(&Instruction::I32WrapI64);
    f.instruction(&Instruction::I32Const(i32::from(b'0')));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::I32Store8(BYTES));
    f.instruction(&Instruction::LocalGet(MAGNITUDE));
    f.instruction(&Instruction::I64Const(10));
    f.instruction(&Instruction::I64DivU);
    f.instruction(&Instruction::LocalTee(MAGNITUDE));
    f.instruction(&Instruction::I64Const(0));
    f.instruction(&Instruction::I64Ne);
    f.instruction(&Instruction::BrIf(0));
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::LocalGet(VALUE));
    f.instruction(&Instruction::I64Const(0));
    f.instruction(&Instruction::I64LtS);
    f.instruction(&Instruction::If(BlockType::Empty));
    increment(f, CURSOR, -1);
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Const(i32::from(b'-')));
    f.instruction(&Instruction::I32Store8(BYTES));
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::LocalGet(BLOCK));
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Store(pair_word(0)));
    f.instruction(&Instruction::LocalGet(BLOCK));
    f.instruction(&Instruction::LocalGet(BLOCK));
    f.instruction(&Instruction::I32Const(END));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalGet(CURSOR));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Store(pair_word(4)));
    f.instruction(&Instruction::LocalGet(BLOCK));
    f.instruction(&Instruction::End);
    function
}

/// `search(text, part) -> i32`: whether `part`'s bytes occur in `text` at an
/// offset `search` allows.
///
/// A part longer than the text never occurs, and the empty part occurs at
/// every allowed offset. Each candidate offset compares eight bytes at a time
/// and then byte by byte, stopping at the first difference. `string_contains`
/// tries every offset in order, so a text of `n` bytes and a part of `m` bytes
/// cost at most `(n - m + 1) * m` byte comparisons; fuel bounds the work like
/// any other guest loop. Nothing is allocated.
pub(crate) fn encode_search(search: Search) -> Function {
    const TEXT: u32 = 0;
    const PART: u32 = 1;
    const TEXT_PTR: u32 = 2;
    const TEXT_LEN: u32 = 3;
    const PART_PTR: u32 = 4;
    const PART_LEN: u32 = 5;
    const POSITION: u32 = 6;
    const LAST: u32 = 7;
    const BASE: u32 = 8;
    const INDEX: u32 = 9;

    let mut function = Function::new([(8, ValType::I32)]);
    let f = &mut function;
    for (pair, word, local) in [
        (TEXT, 0, TEXT_PTR),
        (TEXT, 4, TEXT_LEN),
        (PART, 0, PART_PTR),
        (PART, 4, PART_LEN),
    ] {
        f.instruction(&Instruction::LocalGet(pair));
        f.instruction(&Instruction::I32Load(pair_word(word)));
        f.instruction(&Instruction::LocalSet(local));
    }
    f.instruction(&Instruction::LocalGet(PART_LEN));
    f.instruction(&Instruction::LocalGet(TEXT_LEN));
    f.instruction(&Instruction::I32GtU);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::Return);
    f.instruction(&Instruction::End);
    // last = text_len - part_len, which no longer wraps.
    f.instruction(&Instruction::LocalGet(TEXT_LEN));
    f.instruction(&Instruction::LocalGet(PART_LEN));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::LocalSet(LAST));
    if search == Search::Suffix {
        f.instruction(&Instruction::LocalGet(LAST));
    } else {
        f.instruction(&Instruction::I32Const(0));
    }
    f.instruction(&Instruction::LocalSet(POSITION));

    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(TEXT_PTR));
    f.instruction(&Instruction::LocalGet(POSITION));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(BASE));
    branch_unless_equal(f, BASE, PART_PTR, PART_LEN, INDEX);
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::Return);
    f.instruction(&Instruction::End);
    if search == Search::Anywhere {
        f.instruction(&Instruction::LocalGet(POSITION));
        f.instruction(&Instruction::LocalGet(LAST));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        increment(f, POSITION, 1);
        f.instruction(&Instruction::Br(0));
    }
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::End);
    function
}

/// Falls through when the `length` bytes at `left` equal those at `right`,
/// and otherwise branches to the label enclosing this code (depth 0 where it
/// is emitted). `index` is scratch.
fn branch_unless_equal(f: &mut Function, left: u32, right: u32, length: u32, index: u32) {
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(index));
    // Equal words: `index <= length` holds throughout, so `length - index`
    // never wraps.
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(length));
    f.instruction(&Instruction::LocalGet(index));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(8));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::BrIf(1));
    for base in [left, right] {
        offset_address(f, base, index);
        f.instruction(&Instruction::I64Load(BYTES));
    }
    f.instruction(&Instruction::I64Ne);
    f.instruction(&Instruction::BrIf(2));
    increment(f, index, 8);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(index));
    f.instruction(&Instruction::LocalGet(length));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    for base in [left, right] {
        offset_address(f, base, index);
        f.instruction(&Instruction::I32Load8U(BYTES));
    }
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::BrIf(2));
    increment(f, index, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Locals of [`encode_path_segment`]; local 0 is the input pair.
mod segment_local {
    pub(super) const PTR: u32 = 1;
    pub(super) const LEN: u32 = 2;
    pub(super) const INDEX: u32 = 3;
    pub(super) const BYTE: u32 = 4;
    /// Bytes that are not unreserved, each of which encodes to three.
    pub(super) const RESERVED: u32 = 5;
    /// Offset of the first `;`, `?`, or `#`, or `LEN` when there is none.
    pub(super) const CUT: u32 = 6;
    /// Whether a `%` is followed by a hex digit, `u`, or `U`.
    pub(super) const ESCAPE: u32 = 7;
    /// Start of the trimmed name; while writing, the write cursor.
    pub(super) const START: u32 = 8;
    /// End of the trimmed name.
    pub(super) const END: u32 = 9;
    pub(super) const OUT_PTR: u32 = 10;
    pub(super) const OUT_LEN: u32 = 11;
    /// The refusal reason code while checking ([`super::reason_code`]).
    pub(super) const REASON: u32 = 12;
    pub(super) const RESULT: u32 = 13;
    /// The byte after a `%`; the trimmed name's length; a hex nibble.
    pub(super) const SCRATCH: u32 = 14;
    pub(super) const I32_COUNT: u32 = 14;
}

/// `url_path_segment(text) -> result`.
///
/// One scan over the text refuses the first `/`, `\`, or ASCII control byte
/// at once, and otherwise records whether a `%` is followed by a hex digit,
/// `u`, or `U`, counts the bytes that are not unreserved, and finds the first
/// `;`, `?`, or `#`. The checks then apply in the order of
/// [`PathSegmentError::check`]: empty, forbidden byte, escape, and dot
/// segment (the part before the cut, with spaces trimmed from both ends, is
/// `.` or `..`; no other whitespace byte can remain, because every one is a
/// control byte). An accepted text without reserved bytes shares the input's
/// bytes; otherwise the exact encoded length (`len + 2 * reserved`, which
/// cannot wrap) is allocated and written with uppercase hex escapes. The
/// result is a fresh `Result<String, String>`, whose `Err` text is the
/// refusal's message, written into a fresh allocation.
pub(crate) fn encode_path_segment(realloc_index: u32) -> Function {
    use segment_local::{
        BYTE, CUT, END, ESCAPE, INDEX, LEN, OUT_LEN, OUT_PTR, PTR, REASON, RESERVED, RESULT,
        SCRATCH, START,
    };
    const INPUT: u32 = 0;

    let mut function = Function::new([(segment_local::I32_COUNT, ValType::I32)]);
    let f = &mut function;
    f.instruction(&Instruction::LocalGet(INPUT));
    f.instruction(&Instruction::I32Load(pair_word(0)));
    f.instruction(&Instruction::LocalSet(PTR));
    f.instruction(&Instruction::LocalGet(INPUT));
    f.instruction(&Instruction::I32Load(pair_word(4)));
    f.instruction(&Instruction::LocalSet(LEN));

    // Block `refuse`: every refusal sets `REASON` and branches out of it; the
    // accepted path returns from inside it.
    f.instruction(&Instruction::Block(BlockType::Empty));
    set_reason(f, PathSegmentError::Empty);
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::BrIf(0));

    set_reason(f, PathSegmentError::ForbiddenByte);
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::LocalSet(CUT));
    for local in [INDEX, RESERVED, ESCAPE] {
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(local));
    }
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(INDEX));
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    offset_address(f, PTR, INDEX);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::LocalTee(BYTE));
    // A forbidden byte refuses the text at once: no later reason outranks it.
    f.instruction(&Instruction::I32Const(0x20));
    f.instruction(&Instruction::I32LtU);
    for forbidden in [b'/', b'\\', 0x7f] {
        push_byte_is(f, BYTE, forbidden);
        f.instruction(&Instruction::I32Or);
    }
    f.instruction(&Instruction::BrIf(2));
    // A `%` with a following hex digit, `u`, or `U`.
    push_byte_is(f, BYTE, b'%');
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(INDEX));
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::If(BlockType::Empty));
    offset_address(f, PTR, INDEX);
    f.instruction(&Instruction::I32Load8U(byte_at(1)));
    f.instruction(&Instruction::LocalSet(SCRATCH));
    push_is_hex_digit(f, SCRATCH);
    push_byte_is(f, SCRATCH, b'u');
    f.instruction(&Instruction::I32Or);
    push_byte_is(f, SCRATCH, b'U');
    f.instruction(&Instruction::I32Or);
    f.instruction(&Instruction::LocalGet(ESCAPE));
    f.instruction(&Instruction::I32Or);
    f.instruction(&Instruction::LocalSet(ESCAPE));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
    // Reserved bytes encode to three.
    push_is_unreserved(f, BYTE);
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::If(BlockType::Empty));
    increment(f, RESERVED, 1);
    f.instruction(&Instruction::End);
    // The first `;`, `?`, or `#` ends the name a server would resolve.
    f.instruction(&Instruction::LocalGet(CUT));
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::I32Eq);
    push_byte_is(f, BYTE, b';');
    push_byte_is(f, BYTE, b'?');
    f.instruction(&Instruction::I32Or);
    push_byte_is(f, BYTE, b'#');
    f.instruction(&Instruction::I32Or);
    f.instruction(&Instruction::I32And);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(INDEX));
    f.instruction(&Instruction::LocalSet(CUT));
    f.instruction(&Instruction::End);
    increment(f, INDEX, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    set_reason(f, PathSegmentError::EscapeLike);
    f.instruction(&Instruction::LocalGet(ESCAPE));
    f.instruction(&Instruction::BrIf(0));

    // Trim spaces from both ends of the name before the cut.
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(START));
    f.instruction(&Instruction::LocalGet(CUT));
    f.instruction(&Instruction::LocalSet(END));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(START));
    f.instruction(&Instruction::LocalGet(END));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    offset_address(f, PTR, START);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::I32Const(i32::from(b' ')));
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::BrIf(1));
    increment(f, START, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(END));
    f.instruction(&Instruction::LocalGet(START));
    f.instruction(&Instruction::I32LeU);
    f.instruction(&Instruction::BrIf(1));
    f.instruction(&Instruction::LocalGet(PTR));
    f.instruction(&Instruction::LocalGet(END));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::I32Const(i32::from(b' ')));
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::BrIf(1));
    increment(f, END, -1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    // A trimmed name of one or two bytes, every one a `.`, is a dot segment.
    // Block `not_dot`; only bytes before `END` are read.
    set_reason(f, PathSegmentError::DotSegment);
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(END));
    f.instruction(&Instruction::LocalGet(START));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::LocalTee(SCRATCH));
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::LocalGet(SCRATCH));
    f.instruction(&Instruction::I32Const(2));
    f.instruction(&Instruction::I32GtU);
    f.instruction(&Instruction::I32Or);
    f.instruction(&Instruction::BrIf(0));
    offset_address(f, PTR, START);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::I32Const(i32::from(b'.')));
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::BrIf(0));
    f.instruction(&Instruction::LocalGet(SCRATCH));
    f.instruction(&Instruction::I32Const(2));
    f.instruction(&Instruction::I32Eq);
    f.instruction(&Instruction::If(BlockType::Empty));
    offset_address(f, PTR, START);
    f.instruction(&Instruction::I32Load8U(byte_at(1)));
    f.instruction(&Instruction::I32Const(i32::from(b'.')));
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::BrIf(1));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::Br(1));
    f.instruction(&Instruction::End);

    // Accepted: share the input's bytes, or write the encoding.
    f.instruction(&Instruction::LocalGet(PTR));
    f.instruction(&Instruction::LocalSet(OUT_PTR));
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::LocalSet(OUT_LEN));
    f.instruction(&Instruction::LocalGet(RESERVED));
    f.instruction(&Instruction::If(BlockType::Empty));
    // out_len = len + 2 * reserved; reserved <= len < 2^31, so a sum that
    // wrapped would be smaller than len.
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::LocalGet(RESERVED));
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Shl);
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalTee(OUT_LEN));
    f.instruction(&Instruction::LocalGet(LEN));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::Unreachable);
    f.instruction(&Instruction::End);
    allocate(f, realloc_index, 1, Allocation::Local(OUT_LEN));
    f.instruction(&Instruction::LocalTee(OUT_PTR));
    f.instruction(&Instruction::LocalSet(START));
    write_percent_encoded(f, PTR, LEN, INDEX, BYTE, START, SCRATCH);
    f.instruction(&Instruction::End);
    store_result(f, realloc_index, RESULT, OK_TAG, OUT_PTR, OUT_LEN);
    f.instruction(&Instruction::LocalGet(RESULT));
    f.instruction(&Instruction::Return);
    f.instruction(&Instruction::End);

    // Refused: the message of the reason in `REASON`.
    for error in REFUSALS {
        let message = error.message().as_bytes();
        let length = i32::try_from(message.len()).expect("refusal messages are short");
        f.instruction(&Instruction::LocalGet(REASON));
        f.instruction(&Instruction::I32Const(reason_code(error)));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        allocate(f, realloc_index, 1, Allocation::Constant(length));
        f.instruction(&Instruction::LocalSet(OUT_PTR));
        store_bytes(f, OUT_PTR, message);
        f.instruction(&Instruction::I32Const(length));
        f.instruction(&Instruction::LocalSet(OUT_LEN));
        f.instruction(&Instruction::End);
    }
    store_result(f, realloc_index, RESULT, ERR_TAG, OUT_PTR, OUT_LEN);
    f.instruction(&Instruction::LocalGet(RESULT));
    f.instruction(&Instruction::End);
    function
}

fn set_reason(f: &mut Function, error: PathSegmentError) {
    f.instruction(&Instruction::I32Const(reason_code(error)));
    f.instruction(&Instruction::LocalSet(segment_local::REASON));
}

/// Allocates a `Result<String, String>` into `result` with tag `tag` and the
/// payload pair `(ptr, len)`.
fn store_result(f: &mut Function, realloc_index: u32, result: u32, tag: i32, ptr: u32, len: u32) {
    allocate(f, realloc_index, 4, Allocation::Constant(RESULT_SIZE));
    f.instruction(&Instruction::LocalTee(result));
    f.instruction(&Instruction::I32Const(tag));
    f.instruction(&Instruction::I32Store8(BYTES));
    f.instruction(&Instruction::LocalGet(result));
    f.instruction(&Instruction::LocalGet(ptr));
    f.instruction(&Instruction::I32Store(pair_word(RESULT_PAYLOAD)));
    f.instruction(&Instruction::LocalGet(result));
    f.instruction(&Instruction::LocalGet(len));
    f.instruction(&Instruction::I32Store(pair_word(RESULT_PAYLOAD + 4)));
}

/// Stores the constant `bytes` at the address in `base`, eight at a time.
fn store_bytes(f: &mut Function, base: u32, bytes: &[u8]) {
    for (chunk, offset) in bytes.chunks(8).zip((0u64..).step_by(8)) {
        if let Ok(word) = <[u8; 8]>::try_from(chunk) {
            f.instruction(&Instruction::LocalGet(base));
            f.instruction(&Instruction::I64Const(i64::from_le_bytes(word)));
            f.instruction(&Instruction::I64Store(byte_at(offset)));
        } else {
            for (byte, index) in chunk.iter().zip(offset..) {
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I32Const(i32::from(*byte)));
                f.instruction(&Instruction::I32Store8(byte_at(index)));
            }
        }
    }
}

/// Locals of [`encode_query`]; local 0 is the list.
mod query_local {
    pub(super) const ARRAY: u32 = 1;
    pub(super) const COUNT: u32 = 2;
    pub(super) const ENTRY: u32 = 3;
    pub(super) const ELEMENT: u32 = 4;
    pub(super) const PIECE_PTR: u32 = 5;
    pub(super) const PIECE_LEN: u32 = 6;
    pub(super) const INDEX: u32 = 7;
    pub(super) const BYTE: u32 = 8;
    pub(super) const RESERVED: u32 = 9;
    pub(super) const DEST: u32 = 10;
    pub(super) const CURSOR: u32 = 11;
    pub(super) const TOTAL: u32 = 12;
    pub(super) const PAIR: u32 = 13;
    pub(super) const NIBBLE: u32 = 14;
    pub(super) const I32_COUNT: u32 = 14;
    /// The measured length, kept in 64 bits: entries may share their bytes,
    /// so the encoding of a list can be longer than guest memory.
    pub(super) const TOTAL64: u32 = 15;
    pub(super) const I64_COUNT: u32 = 1;
}

/// `url_query(list) -> pair`.
///
/// A list of name/value records is a pair of the element array (16 bytes per
/// element: the name pair, then the value pair) and the element count. A
/// first pass measures `name=value` per element plus one `&` between
/// elements in 64 bits, and a length beyond `i32::MAX` traps before anything
/// is allocated. A second pass writes every name and value with uppercase
/// hex escapes into one allocation of exactly that length. The empty list
/// gives the empty string.
pub(crate) fn encode_query(realloc_index: u32) -> Function {
    use query_local::{
        ARRAY, BYTE, COUNT, CURSOR, DEST, ENTRY, INDEX, NIBBLE, PAIR, PIECE_LEN, PIECE_PTR,
        RESERVED, TOTAL, TOTAL64,
    };
    const LIST: u32 = 0;

    let mut function = Function::new([
        (query_local::I32_COUNT, ValType::I32),
        (query_local::I64_COUNT, ValType::I64),
    ]);
    let f = &mut function;
    f.instruction(&Instruction::LocalGet(LIST));
    f.instruction(&Instruction::I32Load(pair_word(0)));
    f.instruction(&Instruction::LocalSet(ARRAY));
    f.instruction(&Instruction::LocalGet(LIST));
    f.instruction(&Instruction::I32Load(pair_word(4)));
    f.instruction(&Instruction::LocalSet(COUNT));

    // Measure.
    f.instruction(&Instruction::I64Const(0));
    f.instruction(&Instruction::LocalSet(TOTAL64));
    for_each_entry(f, |f| {
        for field in [0, 8] {
            load_piece(f, field);
            count_reserved(f, PIECE_PTR, PIECE_LEN, INDEX, BYTE, RESERVED);
            // total += len + 2 * reserved
            f.instruction(&Instruction::LocalGet(TOTAL64));
            f.instruction(&Instruction::LocalGet(PIECE_LEN));
            f.instruction(&Instruction::I64ExtendI32U);
            f.instruction(&Instruction::I64Add);
            f.instruction(&Instruction::LocalGet(RESERVED));
            f.instruction(&Instruction::I64ExtendI32U);
            f.instruction(&Instruction::I64Const(1));
            f.instruction(&Instruction::I64Shl);
            f.instruction(&Instruction::I64Add);
            f.instruction(&Instruction::LocalSet(TOTAL64));
        }
        // One `=`, and one `&` before every entry but the first.
        f.instruction(&Instruction::LocalGet(TOTAL64));
        f.instruction(&Instruction::I64Const(1));
        f.instruction(&Instruction::I64Add);
        f.instruction(&Instruction::LocalGet(ENTRY));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Add);
        f.instruction(&Instruction::LocalSet(TOTAL64));
    });
    f.instruction(&Instruction::LocalGet(TOTAL64));
    f.instruction(&Instruction::I64Const(i64::from(i32::MAX)));
    f.instruction(&Instruction::I64GtU);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::Unreachable);
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::LocalGet(TOTAL64));
    f.instruction(&Instruction::I32WrapI64);
    f.instruction(&Instruction::LocalSet(TOTAL));

    // Write.
    allocate(f, realloc_index, 1, Allocation::Local(TOTAL));
    f.instruction(&Instruction::LocalTee(DEST));
    f.instruction(&Instruction::LocalSet(CURSOR));
    for_each_entry(f, |f| {
        f.instruction(&Instruction::LocalGet(ENTRY));
        f.instruction(&Instruction::If(BlockType::Empty));
        store_cursor_byte(f, CURSOR, b'&');
        f.instruction(&Instruction::End);
        load_piece(f, 0);
        write_percent_encoded(f, PIECE_PTR, PIECE_LEN, INDEX, BYTE, CURSOR, NIBBLE);
        store_cursor_byte(f, CURSOR, b'=');
        load_piece(f, 8);
        write_percent_encoded(f, PIECE_PTR, PIECE_LEN, INDEX, BYTE, CURSOR, NIBBLE);
    });

    allocate(f, realloc_index, 4, Allocation::Constant(8));
    f.instruction(&Instruction::LocalTee(PAIR));
    f.instruction(&Instruction::LocalGet(DEST));
    f.instruction(&Instruction::I32Store(pair_word(0)));
    f.instruction(&Instruction::LocalGet(PAIR));
    f.instruction(&Instruction::LocalGet(TOTAL));
    f.instruction(&Instruction::I32Store(pair_word(4)));
    f.instruction(&Instruction::LocalGet(PAIR));
    f.instruction(&Instruction::End);
    function
}

/// Runs `body` once per list element in order, with `ENTRY` its index and
/// `ELEMENT` its address. `body` must not branch out of its own blocks.
fn for_each_entry(f: &mut Function, body: impl FnOnce(&mut Function)) {
    use query_local::{ARRAY, COUNT, ELEMENT, ENTRY};

    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(ENTRY));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(ENTRY));
    f.instruction(&Instruction::LocalGet(COUNT));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    f.instruction(&Instruction::LocalGet(ARRAY));
    f.instruction(&Instruction::LocalGet(ENTRY));
    f.instruction(&Instruction::I32Const(ENTRY_SIZE));
    f.instruction(&Instruction::I32Mul);
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(ELEMENT));
    body(f);
    increment(f, ENTRY, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Loads the pair at `field` bytes into the current element into
/// `PIECE_PTR` and `PIECE_LEN`.
fn load_piece(f: &mut Function, field: u64) {
    use query_local::{ELEMENT, PIECE_LEN, PIECE_PTR};

    f.instruction(&Instruction::LocalGet(ELEMENT));
    f.instruction(&Instruction::I32Load(pair_word(field)));
    f.instruction(&Instruction::LocalSet(PIECE_PTR));
    f.instruction(&Instruction::LocalGet(ELEMENT));
    f.instruction(&Instruction::I32Load(pair_word(field + 4)));
    f.instruction(&Instruction::LocalSet(PIECE_LEN));
}

/// Stores `byte` at `cursor` and advances it.
fn store_cursor_byte(f: &mut Function, cursor: u32, byte: u8) {
    f.instruction(&Instruction::LocalGet(cursor));
    f.instruction(&Instruction::I32Const(i32::from(byte)));
    f.instruction(&Instruction::I32Store8(BYTES));
    increment(f, cursor, 1);
}

/// Sets `reserved` to the number of the `length` bytes at `ptr` that are not
/// unreserved.
fn count_reserved(f: &mut Function, ptr: u32, length: u32, index: u32, byte: u32, reserved: u32) {
    for local in [index, reserved] {
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(local));
    }
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(index));
    f.instruction(&Instruction::LocalGet(length));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    offset_address(f, ptr, index);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::LocalSet(byte));
    f.instruction(&Instruction::LocalGet(reserved));
    push_is_unreserved(f, byte);
    f.instruction(&Instruction::I32Eqz);
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(reserved));
    increment(f, index, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Writes the `length` bytes at `ptr` at `cursor`, each unreserved byte as
/// itself and every other byte as `%HH` with uppercase hex digits, and
/// advances `cursor` past them. `index`, `byte`, and `nibble` are scratch.
fn write_percent_encoded(
    f: &mut Function,
    ptr: u32,
    length: u32,
    index: u32,
    byte: u32,
    cursor: u32,
    nibble: u32,
) {
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(index));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(index));
    f.instruction(&Instruction::LocalGet(length));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    offset_address(f, ptr, index);
    f.instruction(&Instruction::I32Load8U(BYTES));
    f.instruction(&Instruction::LocalSet(byte));
    push_is_unreserved(f, byte);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(cursor));
    f.instruction(&Instruction::LocalGet(byte));
    f.instruction(&Instruction::I32Store8(BYTES));
    increment(f, cursor, 1);
    f.instruction(&Instruction::Else);
    f.instruction(&Instruction::LocalGet(cursor));
    f.instruction(&Instruction::I32Const(i32::from(b'%')));
    f.instruction(&Instruction::I32Store8(BYTES));
    for (shift, offset) in [(Some(4), 1), (None, 2)] {
        f.instruction(&Instruction::LocalGet(byte));
        match shift {
            Some(shift) => {
                f.instruction(&Instruction::I32Const(shift));
                f.instruction(&Instruction::I32ShrU);
            }
            None => {
                f.instruction(&Instruction::I32Const(0x0f));
                f.instruction(&Instruction::I32And);
            }
        }
        f.instruction(&Instruction::LocalSet(nibble));
        f.instruction(&Instruction::LocalGet(cursor));
        push_hex_digit(f, nibble);
        f.instruction(&Instruction::I32Store8(byte_at(offset)));
    }
    increment(f, cursor, 3);
    f.instruction(&Instruction::End);
    increment(f, index, 1);
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);
}

/// Pushes the uppercase hex digit of the nibble (`0`-`15`) in `nibble`.
fn push_hex_digit(f: &mut Function, nibble: u32) {
    f.instruction(&Instruction::LocalGet(nibble));
    f.instruction(&Instruction::I32Const(i32::from(b'0')));
    f.instruction(&Instruction::I32Const(i32::from(b'A') - 10));
    f.instruction(&Instruction::LocalGet(nibble));
    f.instruction(&Instruction::I32Const(10));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::Select);
    f.instruction(&Instruction::I32Add);
}

/// Pushes whether the byte in `byte` equals `expected`.
fn push_byte_is(f: &mut Function, byte: u32, expected: u8) {
    f.instruction(&Instruction::LocalGet(byte));
    f.instruction(&Instruction::I32Const(i32::from(expected)));
    f.instruction(&Instruction::I32Eq);
}

/// Pushes whether the byte in `byte` is an ASCII hex digit, in either case.
fn push_is_hex_digit(f: &mut Function, byte: u32) {
    push_in_range(f, byte, i32::from(b'0'), 10);
    // Setting bit 5 maps exactly `A`-`F` and `a`-`f` onto `a`-`f`.
    f.instruction(&Instruction::LocalGet(byte));
    f.instruction(&Instruction::I32Const(0x20));
    f.instruction(&Instruction::I32Or);
    f.instruction(&Instruction::I32Const(i32::from(b'a')));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(6));
    f.instruction(&Instruction::I32LtU);
    f.instruction(&Instruction::I32Or);
}

/// Pushes whether the byte in `byte` is RFC 3986 unreserved: an ASCII letter
/// or digit, `-`, `.`, `_`, or `~`.
fn push_is_unreserved(f: &mut Function, byte: u32) {
    // Setting bit 5 maps exactly `A`-`Z` and `a`-`z` onto `a`-`z`.
    f.instruction(&Instruction::LocalGet(byte));
    f.instruction(&Instruction::I32Const(0x20));
    f.instruction(&Instruction::I32Or);
    f.instruction(&Instruction::I32Const(i32::from(b'a')));
    f.instruction(&Instruction::I32Sub);
    f.instruction(&Instruction::I32Const(26));
    f.instruction(&Instruction::I32LtU);
    push_in_range(f, byte, i32::from(b'0'), 10);
    f.instruction(&Instruction::I32Or);
    for punctuation in *b"-._~" {
        push_byte_is(f, byte, punctuation);
        f.instruction(&Instruction::I32Or);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_index_the_refusals_in_check_order() {
        for (index, error) in REFUSALS.into_iter().enumerate() {
            assert_eq!(reason_code(error), i32::try_from(index).unwrap());
        }
        // `PathSegmentError` orders its variants in check order too.
        assert!(REFUSALS.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
