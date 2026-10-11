mod analysis;
mod ast;
mod builtin;
mod core;
mod diagnostic;
mod durable;
mod evaluator;
mod formatter;
mod lexer;
mod limits;
mod parser;
mod source;
mod token;

pub use analysis::{
    Analysis, BindingAnalysis, BlockAnalysis, CapabilityRequirement, Effect, EffectSet,
    ExpressionAnalysis, FunctionType, RecordType, RequirementSet, ResolvedName, SymbolAnalysis,
    SymbolId, SymbolKind, Type, analyze,
};
pub use ast::{
    BinaryOperator, Block, Expression, ExpressionKind, MatchKind, Parameter, Program, RecordField,
    RecordTypeField, Statement, StatementKind, TypeAnnotation, TypeKind, UnaryOperator,
    ValueLiteral, VariantArm, VariantFamily, VariantName,
};
pub use builtin::{Builtin, BuiltinCategory};
pub use core::{
    BindingId, BindingKind, BlockId, BlockParameter, CaptureArgument, CaptureId, ClosureId,
    CoreBinding, CoreBlock, CoreCapture, CoreEntrypoint, CoreFunction, CoreModule, CoreOperation,
    CoreParameter, EntrypointKind, FunctionId, FunctionSignature, IrError, MatchBindingId,
    OperationKind, ParameterId, RecordOperand, RecursiveBinding, ValueId, VariantArmBlock, lower,
};
pub use diagnostic::Diagnostic;
pub use durable::{DurableOperationFact, DurableOperationKind, durable_operations};
pub use evaluator::{
    BuiltinFunction, FunctionValue, ListValue, RecordValue, Value, VariantPayload, execute,
};
pub use formatter::{FORMAT_LINE_WIDTH, format_source};
pub use limits::{
    COMPILER_STACK_BYTES, EVALUATOR_STACK_BYTES, MAX_CALL_DEPTH, MAX_CORE_BLOCK_DEPTH,
    MAX_EVALUATION_DEPTH, MAX_NESTING_DEPTH, MAX_STRUCTURAL_DEPTH, MAX_VALUE_DEPTH, run_on_thread,
};
pub use parser::parse;
pub use source::{Position, Source, Span};

pub fn parse_source(source: &Source) -> Result<Program, Diagnostic> {
    let tokens = lexer::lex(source)?;
    parser::parse(tokens)
}

pub fn run_source(source: &Source, output: &mut dyn std::io::Write) -> Result<Value, Diagnostic> {
    let program = parse_source(source)?;
    execute(&program, output)
}
