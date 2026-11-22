//! The expression tree mathjs 11.3.0's `parse` builds (`lib/esm/expression/node/*.js`).

use asrs_js::string::JsString;

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    /// `node.comment`, which `parseBlock` sets only when the statement had a `#` comment.
    pub comment: Option<JsString>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Number(f64),
    String(JsString),
    Boolean(bool),
    Null,
    Undefined,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Constant(Constant),
    Symbol {
        name: JsString,
    },
    Operator {
        op: &'static str,
        fn_name: &'static str,
        implicit: bool,
        is_percentage: bool,
        args: Vec<Node>,
    },
    Parenthesis {
        content: Box<Node>,
    },
    Function {
        callee: Box<Node>,
        args: Vec<Node>,
    },
    Accessor {
        object: Box<Node>,
        index: Box<Node>,
    },
    Index {
        dimensions: Vec<Node>,
        dot_notation: bool,
    },
    Array {
        items: Vec<Node>,
    },
    /// Properties in JS property order (`Object.keys` of the parsed object literal).
    Object {
        properties: Vec<(JsString, Node)>,
    },
    Assignment {
        object: Box<Node>,
        index: Option<Box<Node>>,
        value: Box<Node>,
    },
    FunctionAssignment {
        name: JsString,
        params: Vec<JsString>,
        types: Vec<JsString>,
        expr: Box<Node>,
    },
    Conditional {
        condition: Box<Node>,
        true_expr: Box<Node>,
        false_expr: Box<Node>,
    },
    Range {
        start: Box<Node>,
        end: Box<Node>,
        step: Option<Box<Node>>,
    },
    Relational {
        conditionals: Vec<&'static str>,
        params: Vec<Node>,
    },
    Block {
        blocks: Vec<(Node, bool)>,
    },
}

impl Node {
    pub fn new(kind: NodeKind) -> Node {
        Node {
            kind,
            comment: None,
        }
    }

    /// mathjs's `node.type`.
    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            NodeKind::Constant(_) => "ConstantNode",
            NodeKind::Symbol { .. } => "SymbolNode",
            NodeKind::Operator { .. } => "OperatorNode",
            NodeKind::Parenthesis { .. } => "ParenthesisNode",
            NodeKind::Function { .. } => "FunctionNode",
            NodeKind::Accessor { .. } => "AccessorNode",
            NodeKind::Index { .. } => "IndexNode",
            NodeKind::Array { .. } => "ArrayNode",
            NodeKind::Object { .. } => "ObjectNode",
            NodeKind::Assignment { .. } => "AssignmentNode",
            NodeKind::FunctionAssignment { .. } => "FunctionAssignmentNode",
            NodeKind::Conditional { .. } => "ConditionalNode",
            NodeKind::Range { .. } => "RangeNode",
            NodeKind::Relational { .. } => "RelationalNode",
            NodeKind::Block { .. } => "BlockNode",
        }
    }
}

/// Long operator chains parse into trees as deep as the chain is long, without recursion, so the
/// tree is taken apart iteratively instead of by recursive drop glue.
impl Drop for Node {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_children(&mut self.kind, &mut pending);
        while let Some(mut node) = pending.pop() {
            take_children(&mut node.kind, &mut pending);
        }
    }
}

fn take_children(kind: &mut NodeKind, out: &mut Vec<Node>) {
    let mut take = |child: &mut Box<Node>| {
        out.push(std::mem::replace(
            child.as_mut(),
            Node::new(NodeKind::Constant(Constant::Undefined)),
        ));
    };
    match kind {
        NodeKind::Constant(_) | NodeKind::Symbol { .. } => {}
        NodeKind::Parenthesis { content } => take(content),
        NodeKind::Accessor { object, index } => {
            take(object);
            take(index);
        }
        NodeKind::Assignment {
            object,
            index,
            value,
        } => {
            take(object);
            if let Some(index) = index {
                take(index);
            }
            take(value);
        }
        NodeKind::FunctionAssignment { expr, .. } => take(expr),
        NodeKind::Conditional {
            condition,
            true_expr,
            false_expr,
        } => {
            take(condition);
            take(true_expr);
            take(false_expr);
        }
        NodeKind::Range { start, end, step } => {
            take(start);
            take(end);
            if let Some(step) = step {
                take(step);
            }
        }
        NodeKind::Function { callee, args } => {
            take(callee);
            out.append(args);
        }
        NodeKind::Operator { args, .. } => out.append(args),
        NodeKind::Index { dimensions, .. } => out.append(dimensions),
        NodeKind::Array { items } => out.append(items),
        NodeKind::Relational { params, .. } => out.append(params),
        NodeKind::Object { properties } => out.extend(properties.drain(..).map(|(_, v)| v)),
        NodeKind::Block { blocks } => out.extend(blocks.drain(..).map(|(n, _)| n)),
    }
}
