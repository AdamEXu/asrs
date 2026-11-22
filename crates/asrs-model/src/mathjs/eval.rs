//! `node.compile().evaluate(scope)`: a compile pass mirroring every `_compile`'s own checks
//! (so compile-time errors fire before any evaluation, even in dead branches), then a tree
//! walk mirroring each compiled `evalXxx(scope, args)` closure.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use asrs_js::string::JsString;

use super::node::{Constant, Node, NodeKind};
use super::stack;
use super::{
    FUNCTIONS, Function, HELPER_FUNCTIONS, MathError, OPERATOR_FUNCTIONS, UNSUPPORTED, Value,
};

/// The body of a `FunctionAssignmentNode`'s typed function.
#[derive(Debug)]
struct FunctionDef {
    /// `this.params`; every entry is an `any` signature parameter.
    params: Vec<JsString>,
    /// `this.types`, the signature `join(types, ',')`.
    types: Vec<JsString>,
    expr: Node,
    /// The `scope` captured by the compiled signature closure.
    scope: Scope,
    /// The `args` object the signature's `childArgs` prototypes.
    args: Rc<Args>,
}

/// A scope `Map`.
type Scope = Rc<RefCell<HashMap<JsString, Value>>>;

thread_local! {
    /// The functions the running `evaluate` defined, which [`Function::def`] indexes.
    static FUNCTIONS_DEFINED: RefCell<Vec<Rc<FunctionDef>>> = const { RefCell::new(Vec::new()) };
}

/// The `args` objects passed to compiled evaluators; `childArgs = Object.create(args)`.
#[derive(Debug)]
struct Args {
    vars: HashMap<JsString, Value>,
    parent: Option<Rc<Args>>,
}

impl Args {
    fn root() -> Rc<Args> {
        Rc::new(Args {
            vars: HashMap::new(),
            parent: None,
        })
    }

    fn child(vars: HashMap<JsString, Value>, parent: &Rc<Args>) -> Rc<Args> {
        Rc::new(Args {
            vars,
            parent: Some(Rc::clone(parent)),
        })
    }

    fn get(&self, name: &JsString) -> Option<&Value> {
        self.vars
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.get(name)))
    }
}

fn text(s: &str) -> JsString {
    JsString::from(s)
}

fn cat(parts: &[JsString]) -> JsString {
    let mut out = JsString::new();
    for part in parts {
        out.push_str(part);
    }
    out
}

fn error(message: JsString) -> MathError {
    MathError {
        name: "Error",
        message,
    }
}

fn type_error(message: JsString) -> MathError {
    MathError {
        name: "TypeError",
        message,
    }
}

fn unsupported(what: &str) -> MathError {
    MathError {
        name: UNSUPPORTED,
        message: text(&format!("{what} is not in the asrs mathjs subset")),
    }
}

fn is_subset_function(name: &str) -> bool {
    OPERATOR_FUNCTIONS.contains(&name)
        || FUNCTIONS.contains(&name)
        || HELPER_FUNCTIONS.contains(&name)
}

