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
