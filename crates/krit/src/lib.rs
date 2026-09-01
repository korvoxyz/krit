mod analysis;
mod ast;
mod builtin;
mod core;
mod diagnostic;
mod evaluator;
mod formatter;
mod lexer;
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
pub use evaluator::{BuiltinFunction, FunctionValue, Value, execute};
pub use formatter::{FORMAT_LINE_WIDTH, format_source};
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
