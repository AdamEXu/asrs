use asrs_js::json::parse_units;
use asrs_js::string::JsString;
use asrs_js::value::JsObject;

use super::MathError;
use super::fns::number_from_string;
use super::node::{Constant, Node, NodeKind};
use super::stack;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenType {
    Null,
    Delimiter,
    Number,
    Symbol,
    Unknown,
}

#[derive(Clone)]
struct State<'a> {
    expr: &'a [u16],
    comment: Vec<u16>,
    index: usize,
    token: Vec<u16>,
    token_type: TokenType,
    nesting_level: i32,
    conditional_level: Option<i32>,
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn err(name: &'static str, message: impl Into<JsString>) -> MathError {
    MathError {
        name,
        message: message.into(),
    }
}

fn u(units: &[u16]) -> JsString {
    JsString::from_units(units.to_vec())
}

fn col(s: &State<'_>) -> i64 {
    s.index as i64 - s.token.len() as i64 + 1
}

fn syntax_error(s: &State<'_>, message: &str) -> MathError {
    err(
        "SyntaxError",
        JsString::from(format!("{message} (char {})", col(s))),
    )
}

fn syntax_error_js(s: &State<'_>, message: JsString) -> MathError {
    let mut m = message.into_units();
    m.extend(format!(" (char {})", col(s)).encode_utf16());
    err("SyntaxError", JsString::from_units(m))
}

fn mk(kind: NodeKind) -> Node {
    Node::new(kind)
}

fn constant(value: Constant) -> Node {
    mk(NodeKind::Constant(value))
}

fn symbol(name: &[u16]) -> Node {
    mk(NodeKind::Symbol { name: u(name) })
}

fn operator(op: &'static str, fn_name: &'static str, args: Vec<Node>) -> Node {
    mk(NodeKind::Operator {
        op,
        fn_name,
        implicit: false,
        is_percentage: false,
        args,
    })
}

fn is_constant(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Constant(_))
}

fn is_symbol(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Symbol { .. })
}

fn is_accessor(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Accessor { .. })
}

fn is_function(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Function { .. })
}

fn is_operator(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Operator { .. })
}

fn is_percentage(n: &Node) -> bool {
    matches!(&n.kind, NodeKind::Operator { is_percentage, .. } if *is_percentage)
}

fn rule2_node(n: &Node) -> bool {
    is_constant(n)
        || matches!(&n.kind, NodeKind::Operator { op, args, .. }
            if args.len() == 1 && is_constant(&args[0]) && matches!(*op, "-" | "+" | "~"))
}

fn tok(s: &State<'_>, t: &str) -> bool {
    s.token.iter().copied().eq(t.encode_utf16())
}

fn named_delimiter(t: &[u16]) -> bool {
    ["mod", "to", "in", "and", "xor", "or", "not"]
        .iter()
        .any(|d| t.iter().copied().eq(d.encode_utf16()))
}

fn delimiter(t: &[u16]) -> bool {
    const D: &[&str] = &[
        ",", "(", ")", "[", "]", "{", "}", "\"", "'", ";", "+", "-", "*", ".*", "/", "./", "%",
        "^", ".^", "~", "!", "&", "|", "^|", "=", ":", "?", "==", "!=", "<", ">", "<=", ">=", "<<",
        ">>", ">>>",
    ];
    D.iter().any(|d| t.iter().copied().eq(d.encode_utf16()))
}

fn is_valid_latin_or_greek(c: Option<u16>) -> bool {
    matches!(
        c,
        Some(
            0x61..=0x7a
            | 0x41..=0x5a
            | 0x5f
            | 0x24
            | 0xc0..=0x2af
            | 0x370..=0x3ff
            | 0x2100..=0x214f,
        )
    )
}

fn is_valid_math_symbol(high: Option<u16>, low: Option<u16>) -> bool {
    match (high, low) {
        (Some(0xd835), Some(l)) => {
            (0xdc00..=0xdfff).contains(&l)
                && !matches!(
                    l,
                    0xdc55
                        | 0xdc9d
                        | 0xdca0
                        | 0xdca1
                        | 0xdca3
                        | 0xdca4
                        | 0xdca7
                        | 0xdca8
                        | 0xdcad
                        | 0xdcba
                        | 0xdcbc
                        | 0xdcc4
                        | 0xdd06
                        | 0xdd0b
                        | 0xdd0c
                        | 0xdd15
                        | 0xdd1d
                        | 0xdd3a
                        | 0xdd3f
                        | 0xdd45
                        | 0xdd47
                        ..=0xdd49 | 0xdd51 | 0xdea6 | 0xdea7 | 0xdfcc | 0xdfcd
                )
        }
        _ => false,
    }
}

