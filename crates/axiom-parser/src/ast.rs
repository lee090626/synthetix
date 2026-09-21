//! Axiom Abstract Syntax Tree (AST) definition

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    U32,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOp {
    AddWrap, // +%
    MulWrap, // *%
    Shl,     // <<
    Eq,      // ==
    Ne,      // !=
    Lt,      // <
    Le,      // <=
    Gt,      // >
    Ge,      // >=
    And,     // &&
    Or,      // ||
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecExpr {
    Ident(String),
    Number(u32),
    Bool(bool),
    Binary(BinaryOp, Box<SpecExpr>, Box<SpecExpr>),
    Let(String, Box<SpecExpr>, Box<SpecExpr>),
    If(Box<SpecExpr>, Box<SpecExpr>, Box<SpecExpr>),
    Call(String, Vec<SpecExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub exported: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub requires: Vec<SpecExpr>,
    pub ensures: Vec<SpecExpr>,
    pub requires_spans: Vec<Span>,
    pub ensures_spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub names: Vec<String>,
    pub path: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub name: String,
    pub ty: Type,
    pub initial: SpecExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageElement {
    Show {
        state: String,
    },
    Button {
        label: String,
        state: String,
        function: String,
        args: Vec<SpecExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub name: String,
    pub states: Vec<State>,
    pub elements: Vec<PageElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub imports: Vec<Import>,
    pub contracts: Vec<Contract>,
    pub pages: Vec<Page>,
}