/// `Object.prototype`'s own property names: the names `in math` by inheritance alone.
const OBJECT_PROTO_KEYS: &[&str] = &[
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// `Function.prototype`'s own properties not already covered (for `isSafeProperty` /
/// `isSafeMethod`); `length`, `name`, `prototype` are treated by the callers' own checks.
const FUNCTION_PROTO_KEYS: &[&str] = &["apply", "bind", "call", "caller", "arguments"];

/// `customs.js`' `safeNativeMethods`.
const SAFE_NATIVE_METHODS: &[&str] = &["toString", "valueOf", "toLocaleString"];

/// Properties of the math namespace that resolve to a ported value.
fn math_constant(name: &str) -> Option<Value> {
    Some(match name {
        "pi" | "PI" => Value::Number(std::f64::consts::PI),
        "e" | "E" => Value::Number(std::f64::consts::E),
        "tau" => Value::Number(std::f64::consts::TAU),
        "phi" => Value::Number(std::f64::consts::GOLDEN_RATIO),
        "LN2" => Value::Number(std::f64::consts::LN_2),
        "LN10" => Value::Number(std::f64::consts::LN_10),
        "LOG2E" => Value::Number(std::f64::consts::LOG2_E),
        "LOG10E" => Value::Number(std::f64::consts::LOG10_E),
        "SQRT1_2" => Value::Number(std::f64::consts::FRAC_1_SQRT_2),
        "SQRT2" => Value::Number(std::f64::consts::SQRT_2),
        "i" => Value::Complex { re: 0.0, im: 1.0 },
        "version" => Value::String(JsString::from("11.3.0")),
        "NaN" => Value::Number(f64::NAN),
        "Infinity" => Value::Number(f64::INFINITY),
        "true" => Value::Boolean(true),
        "false" => Value::Boolean(false),
        "null" => Value::Null,
        _ => return None,
    })
}

/// `name in mathWithTransform`: the subset's own entries, plus `config`, plus
/// `Object.prototype`'s inherited properties. Other mathjs names are treated as
/// not present, making them undefined symbols.
fn in_math(name: &JsString) -> bool {
    let Some(name) = name.as_string() else {
        return false;
    };
    is_subset_function(&name)
        || math_constant(&name).is_some()
        || name == "config"
        || OBJECT_PROTO_KEYS.contains(&name.as_str())
}

/// The `getSafeProperty(mathWithTransform, name)` errors that `_compile` performs for a
/// `FunctionNode`'s symbol callee (the `isRaw` probe) and that symbol evaluation performs.
fn check_math_property(name: &JsString) -> Result<(), MathError> {
    let Some(n) = name.as_string() else {
        return Ok(());
    };
    if !OBJECT_PROTO_KEYS.contains(&n.as_str()) {
        return Ok(());
    }
    Err(error(if SAFE_NATIVE_METHODS.contains(&n.as_str()) {
        cat(&[
            text("Cannot access method \""),
            name.clone(),
            text("\" as a property"),
        ])
    } else {
        cat(&[text("No access to property \""), name.clone(), text("\"")])
    }))
}

/// `getSafeProperty(mathWithTransform, name)` at evaluation time.
fn math_property(name: &JsString) -> Result<Value, MathError> {
    if let Some(n) = name.as_string() {
        if let Some(value) = math_constant(&n) {
            return Ok(value);
        }
        if is_subset_function(&n) {
            return Ok(Value::Function(Function {
                name: name.clone(),
                def: None,
            }));
        }
        if n == "config" {
            return Err(unsupported("the mathjs config object"));
        }
    }
    check_math_property(name)?;
    Err(error(cat(&[
        text("No access to property \""),
        name.clone(),
        text("\""),
    ])))
}

/// mathjs's `typeOf` for the values `evaluate` produces.
fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Number(_) => "number",
        Value::Boolean(_) => "boolean",
        Value::String(_) => "string",
        Value::Null => "null",
        Value::Undefined => "undefined",
        Value::Function(_) => "function",
        Value::ResultSet(_) => "ResultSet",
        Value::Complex { .. } => "Complex",
    }
}

/// JS `!!x`.
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Number(x) => *x != 0.0 && !x.is_nan(),
        Value::Boolean(b) => *b,
        Value::String(s) => !s.units().is_empty(),
        Value::Null | Value::Undefined => false,
        _ => true,
    }
}

/// ConditionalNode's `testCondition`.
fn test_condition(condition: &Value) -> Result<bool, MathError> {
    match condition {
        Value::Number(_) | Value::Boolean(_) | Value::String(_) => Ok(js_truthy(condition)),
        // `!!(condition.re || condition.im)`; `||` treats NaN as falsy
        Value::Complex { re, im } => {
            Ok(js_truthy(&Value::Number(*re)) || js_truthy(&Value::Number(*im)))
        }
        Value::Null | Value::Undefined => Ok(false),
        other => Err(type_error(cat(&[
            text("Unsupported type of condition \""),
            text(type_of(other)),
            text("\""),
        ]))),
    }
}