fn is_alpha(c: Option<u16>, c_prev: Option<u16>, c_next: Option<u16>) -> bool {
    is_valid_latin_or_greek(c) || is_valid_math_symbol(c, c_next) || is_valid_math_symbol(c_prev, c)
}

fn is_whitespace(c: Option<u16>, nesting_level: i32) -> bool {
    c == Some(0x20) || c == Some(0x09) || (c == Some(0x0a) && nesting_level > 0)
}

fn is_decimal_mark(c: Option<u16>, c_next: Option<u16>) -> bool {
    c == Some(0x2e) && c_next != Some(0x2f) && c_next != Some(0x2a) && c_next != Some(0x5e)
}

fn is_digit_dot(c: Option<u16>) -> bool {
    matches!(c, Some(0x30..=0x39)) || c == Some(0x2e)
}

fn is_digit(c: Option<u16>) -> bool {
    matches!(c, Some(0x30..=0x39))
}

fn is_hex_digit(c: Option<u16>) -> bool {
    matches!(c, Some(0x30..=0x39 | 0x61..=0x66 | 0x41..=0x46))
}

impl State<'_> {
    fn cur(&self) -> Option<u16> {
        self.expr.get(self.index).copied()
    }

    fn at(&self, off: usize) -> Option<u16> {
        self.expr.get(self.index + off).copied()
    }

    fn prev(&self) -> Option<u16> {
        self.index
            .checked_sub(1)
            .and_then(|i| self.expr.get(i))
            .copied()
    }

    fn push_cur(&mut self) {
        if let Some(c) = self.cur() {
            self.token.push(c);
        }
    }
}

fn get_token(s: &mut State<'_>) -> Result<(), MathError> {
    s.token_type = TokenType::Null;
    s.token.clear();
    s.comment.clear();

    loop {
        if s.cur() == Some(u16::from(b'#')) {
            while let Some(c) = s.cur() {
                if c == 0x0a {
                    break;
                }
                s.comment.push(c);
                s.index += 1;
            }
        }
        if is_whitespace(s.cur(), s.nesting_level) {
            s.index += 1;
        } else {
            break;
        }
    }

    if s.cur().is_none() {
        s.token_type = TokenType::Delimiter;
        return Ok(());
    }

    if s.cur() == Some(0x0a) && s.nesting_level == 0 {
        s.token_type = TokenType::Delimiter;
        s.token.push(0x0a);
        s.index += 1;
        return Ok(());
    }

    for len in [3usize, 2, 1] {
        if s.index + len <= s.expr.len() && delimiter(&s.expr[s.index..s.index + len]) {
            s.token_type = TokenType::Delimiter;
            s.token.extend_from_slice(&s.expr[s.index..s.index + len]);
            s.index += len;
            return Ok(());
        }
    }

    if is_digit_dot(s.cur()) {
        s.token_type = TokenType::Number;

        if s.at(1).is_some() && s.cur() == Some(0x30) && matches!(s.at(1), Some(0x62 | 0x6f | 0x78))
        {
            s.push_cur();
            s.index += 1;
            s.push_cur();
            s.index += 1;
            while is_hex_digit(s.cur()) {
                s.push_cur();
                s.index += 1;
            }
            if s.cur() == Some(0x2e) {
                s.push_cur();
                s.index += 1;
                while is_hex_digit(s.cur()) {
                    s.push_cur();
                    s.index += 1;
                }
            } else if s.cur() == Some(0x69) {
                s.push_cur();
                s.index += 1;
                while is_digit(s.cur()) {
                    s.push_cur();
                    s.index += 1;
                }
            }
            return Ok(());
        }

        if s.cur() == Some(0x2e) {
            s.push_cur();
            s.index += 1;
            if !is_digit(s.cur()) {
                s.token_type = TokenType::Delimiter;
                return Ok(());
            }
        } else {
            while is_digit(s.cur()) {
                s.push_cur();
                s.index += 1;
            }
            if is_decimal_mark(s.cur(), s.at(1)) {
                s.push_cur();
                s.index += 1;
            }
        }
        while is_digit(s.cur()) {
            s.push_cur();
            s.index += 1;
        }
        if s.cur() == Some(0x45) || s.cur() == Some(0x65) {
            if is_digit(s.at(1)) || s.at(1) == Some(0x2d) || s.at(1) == Some(0x2b) {
                s.push_cur();
                s.index += 1;
                if s.cur() == Some(0x2b) || s.cur() == Some(0x2d) {
                    s.push_cur();
                    s.index += 1;
                }
                if !is_digit(s.cur()) {
                    let mut m = units("Digit expected, got \"");
                    if let Some(c) = s.cur() {
                        m.push(c);
                    }
                    m.extend(units("\""));
                    return Err(syntax_error_js(s, JsString::from_units(m)));
                }
                while is_digit(s.cur()) {
                    s.push_cur();
                    s.index += 1;
                }
                if is_decimal_mark(s.cur(), s.at(1)) {
                    let mut m = units("Digit expected, got \"");
                    if let Some(c) = s.cur() {
                        m.push(c);
                    }
                    m.extend(units("\""));
                    return Err(syntax_error_js(s, JsString::from_units(m)));
                }
            } else if s.at(1) == Some(0x2e) {
                s.index += 1;
                let mut m = units("Digit expected, got \"");
                if let Some(c) = s.cur() {
                    m.push(c);
                }
                m.extend(units("\""));
                return Err(syntax_error_js(s, JsString::from_units(m)));
            }
        }
        return Ok(());
    }

    if is_alpha(s.cur(), s.prev(), s.at(1)) {
        while is_alpha(s.cur(), s.prev(), s.at(1)) || is_digit(s.cur()) {
            s.push_cur();
            s.index += 1;
        }
        s.token_type = if named_delimiter(&s.token) {
            TokenType::Delimiter
        } else {
            TokenType::Symbol
        };
        return Ok(());
    }

    s.token_type = TokenType::Unknown;
    while let Some(c) = s.cur() {
        s.token.push(c);
        s.index += 1;
    }
    let mut m = units("Syntax error in part \"");
    m.extend_from_slice(&s.token);
    m.extend(units("\""));
    Err(syntax_error_js(s, JsString::from_units(m)))
}

