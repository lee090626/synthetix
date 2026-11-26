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
pub const VERIFIED_SEMANTICS_VERSION: &str = "v6.4-wide-words";

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
    AddWrap,    // +%
    SubWrap,    // -%
    SubChecked, // -! (must not go below 0; proven by the verifier)
    MulWrap,    // *%
    Div,        // / (unsigned, rounds down; divisor must be non-zero)
    Rem,        // % (unsigned remainder; divisor must be non-zero)
    Shl,        // <<
    BitAnd,     // &
    BitOr,      // |
    BitXor,     // ^
    Eq,         // ==
    Ne,         // !=
    Lt,         // <
    Le,         // <=
    Gt,         // >
    Ge,         // >=
    And,        // &&
    Or,         // ||
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecExpr {
    Ident(String),
    Number(u64),
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
    /// Decimal places of every number (`with 2 decimals`); values are stored
    /// as whole numbers times 10^decimals. 0 means whole numbers.
    pub decimals: u32,
    /// Text answers: the rule returns `n` for `labels[n]`. Empty for numbers.
    pub labels: Vec<String>,
    /// 64-bit words (`with big numbers`): `u32`/`i32` values, literals and
    /// arithmetic are 64 bits wide in this contract.
    pub wide: bool,
    /// Set on the core of a rule over lists of any length (`prices[n]`).
    pub list: Option<ListRule>,
    /// `hint mul_div_le(a, b, c)` lines: proven facts the Lean proof uses
    /// (issue #215). Z3 and synthesis ignore them.
    pub hints: Vec<Hint>,
    /// Set on the step of a `repeat` rule (issue #214).
    pub repeat: Option<RepeatRule>,
}

/// A rule that applies a step a number of times (`repeat years`, issue
/// #214), split into `name__start(inputs)` and `name__step(inputs, value)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatRule {
    /// The rule name the user wrote.
    pub name: String,
    /// The start rule's name.
    pub start: String,
    /// Which input is the number of steps.
    pub count: usize,
    /// The `need count <= max` limit.
    pub max: u64,
}

/// A `hint` line: a lemma from `axiom_verify::lean::LEMMAS` and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub lemma: String,
    pub args: Vec<SpecExpr>,
}

/// A rule over lists of any length (`cart(prices[n])`, issue #182), split
/// into one item rule per `sum(...)`, `min(...)` or `max(...)` and a core
/// rule. Item rules take every input, with one item of each list; the core
/// takes the inputs that are not lists, then the length, then each result, in
/// `items` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRule {
    /// The rule name the user wrote.
    pub name: String,
    /// The inputs in order; `true` marks a list.
    pub params: Vec<(String, bool)>,
    /// Item rules, one per `sum(...)`, `min(...)` or `max(...)`.
    pub items: Vec<String>,
    /// How each item rule's answers are combined, in `items` order.
    pub folds: Vec<Fold>,
    /// The `need n <= max` limit on the length.
    pub max: u64,
    /// The `need n >= least` limit on the length (0 when there is none).
    pub least: u64,
    /// The answer is a list: the core takes every input (one item of each
    /// list) before the length and sums, and runs once per item.
    pub each: bool,
    /// For each item rule, the item rules (indexes into `items`, always plain
    /// `sum(list)`) whose totals it takes after the inputs: `sum(w / sum(w))`.
    /// Those are added up in a first loop, the rest in a second.
    pub uses: Vec<Vec<usize>>,
    /// The core takes `position` (the item's place, from 1) after the length.
    pub position: bool,
}

/// How a list rule combines one item rule's answers over the items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    Sum,
    /// Needs at least one item (`need n >= 1`).
    Min,
    Max,
}

impl Fold {
    /// The word the user writes, also the core's input name before its number.
    pub fn word(self) -> &'static str {
        match self {
            Fold::Sum => "sum",
            Fold::Min => "min",
            Fold::Max => "max",
        }
    }
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
    /// Decimal places shown and typed for this value (see `Contract::decimals`).
    pub decimals: u32,
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
    /// `show transfers(sends, names)`: one line per non-zero `sends` item,
    /// item (i-1)*n+j reading "name i → name j amount".
    Transfers {
        sends: Vec<String>,
        names: Vec<String>,
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