/// `isSafeProperty(this.properties, key)` from `ObjectNode._compile`, applied to key strings:
/// `Object.prototype` and `Function.prototype` names are unsafe except `length` and `name`.
fn unsafe_property_key(key: &JsString) -> bool {
    let Some(key) = key.as_string() else {
        return false;
    };
    if matches!(key.as_str(), "length" | "name") {
        return false;
    }
    OBJECT_PROTO_KEYS.contains(&key.as_str()) || FUNCTION_PROTO_KEYS.contains(&key.as_str())
}

/// Every `_compile`'s checks, in the order `_compile` runs them.
fn compile(node: &Node, arg_names: &HashSet<JsString>) -> Result<(), MathError> {
    stack::check()?;
    match &node.kind {
        NodeKind::Constant(_) | NodeKind::Symbol { .. } => Ok(()),
        NodeKind::Parenthesis { content } => compile(content, arg_names),
        NodeKind::Operator { fn_name, args, .. } => {
            if !is_subset_function(fn_name) {
                return Err(error(text(&format!(
                    "Function {fn_name} missing in provided namespace \"math\""
                ))));
            }
            for arg in args {
                compile(arg, arg_names)?;
            }
            Ok(())
        }
        NodeKind::Function { callee, args } => {
            for arg in args {
                compile(arg, arg_names)?;
            }
            match &callee.kind {
                NodeKind::Symbol { name } => {
                    if !arg_names.contains(name) && in_math(name) {
                        check_math_property(name)?;
                    }
                    Ok(())
                }
                _ => compile(callee, arg_names),
            }
        }
        NodeKind::Accessor { object, index } => {
            compile(object, arg_names)?;
            compile(index, arg_names)
        }
        NodeKind::Index { dimensions, .. } => {
            for dimension in dimensions {
                compile(dimension, arg_names)?;
            }
            Ok(())
        }
        NodeKind::Array { items } => {
            for item in items {
                compile(item, arg_names)?;
            }
            Ok(())
        }
        NodeKind::Object { properties } => {
            for (key, value) in properties {
                if unsafe_property_key(key) {
                    return Err(error(cat(&[
                        text("No access to property \""),
                        key.clone(),
                        text("\""),
                    ])));
                }
                compile(value, arg_names)?;
            }
            Ok(())
        }
        NodeKind::Assignment {
            object,
            index,
            value,
        } => {
            compile(object, arg_names)?;
            if let Some(index) = index {
                compile(index, arg_names)?;
            }
            compile(value, arg_names)?;
            if index.is_none() && !matches!(object.kind, NodeKind::Symbol { .. }) {
                return Err(type_error(text("SymbolNode expected as object")));
            }
            Ok(())
        }
        NodeKind::FunctionAssignment { params, expr, .. } => {
            let mut child = arg_names.clone();
            for param in params {
                child.insert(param.clone());
            }
            compile(expr, &child)
        }
        NodeKind::Conditional {
            condition,
            true_expr,
            false_expr,
        } => {
            compile(condition, arg_names)?;
            compile(true_expr, arg_names)?;
            compile(false_expr, arg_names)
        }
        NodeKind::Range { start, end, step } => {
            compile(start, arg_names)?;
            compile(end, arg_names)?;
            if let Some(step) = step {
                compile(step, arg_names)?;
            }
            Ok(())
        }
        NodeKind::Relational { params, .. } => {
            for param in params {
                compile(param, arg_names)?;
            }
            Ok(())
        }
        NodeKind::Block { blocks } => {
            for (block, _) in blocks {
                compile(block, arg_names)?;
            }
            Ok(())
        }
    }
}