fn get_token_skip_newline(s: &mut State<'_>) -> Result<(), MathError> {
    loop {
        get_token(s)?;
        if !tok(s, "\n") {
            return Ok(());
        }
    }
}

fn open_params(s: &mut State<'_>) {
    s.nesting_level += 1;
}

fn close_params(s: &mut State<'_>) {
    s.nesting_level -= 1;
}

pub fn parse(expr: &JsString) -> Result<Node, MathError> {
    let mut s = State {
        expr: expr.units(),
        comment: Vec::new(),
        index: 0,
        token: Vec::new(),
        token_type: TokenType::Null,
        nesting_level: 0,
        conditional_level: None,
    };
    parse_start(&mut s)
}

fn parse_start(s: &mut State<'_>) -> Result<Node, MathError> {
    get_token(s)?;
    let node = parse_block(s)?;
    if !s.token.is_empty() {
        if s.token_type == TokenType::Delimiter {
            let mut m = units("Unexpected operator ");
            m.extend_from_slice(&s.token);
            return Err(syntax_error_js(s, JsString::from_units(m)));
        } else {
            let mut m = units("Unexpected part \"");
            m.extend_from_slice(&s.token);
            m.extend(units("\""));
            return Err(syntax_error_js(s, JsString::from_units(m)));
        }
    }
    Ok(node)
}

fn parse_block(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node: Option<Node> = None;
    let mut blocks: Vec<(Node, bool)> = Vec::new();
    if !s.token.is_empty() && !tok(s, "\n") && !tok(s, ";") {
        let mut n = parse_assignment(s)?;
        if !s.comment.is_empty() {
            n.comment = Some(u(&s.comment));
        }
        node = Some(n);
    }
    while tok(s, "\n") || tok(s, ";") {
        if blocks.is_empty()
            && let Some(n) = node.take()
        {
            let visible = !tok(s, ";");
            blocks.push((n, visible));
        }
        get_token(s)?;
        if !tok(s, "\n") && !tok(s, ";") && !s.token.is_empty() {
            let mut n = parse_assignment(s)?;
            if !s.comment.is_empty() {
                n.comment = Some(u(&s.comment));
            }
            let visible = !tok(s, ";");
            blocks.push((n, visible));
            node = None;
        }
    }
    if !blocks.is_empty() {
        return Ok(mk(NodeKind::Block { blocks }));
    }
    match node {
        Some(n) => Ok(n),
        None => {
            let mut n = constant(Constant::Undefined);
            if !s.comment.is_empty() {
                n.comment = Some(u(&s.comment));
            }
            Ok(n)
        }
    }
}

