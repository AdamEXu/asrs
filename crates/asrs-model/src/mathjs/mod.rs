//! The part of mathjs 11.3.0 that upstream's `src/unitConversion.ts` reaches through
//! `evaluate(EXTRA_FACTOR.value)`: the whole expression parser, and evaluation over numbers,
//! booleans, strings, null, undefined, functions and result sets with the operator functions,
//! the named functions in [`FUNCTIONS`] and the constants in [`CONSTANTS`]. A Complex is
//! produced only as a whole result. Matrices, ranges, objects, accessors, indexes, units,
//! BigNumber, Fraction and every other mathjs name are not ported: evaluating one returns a
//! [`MathError`] named [`UNSUPPORTED`], which the dialog treats like any other failure.

pub mod node;

use asrs_js::string::JsString;

pub use node::{Constant, Node, NodeKind};

/// The `name` of the error returned for anything outside the ported subset; mathjs never throws
/// an error with this name.
pub const UNSUPPORTED: &str = "asrs.Unsupported";

/// Functions reachable through the parser's operators.
pub const OPERATOR_FUNCTIONS: &[&str] = &[
    "add",
    "subtract",
    "multiply",
    "divide",
    "pow",
    "mod",
    "unaryMinus",
    "unaryPlus",
    "factorial",
    "ctranspose",
    "bitAnd",
    "bitOr",
    "bitXor",
    "bitNot",
    "leftShift",
    "rightArithShift",
    "rightLogShift",
    "and",
    "or",
    "xor",
    "not",
    "equal",
    "unequal",
    "smaller",
    "larger",
    "smallerEq",
    "largerEq",
    "dotMultiply",
    "dotDivide",
    "dotPow",
];

/// Functions callable by name besides [`OPERATOR_FUNCTIONS`].
pub const FUNCTIONS: &[&str] = &[
    "abs", "sqrt", "cbrt", "exp", "log", "log10", "log2", "round", "floor", "ceil", "fix", "sign",
    "min", "max", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "number", "square", "cube",
    "gamma",
];

/// Typed functions the subset functions dispatch to through `any` signatures; their names appear
/// in argument errors.
pub const HELPER_FUNCTIONS: &[&str] = &["addScalar", "multiplyScalar", "equalScalar", "conj"];

pub const CONSTANTS: &[&str] = &[
    "pi", "PI", "e", "E", "tau", "phi", "LN2", "LN10", "LOG2E", "LOG10E", "SQRT1_2", "SQRT2", "i",
    "version",
];

#[derive(Clone, Debug, PartialEq)]
pub struct MathError {
    /// The JS error's `name`: `SyntaxError`, `TypeError`, `Error`, or [`UNSUPPORTED`].
    pub name: &'static str,
    pub message: JsString,
}

/// A function value: a typed function from the namespace or one an expression assigned.
#[derive(Clone, Debug)]
pub struct Function {
    /// The JS function's `name`.
    pub name: JsString,
}

#[derive(Clone, Debug)]
pub enum Value {
    Number(f64),
    Boolean(bool),
    String(JsString),
    Null,
    Undefined,
    Function(Function),
    ResultSet(Vec<Value>),
    Complex { re: f64, im: f64 },
}

impl Value {
    /// mathjs's `typeOf` for the values only a whole result can be.
    pub fn other_type_of(&self) -> Option<&'static str> {
        match self {
            Value::Complex { .. } => Some("Complex"),
            _ => None,
        }
    }
}

/// One typed-function signature as `fn._typedFunctionData.signatures` holds it, in dispatch
/// order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Signature {
    pub params: &'static [Param],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub types: &'static [ParamType],
    pub rest: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamType {
    pub name: &'static str,
    /// `(from, to)` of the typed conversion this type stands for.
    pub conversion: Option<(&'static str, &'static str)>,
}

/// `parse(expr)`.
pub fn parse(expr: &JsString) -> Result<Node, MathError> {
    let _ = expr;
    todo!("D08: port mathjs parse")
}

/// `evaluate(expr)` with a fresh scope.
pub fn evaluate(expr: &JsString) -> Result<Value, MathError> {
    let _ = expr;
    todo!("D08: port mathjs evaluate")
}

/// `math[name](...args)` for a name in [`OPERATOR_FUNCTIONS`], [`FUNCTIONS`] or
/// [`HELPER_FUNCTIONS`].
pub fn call(name: &str, args: &[Value]) -> Result<Value, MathError> {
    let _ = (name, args);
    todo!("D08: port the subset functions")
}

/// `format(value, { truncate: 78 })`, the text FunctionNode puts in "is not a function" errors.
pub fn format(value: &Value) -> JsString {
    let _ = value;
    todo!("D08: port mathjs format")
}

/// The typed function behind `name` (any name [`call`] takes): its JS `name` and signatures.
pub fn signatures(name: &str) -> Option<(&'static str, &'static [Signature])> {
    let _ = name;
    todo!("D08: typed-function signature tables")
}