fn constant_value(constant: &Constant) -> Value {
    match constant {
        Constant::Number(x) => Value::Number(*x),
        Constant::String(s) => Value::String(s.clone()),
        Constant::Boolean(b) => Value::Boolean(*b),
        Constant::Null => Value::Null,
        Constant::Undefined => Value::Undefined,
    }
}

fn eval_symbol(name: &JsString, scope: &Scope, args: &Rc<Args>) -> Result<Value, MathError> {
    if let Some(value) = args.get(name) {
        // the `argNames[name] === true` branch; args and argNames track each other
        return Ok(value.clone());
    }
    if in_math(name) {
        if let Some(value) = scope.borrow().get(name) {
            return Ok(value.clone());
        }
        return math_property(name);
    }
    if let Some(value) = scope.borrow().get(name) {
        return Ok(value.clone());
    }
    Err(error(cat(&[text("Undefined symbol "), name.clone()])))
}

fn eval_args(arg_nodes: &[Node], scope: &Scope, args: &Rc<Args>) -> Result<Vec<Value>, MathError> {
    arg_nodes
        .iter()
        .map(|arg| eval_node(arg, scope, args))
        .collect()
}

/// Calls a function value: `math.f(...)` for `def: None`, a user function otherwise.
fn call_function(function: &Function, values: &[Value]) -> Result<Value, MathError> {
    match function.def {
        None => super::call(&function.name.as_string().unwrap_or_default(), values),
        Some(index) => {
            let def = FUNCTIONS_DEFINED.with_borrow(|defs| Rc::clone(&defs[index]));
            let actual = values.len();
            let expected = def.params.len();
            if actual < expected {
                // `mergeExpectedParams` of a single `any`-type signature at `index`
                let expected_type = def
                    .types
                    .get(actual)
                    .and_then(|t| t.as_string())
                    .unwrap_or_else(|| "any".to_owned());
                return Err(type_error(cat(&[
                    text("Too few arguments in function "),
                    function.name.clone(),
                    text(" (expected: "),
                    text(&expected_type),
                    text(", index: "),
                    text(&format!("{actual}")),
                    text(")"),
                ])));
            }
            if actual > expected {
                return Err(type_error(cat(&[
                    text("Too many arguments in function "),
                    function.name.clone(),
                    text(" (expected: "),
                    text(&format!("{expected}")),
                    text(", actual: "),
                    text(&format!("{actual}")),
                    text(")"),
                ])));
            }
            let mut vars = HashMap::with_capacity(expected);
            for (param, value) in def.params.iter().zip(values) {
                vars.insert(param.clone(), value.clone());
            }
            let child_args = Args::child(vars, &def.args);
            eval_node(&def.expr, &def.scope, &child_args)
        }
    }
}

fn eval_assignment(
    object: &Node,
    index: Option<&Node>,
    value: &Node,
    scope: &Scope,
    args: &Rc<Args>,
) -> Result<Value, MathError> {
    if index.is_some() {
        return Err(unsupported("an indexed assignment"));
    }
    let NodeKind::Symbol { name } = &object.kind else {
        return Err(type_error(text("SymbolNode expected as object")));
    };
    let value = eval_node(value, scope, args)?;
    scope.borrow_mut().insert(name.clone(), value.clone());
    Ok(value)
}