fn parse_assignment(s: &mut State<'_>) -> Result<Node, MathError> {
    stack::check()?;
    let mut node = parse_conditional(s)?;
    if tok(s, "=") {
        if is_symbol(&node) {
            let NodeKind::Symbol { name } = &node.kind else {
                unreachable!()
            };
            let name = name.clone();
            get_token_skip_newline(s)?;
            let value = parse_assignment(s)?;
            if name.units() == units("end") {
                return Err(err(
                    "Error",
                    JsString::from("Cannot assign to symbol \"end\""),
                ));
            }
            return Ok(mk(NodeKind::Assignment {
                object: Box::new(symbol(name.units())),
                index: None,
                value: Box::new(value),
            }));
        } else if is_accessor(&node) {
            get_token_skip_newline(s)?;
            let value = parse_assignment(s)?;
            let NodeKind::Accessor { object, index } =
                std::mem::replace(&mut node.kind, NodeKind::Constant(Constant::Undefined))
            else {
                unreachable!()
            };
            // The AssignmentNode constructor validates object and the reserved name "end".
            if !is_symbol(&object) && !is_accessor(&object) {
                return Err(err(
                    "TypeError",
                    JsString::from("SymbolNode or AccessorNode expected as \"object\""),
                ));
            }
            if let NodeKind::Symbol { name } = &object.kind
                && name.units() == units("end")
            {
                return Err(err(
                    "Error",
                    JsString::from("Cannot assign to symbol \"end\""),
                ));
            }
            return Ok(mk(NodeKind::Assignment {
                object,
                index: Some(index),
                value: Box::new(value),
            }));
        } else if is_function(&node)
            && let NodeKind::Function { callee, args } = &node.kind
            && is_symbol(callee)
        {
            let NodeKind::Symbol { name } = &callee.kind else {
                unreachable!()
            };
            let name = name.clone();
            let mut valid = true;
            let mut params: Vec<Vec<u16>> = Vec::with_capacity(args.len());
            for arg in args {
                if let NodeKind::Symbol { name } = &arg.kind {
                    params.push(name.units().to_vec());
                } else {
                    valid = false;
                }
            }
            if valid {
                get_token_skip_newline(s)?;
                let value = parse_assignment(s)?;
                if name.units() == units("end") {
                    return Err(err(
                        "Error",
                        JsString::from("Illegal function name, \"end\" is a reserved keyword"),
                    ));
                }
                let types = vec![JsString::from("any"); params.len()];
                return Ok(mk(NodeKind::FunctionAssignment {
                    name,
                    params: params.into_iter().map(|p| u(&p)).collect(),
                    types,
                    expr: Box::new(value),
                }));
            }
        }
        return Err(syntax_error(
            s,
            "Invalid left hand side of assignment operator =",
        ));
    }
    Ok(node)
}

fn parse_conditional(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_logical_or(s)?;
    while tok(s, "?") {
        let prev = s.conditional_level;
        s.conditional_level = Some(s.nesting_level);
        get_token_skip_newline(s)?;
        let condition = node;
        let true_expr = parse_assignment(s)?;
        if !tok(s, ":") {
            return Err(syntax_error(
                s,
                "False part of conditional expression expected",
            ));
        }
        s.conditional_level = None;
        get_token_skip_newline(s)?;
        let false_expr = parse_assignment(s)?;
        node = mk(NodeKind::Conditional {
            condition: Box::new(condition),
            true_expr: Box::new(true_expr),
            false_expr: Box::new(false_expr),
        });
        s.conditional_level = prev;
    }
    Ok(node)
}

fn parse_logical_or(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_logical_xor(s)?;
    while tok(s, "or") {
        get_token_skip_newline(s)?;
        node = operator("or", "or", vec![node, parse_logical_xor(s)?]);
    }
    Ok(node)
}

fn parse_logical_xor(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_logical_and(s)?;
    while tok(s, "xor") {
        get_token_skip_newline(s)?;
        node = operator("xor", "xor", vec![node, parse_logical_and(s)?]);
    }
    Ok(node)
}

fn parse_logical_and(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_bitwise_or(s)?;
    while tok(s, "and") {
        get_token_skip_newline(s)?;
        node = operator("and", "and", vec![node, parse_bitwise_or(s)?]);
    }
    Ok(node)
}

