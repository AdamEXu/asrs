//! `node.compile().evaluate(scope)`: a compile pass mirroring every `_compile`'s own checks
//! (so compile-time errors fire before any evaluation, even in dead branches), then a tree
//! walk mirroring each compiled `evalXxx(scope, args, context)` closure.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use asrs_js::string::JsString;

use super::node::{Constant, Node, NodeKind};
use super::{
    FUNCTIONS, Function, HELPER_FUNCTIONS, MathError, OPERATOR_FUNCTIONS, UNSUPPORTED, Value,
};

/// The body of a `FunctionAssignmentNode`'s typed function.
#[derive(Debug)]
pub struct FunctionDef {
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

fn js_eq(s: &JsString, lit: &str) -> bool {
    s.as_string().as_deref() == Some(lit)
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

/// A unit's `PREFIXES` group.
#[derive(Clone, Copy)]
enum Prefix {
    None,
    Short,
    Long,
    ShortLong,
    BinaryShort,
    BinaryLong,
    Btu,
}

const PREFIX_SHORT: &[&str] = &[
    "da", "h", "k", "M", "G", "T", "P", "E", "Z", "Y", "d", "c", "m", "u", "n", "p", "f", "a", "z",
    "y",
];
const PREFIX_LONG: &[&str] = &[
    "deca", "hecto", "kilo", "mega", "giga", "tera", "peta", "exa", "zetta", "yotta", "deci",
    "centi", "milli", "micro", "nano", "pico", "femto", "atto", "zepto", "yocto",
];
const PREFIX_BINARY_SHORT: &[&str] = &[
    "k", "M", "G", "T", "P", "E", "Z", "Y", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "Zi", "Yi",
];
const PREFIX_BINARY_LONG: &[&str] = &[
    "kilo", "mega", "giga", "tera", "peta", "exa", "zetta", "yotta", "kibi", "mebi", "gibi",
    "tebi", "pebi", "exbi", "zebi", "yobi",
];
const PREFIX_BTU: &[&str] = &["MM"];

/// `UNITS` in insertion order, aliases appended (an alias copies its base's prefixes).
const UNIT_NAMES: &[(&str, Prefix)] = &[
    ("meter", Prefix::Long),
    ("inch", Prefix::None),
    ("foot", Prefix::None),
    ("yard", Prefix::None),
    ("mile", Prefix::None),
    ("link", Prefix::None),
    ("rod", Prefix::None),
    ("chain", Prefix::None),
    ("angstrom", Prefix::None),
    ("m", Prefix::Short),
    ("in", Prefix::None),
    ("ft", Prefix::None),
    ("yd", Prefix::None),
    ("mi", Prefix::None),
    ("li", Prefix::None),
    ("rd", Prefix::None),
    ("ch", Prefix::None),
    ("mil", Prefix::None),
    ("m2", Prefix::Short),
    ("sqin", Prefix::None),
    ("sqft", Prefix::None),
    ("sqyd", Prefix::None),
    ("sqmi", Prefix::None),
    ("sqrd", Prefix::None),
    ("sqch", Prefix::None),
    ("sqmil", Prefix::None),
    ("acre", Prefix::None),
    ("hectare", Prefix::None),
    ("m3", Prefix::Short),
    ("L", Prefix::Short),
    ("l", Prefix::Short),
    ("litre", Prefix::Long),
    ("cuin", Prefix::None),
    ("cuft", Prefix::None),
    ("cuyd", Prefix::None),
    ("teaspoon", Prefix::None),
    ("tablespoon", Prefix::None),
    ("drop", Prefix::None),
    ("gtt", Prefix::None),
    ("minim", Prefix::None),
    ("fluiddram", Prefix::None),
    ("fluidounce", Prefix::None),
    ("gill", Prefix::None),
    ("cc", Prefix::None),
    ("cup", Prefix::None),
    ("pint", Prefix::None),
    ("quart", Prefix::None),
    ("gallon", Prefix::None),
    ("beerbarrel", Prefix::None),
    ("oilbarrel", Prefix::None),
    ("hogshead", Prefix::None),
    ("fldr", Prefix::None),
    ("floz", Prefix::None),
    ("gi", Prefix::None),
    ("cp", Prefix::None),
    ("pt", Prefix::None),
    ("qt", Prefix::None),
    ("gal", Prefix::None),
    ("bbl", Prefix::None),
    ("obl", Prefix::None),
    ("g", Prefix::Short),
    ("gram", Prefix::Long),
    ("ton", Prefix::Short),
    ("t", Prefix::Short),
    ("tonne", Prefix::Long),
    ("grain", Prefix::None),
    ("dram", Prefix::None),
    ("ounce", Prefix::None),
    ("poundmass", Prefix::None),
    ("hundredweight", Prefix::None),
    ("stick", Prefix::None),
    ("stone", Prefix::None),
    ("gr", Prefix::None),
    ("dr", Prefix::None),
    ("oz", Prefix::None),
    ("lbm", Prefix::None),
    ("cwt", Prefix::None),
    ("s", Prefix::Short),
    ("min", Prefix::None),
    ("h", Prefix::None),
    ("second", Prefix::Long),
    ("sec", Prefix::Long),
    ("minute", Prefix::None),
    ("hour", Prefix::None),
    ("day", Prefix::None),
    ("week", Prefix::None),
    ("month", Prefix::None),
    ("year", Prefix::None),
    ("decade", Prefix::None),
    ("century", Prefix::None),
    ("millennium", Prefix::None),
    ("hertz", Prefix::Long),
    ("Hz", Prefix::Short),
    ("rad", Prefix::Short),
    ("radian", Prefix::Long),
    ("deg", Prefix::Short),
    ("degree", Prefix::Long),
    ("grad", Prefix::Short),
    ("gradian", Prefix::Long),
    ("cycle", Prefix::None),
    ("arcsec", Prefix::None),
    ("arcmin", Prefix::None),
    ("A", Prefix::Short),
    ("ampere", Prefix::Long),
    ("K", Prefix::None),
    ("degC", Prefix::None),
    ("degF", Prefix::None),
    ("degR", Prefix::None),
    ("kelvin", Prefix::None),
    ("celsius", Prefix::None),
    ("fahrenheit", Prefix::None),
    ("rankine", Prefix::None),
    ("mol", Prefix::Short),
    ("mole", Prefix::Long),
    ("cd", Prefix::Short),
    ("candela", Prefix::Long),
    ("N", Prefix::Short),
    ("newton", Prefix::Long),
    ("dyn", Prefix::Short),
    ("dyne", Prefix::Long),
    ("lbf", Prefix::None),
    ("poundforce", Prefix::None),
    ("kip", Prefix::Long),
    ("kilogramforce", Prefix::None),
    ("J", Prefix::Short),
    ("joule", Prefix::Short),
    ("erg", Prefix::None),
    ("Wh", Prefix::Short),
    ("BTU", Prefix::Btu),
    ("eV", Prefix::Short),
    ("electronvolt", Prefix::Long),
    ("W", Prefix::Short),
    ("watt", Prefix::Long),
    ("hp", Prefix::None),
    ("VAR", Prefix::Short),
    ("VA", Prefix::Short),
    ("Pa", Prefix::Short),
    ("psi", Prefix::None),
    ("atm", Prefix::None),
    ("bar", Prefix::ShortLong),
    ("torr", Prefix::None),
    ("mmHg", Prefix::None),
    ("mmH2O", Prefix::None),
    ("cmH2O", Prefix::None),
    ("coulomb", Prefix::Long),
    ("C", Prefix::Short),
    ("farad", Prefix::Long),
    ("F", Prefix::Short),
    ("volt", Prefix::Long),
    ("V", Prefix::Short),
    ("ohm", Prefix::ShortLong),
    ("Ω", Prefix::Short),
    ("henry", Prefix::Long),
    ("H", Prefix::Short),
    ("siemens", Prefix::Long),
    ("S", Prefix::Short),
    ("weber", Prefix::Long),
    ("Wb", Prefix::Short),
    ("tesla", Prefix::Long),
    ("T", Prefix::Short),
    ("b", Prefix::BinaryShort),
    ("bits", Prefix::BinaryLong),
    ("B", Prefix::BinaryShort),
    ("bytes", Prefix::BinaryLong),
    ("meters", Prefix::Long),
    ("inches", Prefix::None),
    ("feet", Prefix::None),
    ("yards", Prefix::None),
    ("miles", Prefix::None),
    ("links", Prefix::None),
    ("rods", Prefix::None),
    ("chains", Prefix::None),
    ("angstroms", Prefix::None),
    ("lt", Prefix::Short),
    ("litres", Prefix::Long),
    ("liter", Prefix::Long),
    ("liters", Prefix::Long),
    ("teaspoons", Prefix::None),
    ("tablespoons", Prefix::None),
    ("minims", Prefix::None),
    ("fluiddrams", Prefix::None),
    ("fluidounces", Prefix::None),
    ("gills", Prefix::None),
    ("cups", Prefix::None),
    ("pints", Prefix::None),
    ("quarts", Prefix::None),
    ("gallons", Prefix::None),
    ("beerbarrels", Prefix::None),
    ("oilbarrels", Prefix::None),
    ("hogsheads", Prefix::None),
    ("gtts", Prefix::None),
    ("grams", Prefix::Long),
    ("tons", Prefix::Short),
    ("tonnes", Prefix::Long),
    ("grains", Prefix::None),
    ("drams", Prefix::None),
    ("ounces", Prefix::None),
    ("poundmasses", Prefix::None),
    ("hundredweights", Prefix::None),
    ("sticks", Prefix::None),
    ("lb", Prefix::None),
    ("lbs", Prefix::None),
    ("kips", Prefix::Long),
    ("kgf", Prefix::None),
    ("acres", Prefix::None),
    ("hectares", Prefix::None),
    ("sqfeet", Prefix::None),
    ("sqyard", Prefix::None),
    ("sqmile", Prefix::None),
    ("sqmiles", Prefix::None),
    ("mmhg", Prefix::None),
    ("mmh2o", Prefix::None),
    ("cmh2o", Prefix::None),
    ("seconds", Prefix::Long),
    ("secs", Prefix::Long),
    ("minutes", Prefix::None),
    ("mins", Prefix::None),
    ("hours", Prefix::None),
    ("hr", Prefix::None),
    ("hrs", Prefix::None),
    ("days", Prefix::None),
    ("weeks", Prefix::None),
    ("months", Prefix::None),
    ("years", Prefix::None),
    ("decades", Prefix::None),
    ("centuries", Prefix::None),
    ("millennia", Prefix::None),
    ("hertz", Prefix::Long),
    ("radians", Prefix::Long),
    ("degrees", Prefix::Long),
    ("gradians", Prefix::Long),
    ("cycles", Prefix::None),
    ("arcsecond", Prefix::None),
    ("arcseconds", Prefix::None),
    ("arcminute", Prefix::None),
    ("arcminutes", Prefix::None),
    ("BTUs", Prefix::Btu),
    ("watts", Prefix::Long),
    ("joules", Prefix::Short),
    ("amperes", Prefix::Long),
    ("coulombs", Prefix::Long),
    ("volts", Prefix::Long),
    ("ohms", Prefix::ShortLong),
    ("farads", Prefix::Long),
    ("webers", Prefix::Long),
    ("teslas", Prefix::Long),
    ("electronvolts", Prefix::Long),
    ("moles", Prefix::Long),
    ("bit", Prefix::BinaryLong),
    ("byte", Prefix::BinaryLong),
];

/// `Unit.isValuelessUnit(name)` / `_findUnit`: an exact unit or alias name, or a prefix
/// followed by a unit suffix.
fn is_valueless_unit(name: &JsString) -> bool {
    let Some(name) = name.as_string() else {
        return false;
    };
    if UNIT_NAMES.iter().any(|(unit, _)| *unit == name) {
        return true;
    }
    UNIT_NAMES.iter().any(|(unit, group)| {
        if let Some(prefix) = name.strip_suffix(unit) {
            !prefix.is_empty() && prefix_in_group(prefix, *group)
        } else {
            false
        }
    })
}

fn prefix_in_group(prefix: &str, group: Prefix) -> bool {
    match group {
        Prefix::None => false,
        Prefix::Short => PREFIX_SHORT.contains(&prefix),
        Prefix::Long => PREFIX_LONG.contains(&prefix),
        Prefix::ShortLong => PREFIX_SHORT.contains(&prefix) || PREFIX_LONG.contains(&prefix),
        Prefix::BinaryShort => PREFIX_BINARY_SHORT.contains(&prefix),
        Prefix::BinaryLong => PREFIX_BINARY_LONG.contains(&prefix),
        Prefix::Btu => PREFIX_BTU.contains(&prefix),
    }
}

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

/// Whether `value[prop]` is a function under JS's primitive boxing, for `isSafeMethod`.
fn function_valued_property(value: &Value, prop: &str) -> bool {
    const OBJECT_METHODS: &[&str] = &[
        "__defineGetter__",
        "__defineSetter__",
        "__lookupGetter__",
        "__lookupSetter__",
        "constructor",
        "hasOwnProperty",
        "isPrototypeOf",
        "propertyIsEnumerable",
        "toLocaleString",
        "toString",
        "valueOf",
    ];
    const NUMBER_METHODS: &[&str] = &["toExponential", "toFixed", "toPrecision"];
    const STRING_METHODS: &[&str] = &[
        "anchor",
        "at",
        "big",
        "blink",
        "bold",
        "charAt",
        "charCodeAt",
        "codePointAt",
        "concat",
        "endsWith",
        "fixed",
        "fontcolor",
        "fontsize",
        "includes",
        "indexOf",
        "isWellFormed",
        "italics",
        "lastIndexOf",
        "link",
        "localeCompare",
        "match",
        "matchAll",
        "normalize",
        "padEnd",
        "padStart",
        "repeat",
        "replace",
        "replaceAll",
        "search",
        "slice",
        "small",
        "split",
        "strike",
        "sub",
        "substr",
        "substring",
        "sup",
        "toLocaleLowerCase",
        "toLocaleUpperCase",
        "toLowerCase",
        "toUpperCase",
        "toWellFormed",
        "trim",
        "trimEnd",
        "trimLeft",
        "trimRight",
        "trimStart",
    ];
    const COMPLEX_METHODS: &[&str] = &[
        "abs",
        "acos",
        "acosh",
        "add",
        "arg",
        "asin",
        "asinh",
        "atan",
        "atanh",
        "ceil",
        "clone",
        "compare",
        "compareNatural",
        "conjugate",
        "cos",
        "cosh",
        "cot",
        "coth",
        "csc",
        "csch",
        "div",
        "equals",
        "exp",
        "floor",
        "format",
        "fromPolar",
        "im",
        "inverse",
        "log",
        "mul",
        "neg",
        "pow",
        "re",
        "round",
        "sec",
        "sech",
        "sign",
        "sin",
        "sinh",
        "sqrt",
        "sub",
        "tan",
        "tanh",
        "toJSON",
        "toPolar",
        "toVector",
    ];
    if OBJECT_METHODS.contains(&prop) {
        return true;
    }
    match value {
        Value::Number(_) => NUMBER_METHODS.contains(&prop),
        Value::String(_) => STRING_METHODS.contains(&prop),
        Value::Boolean(_) | Value::Null | Value::Undefined | Value::ResultSet(_) => false,
        Value::Function(_) => FUNCTION_PROTO_KEYS.contains(&prop),
        Value::Complex { .. } => COMPLEX_METHODS.contains(&prop),
    }
}

/// `isSafeMethod`, assuming `value[prop]` is already known to be a function.
fn safe_method(prop: &str) -> bool {
    if SAFE_NATIVE_METHODS.contains(&prop) {
        return true;
    }
    if OBJECT_PROTO_KEYS.contains(&prop) || FUNCTION_PROTO_KEYS.contains(&prop) {
        return false;
    }
    true
}

/// `IndexNode.isObjectProperty() ? Some(getObjectProperty()) : None`.
fn object_property(index: &Node) -> Option<&JsString> {
    if let NodeKind::Index { dimensions, .. } = &index.kind
        && let [
            Node {
                kind: NodeKind::Constant(Constant::String(prop)),
                ..
            },
        ] = dimensions.as_slice()
    {
        return Some(prop);
    }
    None
}

/// Whether the subtree holds a `SymbolNode('end')`, like `Node.filter` finds.
fn contains_end(node: &Node) -> bool {
    if matches!(&node.kind, NodeKind::Symbol { name } if js_eq(name, "end")) {
        return true;
    }
    let mut found = false;
    each_child(node, &mut |child| {
        if !found && contains_end(child) {
            found = true;
        }
    });
    found
}

/// `forEach(child)` for every node type.
fn each_child<F: FnMut(&Node)>(node: &Node, f: &mut F) {
    match &node.kind {
        NodeKind::Constant(_) | NodeKind::Symbol { .. } => {}
        NodeKind::Parenthesis { content } => f(content),
        NodeKind::Operator { args, .. } => {
            for arg in args {
                f(arg);
            }
        }
        NodeKind::Function { callee, args } => {
            f(callee);
            for arg in args {
                f(arg);
            }
        }
        NodeKind::Accessor { object, index } => {
            f(object);
            f(index);
        }
        NodeKind::Index { dimensions, .. } => {
            for dimension in dimensions {
                f(dimension);
            }
        }
        NodeKind::Array { items } => {
            for item in items {
                f(item);
            }
        }
        NodeKind::Object { properties } => {
            for (_, value) in properties {
                f(value);
            }
        }
        NodeKind::Assignment {
            object,
            index,
            value,
        } => {
            f(object);
            if let Some(index) = index {
                f(index);
            }
            f(value);
        }
        NodeKind::FunctionAssignment { expr, .. } => f(expr),
        NodeKind::Conditional {
            condition,
            true_expr,
            false_expr,
        } => {
            f(condition);
            f(true_expr);
            f(false_expr);
        }
        NodeKind::Range { start, end, step } => {
            f(start);
            f(end);
            if let Some(step) = step {
                f(step);
            }
        }
        NodeKind::Relational { params, .. } => {
            for param in params {
                f(param);
            }
        }
        NodeKind::Block { blocks } => {
            for (block, _) in blocks {
                f(block);
            }
        }
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
                NodeKind::Accessor { object, index } if object_property(index).is_some() => {
                    compile(object, arg_names)
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
                if contains_end(dimension) {
                    let mut child = arg_names.clone();
                    child.insert(JsString::from("end"));
                    compile(dimension, &child)?;
                } else {
                    compile(dimension, arg_names)?;
                }
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
    // `isUnit ? new Unit(null, name) : SymbolNode.onUndefinedSymbol(name)`
    if is_valueless_unit(name) {
        return Err(unsupported("a unit"));
    }
    Err(error(cat(&[text("Undefined symbol "), name.clone()])))
}

fn eval_args(
    arg_nodes: &[Node],
    scope: &Scope,
    args: &Rc<Args>,
    context: Option<&Value>,
) -> Result<Vec<Value>, MathError> {
    arg_nodes
        .iter()
        .map(|arg| eval_node(arg, scope, args, context))
        .collect()
}

/// Calls a function value: `math.f(...)` for `def: None`, a user function otherwise.
fn call_function(function: &Function, values: &[Value]) -> Result<Value, MathError> {
    match &function.def {
        None => super::call(&function.name.as_string().unwrap_or_default(), values),
        Some(def) => {
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
            eval_node(&def.expr, &def.scope, &child_args, None)
        }
    }
}

/// `getSafeProperty(object, prop)` for an `AccessorNode` property read; always an error
/// for the values this subset produces.
fn read_property(object: &Value, prop: &JsString) -> MathError {
    match object {
        Value::Null => type_error(cat(&[
            text("Cannot read properties of null (reading '"),
            prop.clone(),
            text("')"),
        ])),
        Value::Undefined => type_error(cat(&[
            text("Cannot read properties of undefined (reading '"),
            prop.clone(),
            text("')"),
        ])),
        _ => {
            let p = prop.as_string().unwrap_or_default();
            if function_valued_property(object, &p) && safe_method(&p) {
                error(cat(&[
                    text("Cannot access method \""),
                    prop.clone(),
                    text("\" as a property"),
                ]))
            } else {
                error(cat(&[
                    text("No access to property \""),
                    prop.clone(),
                    text("\""),
                ]))
            }
        }
    }
}

/// `access(object, index)` for the values this subset produces; only strings succeed.
fn access(object: &Value, dimensions: &[Value]) -> Result<Value, MathError> {
    match object {
        Value::String(s) => get_substring(s, dimensions),
        // `typeof object === 'object'`
        Value::Null | Value::Complex { .. } | Value::ResultSet(_) => {
            if let [Value::String(prop)] = dimensions {
                // `index.isObjectProperty()` is true for a single string dimension
                Err(read_property(object, prop))
            } else {
                Err(type_error(text(
                    "Cannot apply a numeric index as object property",
                )))
            }
        }
        _ => Err(type_error(text(
            "Cannot apply index: unsupported type of object",
        ))),
    }
}

/// `assign(object, index, value)`.
fn assign(object: &Value, dimensions: &[Value], value: &Value) -> Result<Value, MathError> {
    match object {
        Value::String(s) => {
            let Value::String(_) = value else {
                // `subset` has no `string, Index, <other>` signature
                return Err(type_error(cat(&[
                    text(
                        "Unexpected type of argument in function subset (expected: string, actual: ",
                    ),
                    text(type_of(value)),
                    text(", index: 2)"),
                ])));
            };
            set_substring(s, dimensions, value)
        }
        Value::Null | Value::Complex { .. } | Value::ResultSet(_) => {
            if let [Value::String(prop)] = dimensions {
                // `setSafeProperty`: only plain objects accept properties
                Err(error(cat(&[
                    text("No access to property \""),
                    prop.clone(),
                    text("\""),
                ])))
            } else {
                Err(type_error(text(
                    "Cannot apply a numeric index as object property",
                )))
            }
        }
        _ => Err(type_error(text(
            "Cannot apply index: unsupported type of object",
        ))),
    }
}

/// `'' + value`: JS string coercion, used in index-validation messages.
fn js_display(value: &Value) -> String {
    match value {
        Value::Number(n) => {
            if n.is_nan() {
                "NaN".to_owned()
            } else if *n == f64::INFINITY {
                "Infinity".to_owned()
            } else if *n == f64::NEG_INFINITY {
                "-Infinity".to_owned()
            } else {
                format!("{n}")
            }
        }
        Value::String(s) => s.to_string_lossy(),
        Value::Boolean(b) => format!("{b}"),
        Value::Null => "null".to_owned(),
        Value::Undefined => "undefined".to_owned(),
        other => super::format(other).to_string_lossy(),
    }
}

/// `validateIndex` on a zero-based index, with `errorTransform`'s shift back to
/// one-based messages.
fn validate_index(dimension: &Value, length: Option<usize>) -> Result<usize, MathError> {
    let Value::Number(index) = dimension else {
        return Err(type_error(text(&format!(
            "Index must be an integer (value: {})",
            js_display(dimension)
        ))));
    };
    if !index.is_finite() || index.fract() != 0.0 {
        return Err(type_error(text(&format!(
            "Index must be an integer (value: {})",
            js_display(dimension)
        ))));
    }
    if *index < 0.0 {
        // `IndexError(index, length)` -> `errorTransform` -> `IndexError(index+1, 1, length+1)`
        return Err(MathError {
            name: "IndexError",
            message: text(&format!("Index out of range ({} < 1)", index + 1.0)),
        });
    }
    if let Some(length) = length
        && *index >= length as f64
    {
        return Err(MathError {
            name: "IndexError",
            message: text(&format!(
                "Index out of range ({} > {})",
                index + 1.0,
                length
            )),
        });
    }
    Ok(*index as usize)
}

fn dimension_error(actual: usize, expected: usize) -> MathError {
    MathError {
        name: "DimensionError",
        message: text(&format!("Dimension mismatch ({actual} != {expected})")),
    }
}

/// `_getSubstring`.
fn get_substring(s: &JsString, dimensions: &[Value]) -> Result<Value, MathError> {
    if dimensions.len() != 1 {
        return Err(dimension_error(dimensions.len(), 1));
    }
    let index = validate_index(&dimensions[0], Some(s.units().len()))?;
    Ok(Value::String(JsString::from_units(vec![s.units()[index]])))
}

/// `_setSubstring` for a scalar index.
fn set_substring(s: &JsString, dimensions: &[Value], value: &Value) -> Result<Value, MathError> {
    if dimensions.len() != 1 {
        return Err(dimension_error(dimensions.len(), 1));
    }
    let Value::String(replacement) = value else {
        return Err(type_error(cat(&[
            text("Unexpected type of argument in function subset (expected: string, actual: "),
            text(type_of(value)),
            text(", index: 2)"),
        ])));
    };
    // a string dimension has no `.size()`
    let Value::Number(_) = dimensions[0] else {
        return Err(type_error(text("range.size is not a function")));
    };
    if replacement.units().len() != 1 {
        return Err(dimension_error(1, replacement.units().len()));
    }
    // `validateIndex(index.min()[0])`, no length: only the lower bound is checked
    let index = validate_index(&dimensions[0], None)?;
    let mut units = s.units().to_vec();
    units.resize(units.len().max(index + 1), ' ' as u16);
    units[index] = replacement.units()[0];
    Ok(Value::String(JsString::from_units(units)))
}

/// `indexTransform`: every evaluated dimension shifts from one-based to zero-based.
fn transform_dimension(value: &Value) -> Result<Value, MathError> {
    match value {
        Value::Number(n) => Ok(Value::Number(n - 1.0)),
        Value::String(_) => Ok(value.clone()),
        _ => Err(type_error(text(
            "Dimension must be an Array, Matrix, number, string, or Range",
        ))),
    }
}

/// `evalIndexNode`: evaluates the dimensions, binds `end` per dimension where the
/// node subtree needs it, and applies the one-based index transform.
fn eval_index_dimensions(
    dimensions: &[Node],
    scope: &Scope,
    args: &Rc<Args>,
    context: Option<&Value>,
) -> Result<Vec<Value>, MathError> {
    // all dimensions evaluate before `index(...)` applies the transform
    let mut result = Vec::with_capacity(dimensions.len());
    for (i, dimension) in dimensions.iter().enumerate() {
        let value = if contains_end(dimension) {
            match context {
                Some(Value::String(s)) => {
                    // `size(string).valueOf()[i]`; only the first dimension has a size
                    let end = if i == 0 {
                        Value::Number(s.units().len() as f64)
                    } else {
                        Value::Undefined
                    };
                    let child = Args::child(HashMap::from([(JsString::from("end"), end)]), args);
                    eval_node(dimension, scope, &child, context)?
                }
                _ => {
                    return Err(type_error(cat(&[
                        text(
                            "Cannot resolve \"end\": context must be a Matrix, Array, or string but is ",
                        ),
                        text(context.map(type_of).unwrap_or("null")),
                    ])));
                }
            }
        } else {
            eval_node(dimension, scope, args, context)?
        };
        result.push(value);
    }
    result.iter().map(transform_dimension).collect()
}

fn index_dimensions(index: &Node) -> &[Node] {
    match &index.kind {
        NodeKind::Index { dimensions, .. } => dimensions,
        _ => &[],
    }
}

fn eval_accessor(
    object: &Node,
    index: &Node,
    scope: &Scope,
    args: &Rc<Args>,
    context: Option<&Value>,
) -> Result<Value, MathError> {
    let object = eval_node(object, scope, args, context)?;
    if let Some(prop) = object_property(index) {
        Err(read_property(&object, prop))
    } else {
        let dimensions =
            eval_index_dimensions(index_dimensions(index), scope, args, Some(&object))?;
        access(&object, &dimensions)
    }
}

fn eval_assignment(
    object: &Node,
    index: Option<&Node>,
    value: &Node,
    scope: &Scope,
    args: &Rc<Args>,
    context: Option<&Value>,
) -> Result<Value, MathError> {
    match index {
        None => {
            let NodeKind::Symbol { name } = &object.kind else {
                return Err(type_error(text("SymbolNode expected as object")));
            };
            let value = eval_node(value, scope, args, context)?;
            scope.borrow_mut().insert(name.clone(), value.clone());
            Ok(value)
        }
        Some(index) => {
            if let Some(prop) = object_property(index) {
                // `a.b = v`
                eval_node(object, scope, args, context)?;
                eval_node(value, scope, args, context)?;
                Err(error(cat(&[
                    text("No access to property \""),
                    prop.clone(),
                    text("\""),
                ])))
            } else if let NodeKind::Symbol { name } = &object.kind {
                // `a[i] = v`
                let child = eval_node(object, scope, args, context)?;
                let value = eval_node(value, scope, args, context)?;
                let dimensions =
                    eval_index_dimensions(index_dimensions(index), scope, args, Some(&child))?;
                let assigned = assign(&child, &dimensions, &value)?;
                scope.borrow_mut().insert(name.clone(), assigned);
                Ok(value)
            } else if let NodeKind::Accessor {
                object: parent_node,
                index: parent_index,
            } = &object.kind
            {
                if object_property(parent_index).is_some() {
                    // `a.b[i] = v`: `getSafeProperty(parent, prop)` never produces a
                    // subset value, so its error is what upstream reports
                    let parent = eval_node(parent_node, scope, args, context)?;
                    let prop = object_property(parent_index).unwrap();
                    Err(read_property(&parent, prop))
                } else {
                    // `a[i][j] = v`
                    let parent = eval_node(parent_node, scope, args, context)?;
                    let parent_dimensions = eval_index_dimensions(
                        index_dimensions(parent_index),
                        scope,
                        args,
                        Some(&parent),
                    )?;
                    let child = access(&parent, &parent_dimensions)?;
                    let dimensions =
                        eval_index_dimensions(index_dimensions(index), scope, args, Some(&child))?;
                    let value = eval_node(value, scope, args, context)?;
                    let new_child = assign(&child, &dimensions, &value)?;
                    assign(&parent, &parent_dimensions, &new_child)?;
                    Ok(value)
                }
            } else {
                Err(type_error(text("SymbolNode expected as object")))
            }
        }
    }
}

/// A stand-in for `this.fn.toString()` in the `did not evaluate to a function` message;
/// only Symbol/Accessor callees appear in practice, so other kinds get a rough rendering.
fn node_string(node: &Node) -> JsString {
    match &node.kind {
        NodeKind::Symbol { name } => name.clone(),
        NodeKind::Constant(constant) => super::format(&constant_value(constant)),
        NodeKind::Parenthesis { content } => cat(&[text("("), node_string(content), text(")")]),
        NodeKind::Function { callee, args } => {
            let mut parts = vec![node_string(callee), text("(")];
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    parts.push(text(", "));
                }
                parts.push(node_string(arg));
            }
            parts.push(text(")"));
            cat(&parts)
        }
        NodeKind::Accessor { object, index } => cat(&[node_string(object), index_string(index)]),
        _ => text(node.type_name()),
    }
}

fn index_string(index: &Node) -> JsString {
    if let NodeKind::Index {
        dimensions,
        dot_notation,
    } = &index.kind
    {
        if *dot_notation && let Some(prop) = object_property(index) {
            return cat(&[text("."), prop.clone()]);
        }
        let mut parts = vec![text("[")];
        for (i, dimension) in dimensions.iter().enumerate() {
            if i > 0 {
                parts.push(text(", "));
            }
            parts.push(node_string(dimension));
        }
        parts.push(text("]"));
        cat(&parts)
    } else {
        JsString::new()
    }
}

fn eval_call(
    callee: &Node,
    arg_nodes: &[Node],
    scope: &Scope,
    args: &Rc<Args>,
    context: Option<&Value>,
) -> Result<Value, MathError> {
    if let NodeKind::Symbol { name } = &callee.kind {
        if let Some(f) = args.get(name).cloned() {
            // the callee is a function-assignment parameter
            let Value::Function(function) = f else {
                return Err(type_error(cat(&[
                    text("Argument '"),
                    name.clone(),
                    text("' was not a function; received: "),
                    super::format(&f),
                ])));
            };
            let values = eval_args(arg_nodes, scope, args, context)?;
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
        let values = eval_args(arg_nodes, scope, args, context)?;
        call_function(&function, &values)
    } else if let NodeKind::Accessor { object, index } = &callee.kind {
        if let Some(prop) = object_property(index) {
            // `a.b(...)`, called with the object as context
            let object = eval_node(object, scope, args, context)?;
            let p = prop.as_string().unwrap_or_default();
            if function_valued_property(&object, &p) && safe_method(&p) {
                // `object[prop].apply(object, values)` invokes a builtin
                eval_args(arg_nodes, scope, args, context)?;
                return Err(unsupported("a prototype method call"));
            }
            return Err(error(cat(&[
                text("No access to method \""),
                prop.clone(),
                text("\""),
            ])));
        }
        let f = eval_node(callee, scope, args, context)?;
        if !matches!(f, Value::Function(_)) {
            return Err(type_error(cat(&[
                text("Expression '"),
                node_string(callee),
                text("' did not evaluate to a function; value is:\n  "),
                super::format(&f),
            ])));
        }
        let values = eval_args(arg_nodes, scope, args, context)?;
        let Value::Function(function) = f else {
            unreachable!()
        };
        call_function(&function, &values)
    } else {
        let f = eval_node(callee, scope, args, context)?;
        if !matches!(f, Value::Function(_)) {
            return Err(type_error(cat(&[
                text("Expression '"),
                node_string(callee),
                text("' did not evaluate to a function; value is:\n  "),
                super::format(&f),
            ])));
        }
        let values = eval_args(arg_nodes, scope, args, context)?;
        let Value::Function(function) = f else {
            unreachable!()
        };
        call_function(&function, &values)
    }
}

fn eval_node(
    node: &Node,
    scope: &Scope,
    args: &Rc<Args>,
    context: Option<&Value>,
) -> Result<Value, MathError> {
    match &node.kind {
        NodeKind::Constant(constant) => Ok(constant_value(constant)),
        NodeKind::Symbol { name } => eval_symbol(name, scope, args),
        NodeKind::Parenthesis { content } => eval_node(content, scope, args, context),
        NodeKind::Operator {
            fn_name, args: ops, ..
        } => {
            let values = eval_args(ops, scope, args, context)?;
            super::call(fn_name, &values)
        }
        NodeKind::Function {
            callee,
            args: arg_nodes,
        } => eval_call(callee, arg_nodes, scope, args, context),
        NodeKind::Accessor { object, index } => eval_accessor(object, index, scope, args, context),
        NodeKind::Index { dimensions, .. } => {
            eval_index_dimensions(dimensions, scope, args, context)?;
            Err(unsupported("an Index"))
        }
        NodeKind::Array { items } => {
            for item in items {
                eval_node(item, scope, args, context)?;
            }
            Err(unsupported("a matrix"))
        }
        NodeKind::Object { properties } => {
            // `evalEntries` semantics: last node per key, in first-seen order
            let mut order: Vec<&JsString> = Vec::new();
            let mut entries: HashMap<&JsString, &Node> = HashMap::new();
            for (key, value) in properties {
                if !entries.contains_key(key) {
                    order.push(key);
                }
                entries.insert(key, value);
            }
            for key in order {
                eval_node(entries[key], scope, args, context)?;
            }
            Err(unsupported("an object"))
        }
        NodeKind::Assignment {
            object,
            index,
            value,
        } => eval_assignment(object, index.as_deref(), value, scope, args, context),
        NodeKind::FunctionAssignment {
            name,
            params,
            types,
            expr,
        } => {
            let function = Value::Function(Function {
                name: name.clone(),
                def: Some(Rc::new(FunctionDef {
                    params: params.clone(),
                    types: types.clone(),
                    expr: (**expr).clone(),
                    scope: Rc::clone(scope),
                    args: Rc::clone(args),
                })),
            });
            scope.borrow_mut().insert(name.clone(), function.clone());
            Ok(function)
        }
        NodeKind::Conditional {
            condition,
            true_expr,
            false_expr,
        } => {
            if test_condition(&eval_node(condition, scope, args, context)?)? {
                eval_node(true_expr, scope, args, context)
            } else {
                eval_node(false_expr, scope, args, context)
            }
        }
        NodeKind::Range { start, end, step } => {
            eval_node(start, scope, args, context)?;
            eval_node(end, scope, args, context)?;
            if let Some(step) = step {
                eval_node(step, scope, args, context)?;
            }
            Err(unsupported("a range"))
        }
        NodeKind::Relational {
            conditionals,
            params,
        } => {
            let mut rhs = eval_node(&params[0], scope, args, context)?;
            for (i, conditional) in conditionals.iter().enumerate() {
                let lhs = rhs;
                rhs = eval_node(&params[i + 1], scope, args, context)?;
                if !js_truthy(&super::call(conditional, &[lhs, rhs.clone()])?) {
                    return Ok(Value::Boolean(false));
                }
            }
            Ok(Value::Boolean(true))
        }
        NodeKind::Block { blocks } => {
            let mut results = Vec::new();
            for (block, visible) in blocks {
                let result = eval_node(block, scope, args, context)?;
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
    let node = super::parse(expr)?;
    compile(&node, &HashSet::new())?;
    eval_node(
        &node,
        &Rc::new(RefCell::new(HashMap::new())),
        &Args::root(),
        None,
    )
}

#[cfg(test)]
mod smoke_tests {
    use super::*;

    fn eval(s: &str) -> Result<Value, MathError> {
        evaluate(&JsString::from(s))
    }

    fn err(s: &str) -> MathError {
        eval(s).unwrap_err()
    }

    fn num(v: &Value) -> f64 {
        let Value::Number(n) = v else {
            panic!("expected number, got {v:?}")
        };
        *n
    }

    fn str_of(v: &Value) -> String {
        let Value::String(s) = v else {
            panic!("expected string, got {v:?}")
        };
        s.to_string_lossy()
    }

    fn rs(v: &Value) -> &[Value] {
        let Value::ResultSet(entries) = v else {
            panic!("expected result set, got {v:?}")
        };
        entries
    }

    #[test]
    fn constants_and_symbols() {
        assert_eq!(num(&eval("2").unwrap()), 2.0);
        assert_eq!(num(&eval("pi").unwrap()), std::f64::consts::PI);
        assert_eq!(num(&eval("phi").unwrap()), std::f64::consts::GOLDEN_RATIO);
        assert_eq!(str_of(&eval("version").unwrap()), "11.3.0");
        assert!(matches!(
            eval("i").unwrap(),
            Value::Complex { re: 0.0, im: 1.0 }
        ));
        assert!(matches!(eval("true").unwrap(), Value::Boolean(true)));
        assert!(matches!(eval("null").unwrap(), Value::Null));
        assert!(matches!(eval("undefined").unwrap(), Value::Undefined));
        assert_eq!(str_of(&eval("'hi'").unwrap()), "hi");
    }

    #[test]
    fn symbol_errors() {
        assert_eq!(err("x").message.to_string_lossy(), "Undefined symbol x");
        assert_eq!(
            err("toString").message.to_string_lossy(),
            "Cannot access method \"toString\" as a property"
        );
        assert_eq!(
            err("valueOf").message.to_string_lossy(),
            "Cannot access method \"valueOf\" as a property"
        );
        assert_eq!(
            err("constructor").message.to_string_lossy(),
            "No access to property \"constructor\""
        );
        assert_eq!(
            err("__proto__").message.to_string_lossy(),
            "No access to property \"__proto__\""
        );
        assert_eq!(
            err("foo(1)").message.to_string_lossy(),
            "Undefined function foo"
        );
    }

    #[test]
    fn conditionals() {
        assert_eq!(num(&eval("1 ? 2 : 3").unwrap()), 2.0);
        assert_eq!(num(&eval("0 ? 2 : 3").unwrap()), 3.0);
        assert_eq!(num(&eval("NaN ? 1 : 2").unwrap()), 2.0);
        assert_eq!(num(&eval("'' ? 1 : 2").unwrap()), 2.0);
        assert_eq!(num(&eval("'0' ? 1 : 2").unwrap()), 1.0);
        assert_eq!(num(&eval("null ? 1 : 2").unwrap()), 2.0);
        assert_eq!(num(&eval("undefined ? 1 : 2").unwrap()), 2.0);
        assert_eq!(num(&eval("true ? 1 : 2").unwrap()), 1.0);
        // lazy branch evaluation
        assert_eq!(num(&eval("1 ? 2 : x").unwrap()), 2.0);
        assert_eq!(
            err("1 ? x : 2").message.to_string_lossy(),
            "Undefined symbol x"
        );
        let e = err("sin ? 1 : 2");
        assert_eq!(e.name, "TypeError");
        assert_eq!(
            e.message.to_string_lossy(),
            "Unsupported type of condition \"function\""
        );
    }

    #[test]
    fn assignments_and_blocks() {
        assert_eq!(num(&eval("x = 2").unwrap()), 2.0);
        assert_eq!(rs(&eval("x = 2; x").unwrap()).len(), 1);
        assert_eq!(num(&rs(&eval("x = 2; x").unwrap())[0]), 2.0);
        assert_eq!(rs(&eval("x = 2;").unwrap()).len(), 0);
        assert_eq!(rs(&eval("1; 2").unwrap()).len(), 1);
        assert_eq!(rs(&eval("1\n2").unwrap()).len(), 2);
        assert_eq!(rs(&eval("1;2;").unwrap()).len(), 0);
        assert_eq!(num(&rs(&eval("pi = 3; pi").unwrap())[0]), 3.0);
        assert_eq!(
            err("end = 2").message.to_string_lossy(),
            "Cannot assign to symbol \"end\""
        );
    }

    #[test]
    fn function_assignments() {
        assert!(matches!(eval("f(x) = x").unwrap(), Value::Function(_)));
        assert_eq!(num(&rs(&eval("f(x) = x; f(3)").unwrap())[0]), 3.0);
        assert_eq!(num(&rs(&eval("f() = 7; f()").unwrap())[0]), 7.0);
        assert!(matches!(
            rs(&eval("f(x) = x; f(null)").unwrap())[0],
            Value::Null
        ));
        let e = err("f(x) = x; f()");
        assert_eq!(e.name, "TypeError");
        assert_eq!(
            e.message.to_string_lossy(),
            "Too few arguments in function f (expected: any, index: 0)"
        );
        let e = err("f(x) = x; f(1, 2)");
        assert_eq!(
            e.message.to_string_lossy(),
            "Too many arguments in function f (expected: 1, actual: 2)"
        );
        assert_eq!(
            err("f(x) = y; f(1)").message.to_string_lossy(),
            "Undefined symbol y"
        );
        // captured scope: the assignment inside the body writes the outer scope
        assert_eq!(num(&rs(&eval("f() = (g = 4); f(); g").unwrap())[0]), 4.0);
    }

    #[test]
    fn string_access() {
        assert_eq!(str_of(&rs(&eval("a = 'hi'; a[1]").unwrap())[0]), "h");
        assert_eq!(str_of(&rs(&eval("a = 'hi'; a[end]").unwrap())[0]), "i");
        assert_eq!(
            str_of(&rs(&eval("a = 'hi'; a[2] = 'x'; a").unwrap())[0]),
            "hx"
        );
        let e = err("a = 'hi'; a[0]");
        assert_eq!(e.name, "IndexError");
        assert_eq!(e.message.to_string_lossy(), "Index out of range (0 < 1)");
        let e = err("a = 'hi'; a[5]");
        assert_eq!(e.name, "IndexError");
        assert_eq!(e.message.to_string_lossy(), "Index out of range (5 > 2)");
        let e = err("a = 'hi'; a[1.5]");
        assert_eq!(e.name, "TypeError");
        assert_eq!(
            e.message.to_string_lossy(),
            "Index must be an integer (value: 0.5)"
        );
        let e = err("a = 5; a[1]");
        assert_eq!(e.name, "TypeError");
        assert_eq!(
            e.message.to_string_lossy(),
            "Cannot apply index: unsupported type of object"
        );
        let e = err("a = 'hi'; a[true]");
        assert_eq!(e.name, "TypeError");
        assert_eq!(
            e.message.to_string_lossy(),
            "Dimension must be an Array, Matrix, number, string, or Range"
        );
        let e = err("a = 'hi'; a.b");
        assert_eq!(e.name, "Error");
        assert_eq!(e.message.to_string_lossy(), "No access to property \"b\"");
        let e = err("a = 'hi'; a.toString");
        assert_eq!(
            e.message.to_string_lossy(),
            "Cannot access method \"toString\" as a property"
        );
    }
}