fn eval_call(
    callee: &Node,
    arg_nodes: &[Node],
    scope: &Scope,
    args: &Rc<Args>,
) -> Result<Value, MathError> {
    if let NodeKind::Symbol { name } = &callee.kind {
        if let Some(f) = args.get(name).cloned() {
            let Value::Function(function) = f else {
                return Err(type_error(cat(&[
                    text("Argument '"),
                    name.clone(),
                    text("' was not a function; received: "),
                    super::format(&f),
                ])));
            };
            let values = eval_args(arg_nodes, scope, args)?;
            return call_function(&function, &values);
        }
        // `resolveFn(scope)`, before evaluating the arguments
        let value = if let Some(value) = scope.borrow().get(name) {
            value.clone()
        } else if in_math(name) {
            math_property(name)?
        } else {
            return Err(error(cat(&[text("Undefined function "), name.clone()])));
        };
        let Value::Function(function) = value else {
            return Err(type_error(cat(&[
                text("'"),
                name.clone(),
                text("' is not a function; its value is:\n  "),
                super::format(&value),
            ])));
        };
        let values = eval_args(arg_nodes, scope, args)?;
        call_function(&function, &values)
    } else {
        Err(unsupported("a callee that is not a symbol"))
    }
}

fn eval_node(node: &Node, scope: &Scope, args: &Rc<Args>) -> Result<Value, MathError> {
    stack::check()?;
    match &node.kind {
        NodeKind::Constant(constant) => Ok(constant_value(constant)),
        NodeKind::Symbol { name } => eval_symbol(name, scope, args),
        NodeKind::Parenthesis { content } => eval_node(content, scope, args),
        NodeKind::Operator {
            fn_name, args: ops, ..
        } => {
            let values = eval_args(ops, scope, args)?;
            super::call(fn_name, &values)
        }
        NodeKind::Function {
            callee,
            args: arg_nodes,
        } => eval_call(callee, arg_nodes, scope, args),
        NodeKind::Accessor { .. } | NodeKind::Index { .. } => Err(unsupported("an accessor")),
        NodeKind::Array { .. } => Err(unsupported("a matrix")),
        NodeKind::Object { .. } => Err(unsupported("an object")),
        NodeKind::Assignment {
            object,
            index,
            value,
        } => eval_assignment(object, index.as_deref(), value, scope, args),
        NodeKind::FunctionAssignment {
            name,
            params,
            types,
            expr,
        } => {
            let def = Rc::new(FunctionDef {
                params: params.clone(),
                types: types.clone(),
                expr: (**expr).clone(),
                scope: Rc::clone(scope),
                args: Rc::clone(args),
            });
            let index = FUNCTIONS_DEFINED.with_borrow_mut(|defs| {
                defs.push(def);
                defs.len() - 1
            });
            let function = Value::Function(Function {
                name: name.clone(),
                def: Some(index),
            });
            scope.borrow_mut().insert(name.clone(), function.clone());
            Ok(function)
        }
        NodeKind::Conditional {
            condition,
            true_expr,
            false_expr,
        } => {
            if test_condition(&eval_node(condition, scope, args)?)? {
                eval_node(true_expr, scope, args)
            } else {
                eval_node(false_expr, scope, args)
            }
        }
        NodeKind::Range { .. } => Err(unsupported("a range")),
        NodeKind::Relational {
            conditionals,
            params,
        } => {
            let mut rhs = eval_node(&params[0], scope, args)?;
            for (i, conditional) in conditionals.iter().enumerate() {
                let lhs = rhs;
                rhs = eval_node(&params[i + 1], scope, args)?;
                if !js_truthy(&super::call(conditional, &[lhs, rhs.clone()])?) {
                    return Ok(Value::Boolean(false));
                }
            }
            Ok(Value::Boolean(true))
        }
        NodeKind::Block { blocks } => {
            let mut results = Vec::new();
            for (block, visible) in blocks {
                let result = eval_node(block, scope, args)?;
                if *visible {
                    results.push(result);
                }
            }
            Ok(Value::ResultSet(results))
        }
    }
}

/// `evaluate(expr)` with a fresh `Map` scope.
pub fn evaluate(expr: &JsString) -> Result<Value, MathError> {
    let node = super::parse::parse(expr)?;
    compile(&node, &HashSet::new())?;
    let result = eval_node(&node, &Rc::new(RefCell::new(HashMap::new())), &Args::root());
    FUNCTIONS_DEFINED.with_borrow_mut(Vec::clear);
    result
}