fn parse_bitwise_or(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_bitwise_xor(s)?;
    while tok(s, "|") {
        get_token_skip_newline(s)?;
        node = operator("|", "bitOr", vec![node, parse_bitwise_xor(s)?]);
    }
    Ok(node)
}

fn parse_bitwise_xor(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_bitwise_and(s)?;
    while tok(s, "^|") {
        get_token_skip_newline(s)?;
        node = operator("^|", "bitXor", vec![node, parse_bitwise_and(s)?]);
    }
    Ok(node)
}

fn parse_bitwise_and(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_relational(s)?;
    while tok(s, "&") {
        get_token_skip_newline(s)?;
        node = operator("&", "bitAnd", vec![node, parse_relational(s)?]);
    }
    Ok(node)
}

fn parse_relational(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut params = vec![parse_shift(s)?];
    let mut conditionals: Vec<(&'static str, &'static str)> = Vec::new();
    while let Some(&cond) = [
        ("==", "equal"),
        ("!=", "unequal"),
        ("<", "smaller"),
        (">", "larger"),
        ("<=", "smallerEq"),
        (">=", "largerEq"),
    ]
    .iter()
    .find(|(op, _)| tok(s, op))
    {
        conditionals.push(cond);
        get_token_skip_newline(s)?;
        params.push(parse_shift(s)?);
    }
    match params.len() {
        1 => Ok(params.pop().unwrap()),
        2 => Ok(operator(conditionals[0].0, conditionals[0].1, params)),
        _ => Ok(mk(NodeKind::Relational {
            conditionals: conditionals.iter().map(|c| c.1).collect(),
            params,
        })),
    }
}

fn parse_shift(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_conversion(s)?;
    while let Some(&(name, fn_name)) = [
        ("<<", "leftShift"),
        (">>", "rightArithShift"),
        (">>>", "rightLogShift"),
    ]
    .iter()
    .find(|(op, _)| tok(s, op))
    {
        get_token_skip_newline(s)?;
        node = operator(name, fn_name, vec![node, parse_conversion(s)?]);
    }
    Ok(node)
}

fn parse_conversion(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_range(s)?;
    while tok(s, "to") || tok(s, "in") {
        let name = if tok(s, "to") { "to" } else { "in" };
        get_token_skip_newline(s)?;
        if name == "in" && s.token.is_empty() {
            node = mk(NodeKind::Operator {
                op: "*",
                fn_name: "multiply",
                implicit: true,
                is_percentage: false,
                args: vec![node, symbol(&units("in"))],
            });
        } else {
            node = operator(name, "to", vec![node, parse_range(s)?]);
        }
    }
    Ok(node)
}

fn parse_range(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node;
    let mut params: Vec<Node> = Vec::new();
    if tok(s, ":") {
        node = constant(Constant::Number(1.0));
    } else {
        node = parse_add_subtract(s)?;
    }
    if tok(s, ":") && s.conditional_level != Some(s.nesting_level) {
        params.push(node);
        while tok(s, ":") && params.len() < 3 {
            get_token_skip_newline(s)?;
            if tok(s, ")") || tok(s, "]") || tok(s, ",") || s.token.is_empty() {
                params.push(symbol(&units("end")));
            } else {
                params.push(parse_add_subtract(s)?);
            }
        }
        node = if params.len() == 3 {
            let mut it = params.into_iter();
            let start = it.next().unwrap();
            let step = it.next().unwrap();
            let end = it.next().unwrap();
            mk(NodeKind::Range {
                start: Box::new(start),
                end: Box::new(end),
                step: Some(Box::new(step)),
            })
        } else {
            let mut it = params.into_iter();
            let start = it.next().unwrap();
            let end = it.next().unwrap();
            mk(NodeKind::Range {
                start: Box::new(start),
                end: Box::new(end),
                step: None,
            })
        };
    }
    Ok(node)
}

fn parse_add_subtract(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_multiply_divide(s)?;
    while let Some(&(name, fn_name)) = [("+", "add"), ("-", "subtract")]
        .iter()
        .find(|(op, _)| tok(s, op))
    {
        get_token_skip_newline(s)?;
        let right = parse_multiply_divide(s)?;
        let params = if is_percentage(&right) {
            vec![
                node.clone(),
                mk(NodeKind::Operator {
                    op: "*",
                    fn_name: "multiply",
                    implicit: false,
                    is_percentage: false,
                    args: vec![node, right],
                }),
            ]
        } else {
            vec![node, right]
        };
        node = operator(name, fn_name, params);
    }
    Ok(node)
}

