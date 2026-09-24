//! Axiom Abstract Syntax Tree (AST) definition

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    U32,
    I32,
    Bool,
    FixedArray(Box<Type>, usize),
    Text,
    Record(String),
    List(Box<Type>),
    Unknown,
}

/// The largest fixed array accepted by the v6.1 proof core.
pub const MAX_VERIFIED_ARRAY_LEN: usize = 16;
pub const MAX_VERIFIED_LOOP_ITERATIONS: u32 = 16;
pub const MAX_VERIFIED_RECURSION_DEPTH: u32 = 16;
/// Bump when verified expression, type, or lowering semantics change.
pub const VERIFIED_SEMANTICS_VERSION: &str = "v6.3-bounded-program-model";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    Contract,
    Function,
    Extern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOp {
    AddWrap, // +%
    SubWrap, // -%
    MulWrap, // *%
    Shl,     // <<
    BitAnd,  // &
    BitOr,   // |
    BitXor,  // ^
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
    Text(String),
    List(Vec<SpecExpr>),
    Record {
        name: String,
        fields: Vec<(String, SpecExpr)>,
    },
    Field(Box<SpecExpr>, String),
    Index(Box<SpecExpr>, Box<SpecExpr>),
    ArrayUpdate(Box<SpecExpr>, Box<SpecExpr>, Box<SpecExpr>),
    For {
        index: String,
        start: u32,
        end: u32,
        accumulator: String,
        initial: Box<SpecExpr>,
        invariant: Box<SpecExpr>,
        body: Box<SpecExpr>,
    },
    Binary(BinaryOp, Box<SpecExpr>, Box<SpecExpr>),
    Let(String, Box<SpecExpr>, Box<SpecExpr>),
    If(Box<SpecExpr>, Box<SpecExpr>, Box<SpecExpr>),
    Call(String, Vec<SpecExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub kind: DeclarationKind,
    pub exported: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub implementation: Option<SpecExpr>,
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
pub struct RecordDef {
    pub name: String,
    pub fields: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reducer {
    pub name: String,
    pub updates: Vec<ReducerUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducerUpdate {
    pub state: String,
    pub function: String,
    pub args: Vec<SpecExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub elements: Vec<PageElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageElement {
    Show {
        state: String,
    },
    ShowExpr {
        value: SpecExpr,
    },
    ShowText {
        text: String,
    },
    WhenShow {
        condition: SpecExpr,
        text: String,
    },
    Button {
        label: String,
        state: String,
        function: String,
        args: Vec<SpecExpr>,
    },
    RunReducer {
        label: String,
        reducer: String,
    },
    Input {
        state: String,
        placeholder: String,
    },
    List {
        state: String,
        item: String,
        elements: Vec<PageElement>,
    },
    Add {
        label: String,
        state: String,
        value: SpecExpr,
    },
    Use {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub name: String,
    pub states: Vec<State>,
    pub persisted: Vec<String>,
    pub reducers: Vec<Reducer>,
    pub elements: Vec<PageElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub imports: Vec<Import>,
    pub records: Vec<RecordDef>,
    pub components: Vec<Component>,
    pub contracts: Vec<Contract>,
    pub pages: Vec<Page>,
}