fn parse_multiply_divide(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_implicit_multiplication(s)?;
    while let Some(&(name, fn_name)) = [
        ("*", "multiply"),
        (".*", "dotMultiply"),
        ("/", "divide"),
        ("./", "dotDivide"),
    ]
    .iter()
    .find(|(op, _)| tok(s, op))
    {
        get_token_skip_newline(s)?;
        let last = parse_implicit_multiplication(s)?;
        node = operator(name, fn_name, vec![node, last]);
    }
    Ok(node)
}

fn parse_implicit_multiplication(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_rule2(s)?;
    let mut last = node.clone();
    loop {
        if s.token_type == TokenType::Symbol
            || (tok(s, "in") && is_constant(&node))
            || (s.token_type == TokenType::Number
                && !is_constant(&last)
                && (!is_operator(&last)
                    || matches!(&last.kind, NodeKind::Operator { op: "!", .. })))
            || tok(s, "(")
        {
            last = parse_rule2(s)?;
            node = mk(NodeKind::Operator {
                op: "*",
                fn_name: "multiply",
                implicit: true,
                is_percentage: false,
                args: vec![node, last.clone()],
            });
        } else {
            break;
        }
    }
    Ok(node)
}

fn parse_rule2(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_percentage(s)?;
    let mut last = node.clone();
    let mut token_states: Vec<State<'_>> = Vec::new();
    loop {
        if tok(s, "/") && rule2_node(&last) {
            token_states.push(s.clone());
            get_token_skip_newline(s)?;
            if s.token_type == TokenType::Number {
                token_states.push(s.clone());
                get_token_skip_newline(s)?;
                if s.token_type == TokenType::Symbol || tok(s, "(") {
                    *s = token_states.pop().unwrap();
                    token_states.pop();
                    last = parse_percentage(s)?;
                    node = operator("/", "divide", vec![node, last.clone()]);
                } else {
                    token_states.pop();
                    *s = token_states.pop().unwrap();
                    break;
                }
            } else {
                *s = token_states.pop().unwrap();
                break;
            }
        } else {
            break;
        }
    }
    Ok(node)
}

fn parse_percentage(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_unary(s)?;
    while tok(s, "%") || tok(s, "mod") {
        let name_is_pct = tok(s, "%");
        let name = if name_is_pct { "%" } else { "mod" };
        get_token_skip_newline(s)?;
        if name_is_pct && s.token_type == TokenType::Delimiter && !tok(s, "(") {
            node = mk(NodeKind::Operator {
                op: "/",
                fn_name: "divide",
                implicit: false,
                is_percentage: true,
                args: vec![node, constant(Constant::Number(100.0))],
            });
        } else {
            let params = vec![node, parse_unary(s)?];
            node = operator(name, "mod", params);
        }
    }
    Ok(node)
}

fn parse_unary(s: &mut State<'_>) -> Result<Node, MathError> {
    stack::check()?;
    if let Some(&(name, fn_name)) = [
        ("-", "unaryMinus"),
        ("+", "unaryPlus"),
        ("~", "bitNot"),
        ("not", "not"),
    ]
    .iter()
    .find(|(op, _)| tok(s, op))
    {
        get_token_skip_newline(s)?;
        return Ok(operator(name, fn_name, vec![parse_unary(s)?]));
    }
    parse_pow(s)
}

fn parse_pow(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_left_hand_operators(s)?;
    if tok(s, "^") || tok(s, ".^") {
        let (name, fn_name) = if tok(s, "^") {
            ("^", "pow")
        } else {
            (".^", "dotPow")
        };
        get_token_skip_newline(s)?;
        node = operator(name, fn_name, vec![node, parse_unary(s)?]);
    }
    Ok(node)
}

fn parse_left_hand_operators(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut node = parse_symbol_entry(s)?;
    while tok(s, "!") || tok(s, "'") {
        let (name, fn_name) = if tok(s, "!") {
            ("!", "factorial")
        } else {
            ("'", "ctranspose")
        };
        get_token(s)?;
        node = operator(name, fn_name, vec![node]);
        node = parse_accessors(s, node)?;
    }
    Ok(node)
}

fn parse_symbol_entry(s: &mut State<'_>) -> Result<Node, MathError> {
    // parseCustomNodes: extraNodes is always empty, so it always falls through to parseSymbol.
    parse_symbol(s)
}

fn parse_symbol(s: &mut State<'_>) -> Result<Node, MathError> {
    if s.token_type == TokenType::Symbol
        || (s.token_type == TokenType::Delimiter && named_delimiter(&s.token))
    {
        let name = s.token.clone();
        get_token(s)?;
        let node = if name == units("true") {
            constant(Constant::Boolean(true))
        } else if name == units("false") {
            constant(Constant::Boolean(false))
        } else if name == units("null") {
            constant(Constant::Null)
        } else if name == units("undefined") {
            constant(Constant::Undefined)
        } else if name == units("NaN") {
            constant(Constant::Number(f64::NAN))
        } else if name == units("Infinity") {
            constant(Constant::Number(f64::INFINITY))
        } else {
            symbol(&name)
        };
        return parse_accessors(s, node);
    }
    parse_double_quotes_string(s)
}

fn parse_accessors(s: &mut State<'_>, mut node: Node) -> Result<Node, MathError> {
    while tok(s, "(") || tok(s, "[") || tok(s, ".") {
        let mut params: Vec<Node> = Vec::new();
        if tok(s, "(") {
            if is_symbol(&node) || is_accessor(&node) {
                open_params(s);
                get_token(s)?;
                if !tok(s, ")") {
                    params.push(parse_assignment(s)?);
                    while tok(s, ",") {
                        get_token(s)?;
                        params.push(parse_assignment(s)?);
                    }
                }
                if !tok(s, ")") {
                    return Err(syntax_error(s, "Parenthesis ) expected"));
                }
                close_params(s);
                get_token(s)?;
                node = mk(NodeKind::Function {
                    callee: Box::new(node),
                    args: params,
                });
            } else {
                return Ok(node);
            }
        } else if tok(s, "[") {
            open_params(s);
            get_token(s)?;
            if !tok(s, "]") {
                params.push(parse_assignment(s)?);
                while tok(s, ",") {
                    get_token(s)?;
                    params.push(parse_assignment(s)?);
                }
            }
            if !tok(s, "]") {
                return Err(syntax_error(s, "Parenthesis ] expected"));
            }
            close_params(s);
            get_token(s)?;
            node = mk(NodeKind::Accessor {
                object: Box::new(node),
                index: Box::new(mk(NodeKind::Index {
                    dimensions: params,
                    dot_notation: false,
                })),
            });
        } else {
            get_token(s)?;
            if s.token_type != TokenType::Symbol {
                return Err(syntax_error(s, "Property name expected after dot"));
            }
            params.push(constant(Constant::String(u(&s.token))));
            get_token(s)?;
            node = mk(NodeKind::Accessor {
                object: Box::new(node),
                index: Box::new(mk(NodeKind::Index {
                    dimensions: params,
                    dot_notation: true,
                })),
            });
        }
    }
    Ok(node)
}

fn parse_double_quotes_string(s: &mut State<'_>) -> Result<Node, MathError> {
    if tok(s, "\"") {
        let str_ = string_token(s, u16::from(b'"'))?;
        let node = constant(Constant::String(str_));
        return parse_accessors(s, node);
    }
    parse_single_quotes_string(s)
}

fn parse_single_quotes_string(s: &mut State<'_>) -> Result<Node, MathError> {
    if tok(s, "'") {
        let str_ = string_token(s, u16::from(b'\''))?;
        let node = constant(Constant::String(str_));
        return parse_accessors(s, node);
    }
    parse_matrix(s)
}

/// Collects the raw contents of a quoted string (escapes left intact), consumes the
/// closing quote, and decodes it as `JSON.parse('"' + str + '"')` does.
fn string_token(s: &mut State<'_>, quote: u16) -> Result<JsString, MathError> {
    let mut raw: Vec<u16> = Vec::new();
    while let Some(c) = s.cur() {
        if c == quote {
            break;
        }
        if c == u16::from(b'\\') {
            raw.push(c);
            s.index += 1;
        }
        if let Some(c) = s.cur() {
            raw.push(c);
        }
        s.index += 1;
    }
    get_token(s)?;
    if !s.token.iter().copied().eq(std::iter::once(quote)) {
        let q = char::from(quote as u8);
        return Err(syntax_error(s, &format!("End of string {q} expected")));
    }
    get_token(s)?;
    let mut json = Vec::with_capacity(raw.len() + 2);
    json.push(u16::from(b'"'));
    json.extend_from_slice(&raw);
    json.push(u16::from(b'"'));
    match parse_units(&json) {
        Ok(v) => match v.into_string() {
            Some(decoded) => Ok(decoded),
            None => unreachable!("JSON.parse of a quoted string yields a string or throws"),
        },
        Err(e) => Err(err(e.name, e.message)),
    }
}

fn parse_matrix(s: &mut State<'_>) -> Result<Node, MathError> {
    if tok(s, "[") {
        open_params(s);
        get_token(s)?;
        let array;
        if !tok(s, "]") {
            let row = parse_row(s)?;
            if tok(s, ";") {
                let mut params = vec![row];
                while tok(s, ";") {
                    get_token(s)?;
                    params.push(parse_row(s)?);
                }
                if !tok(s, "]") {
                    return Err(syntax_error(s, "End of matrix ] expected"));
                }
                close_params(s);
                get_token(s)?;
                let cols = match &params[0].kind {
                    NodeKind::Array { items } => items.len(),
                    _ => unreachable!(),
                };
                for row in &params[1..] {
                    let NodeKind::Array { items } = &row.kind else {
                        unreachable!()
                    };
                    if items.len() != cols {
                        return Err(syntax_error(
                            s,
                            &format!("Column dimensions mismatch ({} !== {cols})", items.len()),
                        ));
                    }
                }
                array = mk(NodeKind::Array { items: params });
            } else {
                if !tok(s, "]") {
                    return Err(syntax_error(s, "End of matrix ] expected"));
                }
                close_params(s);
                get_token(s)?;
                array = row;
            }
        } else {
            close_params(s);
            get_token(s)?;
            array = mk(NodeKind::Array { items: Vec::new() });
        }
        return parse_accessors(s, array);
    }
    parse_object(s)
}

fn parse_row(s: &mut State<'_>) -> Result<Node, MathError> {
    let mut params = vec![parse_assignment(s)?];
    while tok(s, ",") {
        get_token(s)?;
        params.push(parse_assignment(s)?);
    }
    Ok(mk(NodeKind::Array { items: params }))
}

fn parse_object(s: &mut State<'_>) -> Result<Node, MathError> {
    if tok(s, "{") {
        open_params(s);
        let mut properties: JsObject<Node> = JsObject::new();
        loop {
            get_token(s)?;
            if tok(s, "}") {
                break;
            }
            let key: Vec<u16> = if tok(s, "\"") {
                string_token(s, u16::from(b'"'))?.into_units()
            } else if tok(s, "'") {
                string_token(s, u16::from(b'\''))?.into_units()
            } else if s.token_type == TokenType::Symbol
                || (s.token_type == TokenType::Delimiter && named_delimiter(&s.token))
            {
                let k = s.token.clone();
                get_token(s)?;
                k
            } else {
                return Err(syntax_error(s, "Symbol or string expected as object key"));
            };
            if !tok(s, ":") {
                return Err(syntax_error(s, "Colon : expected after object key"));
            }
            get_token(s)?;
            let value = parse_assignment(s)?;
            // properties["__proto__"] = v sets the prototype, never an own property.
            if key != units("__proto__") {
                properties.insert(u(&key), value);
            }
            if !tok(s, ",") {
                break;
            }
        }
        if !tok(s, "}") {
            return Err(syntax_error(
                s,
                "Comma , or bracket } expected after object value",
            ));
        }
        close_params(s);
        get_token(s)?;
        let node = mk(NodeKind::Object {
            properties: properties.into_iter().collect(),
        });
        return parse_accessors(s, node);
    }
    parse_number(s)
}

fn parse_number(s: &mut State<'_>) -> Result<Node, MathError> {
    if s.token_type == TokenType::Number {
        let number_str = s.token.clone();
        get_token(s)?;
        return Ok(constant(Constant::Number(number_from_string(&number_str)?)));
    }
    parse_parentheses(s)
}

fn parse_parentheses(s: &mut State<'_>) -> Result<Node, MathError> {
    if tok(s, "(") {
        open_params(s);
        get_token(s)?;
        let n = parse_assignment(s)?;
        if !tok(s, ")") {
            return Err(syntax_error(s, "Parenthesis ) expected"));
        }
        close_params(s);
        get_token(s)?;
        let node = mk(NodeKind::Parenthesis {
            content: Box::new(n),
        });
        return parse_accessors(s, node);
    }
    parse_end(s)
}

fn parse_end(s: &mut State<'_>) -> Result<Node, MathError> {
    if s.token.is_empty() {
        Err(syntax_error(s, "Unexpected end of expression"))
    } else {
        Err(syntax_error(s, "Value expected"))
    }
}
