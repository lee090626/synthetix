//! Recursive-descent and Pratt parser for Axiom modules.

use crate::ast::*;
use crate::lexer::{Lexer, Position, Token};

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    current_position: Position,
}

impl<'a> Parser<'a> {
    pub fn new(mut lexer: Lexer<'a>) -> Result<Self, String> {
        let (current, current_position) = lexer.next_token_with_position()?;
        Ok(Self {
            lexer,
            current,
            current_position,
        })
    }

    fn advance(&mut self) -> Result<Token, String> {
        let (next, next_position) = self.lexer.next_token_with_position()?;
        self.current_position = next_position;
        Ok(std::mem::replace(&mut self.current, next))
    }

    fn error(&self, message: impl std::fmt::Display) -> String {
        self.error_at(self.current_position, message)
    }

    fn error_at(&self, position: Position, message: impl std::fmt::Display) -> String {
        format!("{}:{}: {}", position.line, position.column, message)
    }

    fn expect(&mut self, expected: Token) -> Result<(), String> {
        if self.current == expected {
            self.advance()?;
            Ok(())
        } else {
            Err(self.error(format!(
                "Expected token {:?}, found {:?}",
                expected, self.current
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match &self.current {
            Token::Ident(name) => {
                let name = name.clone();
                self.advance()?;
                Ok(name)
            }
            other => Err(self.error(format!("Expected identifier, found {:?}", other))),
        }
    }

    fn next_is_record_literal(&self) -> Result<bool, String> {
        let mut lexer = self.lexer.clone();
        let (first, _) = lexer.next_token_with_position()?;
        let (second, _) = lexer.next_token_with_position()?;
        Ok(matches!(first, Token::Ident(_)) && second == Token::Colon)
    }

    /// Parses the legacy single-contract input and rejects module declarations.
    pub fn parse_contract(&mut self) -> Result<Contract, String> {
        let contract = self.parse_contract_decl(false)?;
        if self.current != Token::Eof {
            return Err(self.error(format!(
                "Unexpected token after contract: {:?}",
                self.current
            )));
        }
        Ok(contract)
    }

    /// Parses a complete `.ax` source file.
    pub fn parse_module(&mut self) -> Result<Module, String> {
        let mut imports = Vec::new();
        let mut records = Vec::new();
        let mut components = Vec::new();
        let mut contracts = Vec::new();
        let mut pages = Vec::new();

        while self.current != Token::Eof {
            match self.current {
                Token::Import => imports.push(self.parse_import()?),
                Token::Record => records.push(self.parse_record()?),
                Token::Component => components.push(self.parse_component()?),
                Token::Export => {
                    self.advance()?;
                    contracts.push(self.parse_contract_decl(true)?);
                }
                Token::Contract => contracts.push(self.parse_contract_decl(false)?),
                Token::Page => pages.push(self.parse_page()?),
                ref other => {
                    return Err(self.error(format!(
                        "Expected import, record, component, contract, or page, found {:?}",
                        other
                    )));
                }
            }
        }

        Ok(Module {
            imports,
            records,
            components,
            contracts,
            pages,
        })
    }

    fn parse_import(&mut self) -> Result<Import, String> {
        let span = Span {
            line: self.current_position.line,
            column: self.current_position.column,
        };
        self.expect(Token::Import)?;
        self.expect(Token::LBrace)?;

        let mut names = Vec::new();
        while self.current != Token::RBrace && self.current != Token::Eof {
            names.push(self.expect_ident()?);
            match self.current {
                Token::Comma => {
                    self.advance()?;
                    if self.current == Token::RBrace {
                        break;
                    }
                }
                Token::RBrace => break,
                ref other => {
                    return Err(self.error(format!(
                        "Expected ',' or '}}' in import list, found {:?}",
                        other
                    )));
                }
            }
        }
        self.expect(Token::RBrace)?;
        self.expect(Token::From)?;
        let path = match &self.current {
            Token::String(path) => {
                let path = path.clone();
                self.advance()?;
                path
            }
            other => return Err(self.error(format!("Expected import path, found {:?}", other))),
        };
        self.expect(Token::Semi)?;

        if names.is_empty() {
            return Err(self.error("import list cannot be empty"));
        }

        Ok(Import { names, path, span })
    }

    fn parse_record(&mut self) -> Result<RecordDef, String> {
        self.expect(Token::Record)?;
        let name = self.expect_ident()?;
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();
        while self.current != Token::RBrace && self.current != Token::Eof {
            let field_name = self.expect_ident()?;
            self.expect(Token::Colon)?;
            let ty = self.parse_type()?;
            self.expect(Token::Semi)?;
            fields.push(Param {
                name: field_name,
                ty,
            });
        }
        self.expect(Token::RBrace)?;
        if fields.is_empty() {
            return Err(self.error("record must have at least one field"));
        }
        Ok(RecordDef { name, fields })
    }

    fn parse_component(&mut self) -> Result<Component, String> {
        self.expect(Token::Component)?;
        let name = self.expect_ident()?;
        self.expect(Token::LBrace)?;
        let mut elements = Vec::new();
        while self.current != Token::RBrace && self.current != Token::Eof {
            elements.push(self.parse_content_element()?);
        }
        self.expect(Token::RBrace)?;
        Ok(Component { name, elements })
    }

    fn parse_contract_decl(&mut self, exported: bool) -> Result<Contract, String> {
        self.expect(Token::Contract)?;
        let name = self.expect_ident()?;

        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        while self.current != Token::RParen && self.current != Token::Eof {
            let param_name = self.expect_ident()?;
            self.expect(Token::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param {
                name: param_name,
                ty,
            });

            match self.current {
                Token::Comma => {
                    self.advance()?;
                    if self.current == Token::RParen {
                        break;
                    }
                }
                Token::RParen => break,
                ref other => {
                    return Err(self.error(format!(
                        "Expected ',' or ')' after parameter, found {:?}",
                        other
                    )));
                }
            }
        }
        self.expect(Token::RParen)?;

        self.expect(Token::Arrow)?;
        let return_type = self.parse_type()?;

        self.expect(Token::LBrace)?;
        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        let mut requires_spans = Vec::new();
        let mut ensures_spans = Vec::new();

        while self.current != Token::RBrace && self.current != Token::Eof {
            match self.current {
                Token::Requires | Token::When => {
                    requires_spans.push(Span {
                        line: self.current_position.line,
                        column: self.current_position.column,
                    });
                    self.advance()?;
                    let expr = self.parse_expr(0)?;
                    self.expect(Token::Semi)?;
                    requires.push(expr);
                }
                Token::Ensures => {
                    ensures_spans.push(Span {
                        line: self.current_position.line,
                        column: self.current_position.column,
                    });
                    self.advance()?;
                    let expr = self.parse_expr(0)?;
                    self.expect(Token::Semi)?;
                    ensures.push(expr);
                }
                _ => {
                    return Err(self.error(format!(
                        "Unexpected token inside contract body: {:?}",
                        self.current
                    )));
                }
            }
        }
        self.expect(Token::RBrace)?;

        Ok(Contract {
            exported,
            name,
            params,
            return_type,
            requires,
            ensures,
            requires_spans,
            ensures_spans,
        })
    }

    fn parse_page(&mut self) -> Result<Page, String> {
        self.expect(Token::Page)?;
        let name = self.expect_ident()?;
        self.expect(Token::LBrace)?;
        let mut states = Vec::new();
        let mut persisted = Vec::new();
        let mut reducers = Vec::new();
        let mut elements = Vec::new();

        while self.current != Token::RBrace && self.current != Token::Eof {
            match self.current {
                Token::State => {
                    self.advance()?;
                    let state_name = self.expect_ident()?;
                    self.expect(Token::Colon)?;
                    let ty = self.parse_type()?;
                    self.expect(Token::Assign)?;
                    let initial = self.parse_expr(0)?;
                    self.expect(Token::Semi)?;
                    states.push(State {
                        name: state_name,
                        ty,
                        initial,
                    });
                }
                Token::Persist => {
                    self.advance()?;
                    persisted.push(self.expect_ident()?);
                    self.expect(Token::Semi)?;
                }
                Token::Reducer => reducers.push(self.parse_reducer()?),
                Token::Input => elements.push(self.parse_input()?),
                Token::List => elements.push(self.parse_list()?),
                Token::Use => {
                    self.advance()?;
                    let name = self.expect_ident()?;
                    self.expect(Token::Semi)?;
                    elements.push(PageElement::Use { name });
                }
                Token::Show | Token::When | Token::Button => {
                    elements.push(self.parse_content_element()?);
                }
                ref other => {
                    return Err(
                        self.error(format!("Unexpected token inside page body: {:?}", other))
                    );
                }
            }
        }
        self.expect(Token::RBrace)?;

        Ok(Page {
            name,
            states,
            persisted,
            reducers,
            elements,
        })
    }

    fn parse_content_element(&mut self) -> Result<PageElement, String> {
        match self.current {
            Token::Show => self.parse_show(),
            Token::When => self.parse_when_show(),
            Token::Button => self.parse_button(),
            Token::Use => {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(Token::Semi)?;
                Ok(PageElement::Use { name })
            }
            ref other => Err(self.error(format!("Unexpected page element: {:?}", other))),
        }
    }

    fn parse_show(&mut self) -> Result<PageElement, String> {
        self.expect(Token::Show)?;
        if let Token::String(text) = &self.current {
            let text = text.clone();
            self.advance()?;
            self.expect(Token::Semi)?;
            return Ok(PageElement::ShowText { text });
        }
        let value = self.parse_expr(0)?;
        self.expect(Token::Semi)?;
        Ok(match value {
            SpecExpr::Ident(state) => PageElement::Show { state },
            value => PageElement::ShowExpr { value },
        })
    }

    fn parse_when_show(&mut self) -> Result<PageElement, String> {
        self.expect(Token::When)?;
        let condition = self.parse_expr(0)?;
        self.expect(Token::Show)?;
        let text = match &self.current {
            Token::String(text) => {
                let text = text.clone();
                self.advance()?;
                text
            }
            other => {
                return Err(self.error(format!(
                    "Expected conditional show text string, found {:?}",
                    other
                )));
            }
        };
        self.expect(Token::Semi)?;
        Ok(PageElement::WhenShow { condition, text })
    }

    fn parse_button(&mut self) -> Result<PageElement, String> {
        self.expect(Token::Button)?;
        let label = self.expect_string("button label")?;
        match self.current {
            Token::Updates => {
                self.advance()?;
                let state = self.parse_path()?;
                self.expect(Token::With)?;
                let function = self.expect_ident()?;
                let args = self.parse_call_args()?;
                self.expect(Token::Semi)?;
                Ok(PageElement::Button {
                    label,
                    state,
                    function,
                    args,
                })
            }
            Token::Adds => {
                self.advance()?;
                let state = self.expect_ident()?;
                self.expect(Token::With)?;
                let value = self.parse_expr(0)?;
                self.expect(Token::Semi)?;
                Ok(PageElement::Add {
                    label,
                    state,
                    value,
                })
            }
            Token::Runs => {
                self.advance()?;
                let reducer = self.expect_ident()?;
                self.expect(Token::Semi)?;
                Ok(PageElement::RunReducer { label, reducer })
            }
            ref other => Err(self.error(format!(
                "Expected updates, adds, or runs after button label, found {:?}",
                other
            ))),
        }
    }

    fn parse_input(&mut self) -> Result<PageElement, String> {
        self.expect(Token::Input)?;
        let state = self.expect_ident()?;
        self.expect(Token::Placeholder)?;
        let placeholder = self.expect_string("input placeholder")?;
        self.expect(Token::Semi)?;
        Ok(PageElement::Input { state, placeholder })
    }

    fn parse_list(&mut self) -> Result<PageElement, String> {
        self.expect(Token::List)?;
        let state = self.expect_ident()?;
        self.expect(Token::As)?;
        let item = self.expect_ident()?;
        self.expect(Token::LBrace)?;
        let mut elements = Vec::new();
        while self.current != Token::RBrace && self.current != Token::Eof {
            elements.push(self.parse_content_element()?);
        }
        self.expect(Token::RBrace)?;
        Ok(PageElement::List {
            state,
            item,
            elements,
        })
    }

    fn parse_reducer(&mut self) -> Result<Reducer, String> {
        self.expect(Token::Reducer)?;
        let name = self.expect_ident()?;
        self.expect(Token::LBrace)?;
        let mut updates = Vec::new();
        while self.current != Token::RBrace && self.current != Token::Eof {
            self.expect(Token::Updates)?;
            let state = self.parse_path()?;
            self.expect(Token::With)?;
            let function = self.expect_ident()?;
            let args = self.parse_call_args()?;
            self.expect(Token::Semi)?;
            updates.push(ReducerUpdate {
                state,
                function,
                args,
            });
        }
        self.expect(Token::RBrace)?;
        if updates.is_empty() {
            return Err(self.error("reducer must contain at least one update"));
        }
        Ok(Reducer { name, updates })
    }

    fn parse_path(&mut self) -> Result<String, String> {
        let mut path = self.expect_ident()?;
        while self.current == Token::Dot {
            self.advance()?;
            path.push('.');
            path.push_str(&self.expect_ident()?);
        }
        Ok(path)
    }

    fn expect_string(&mut self, description: &str) -> Result<String, String> {
        match &self.current {
            Token::String(value) => {
                let value = value.clone();
                self.advance()?;
                Ok(value)
            }
            other => Err(self.error(format!("Expected {description} string, found {:?}", other))),
        }
    }

    fn parse_type(&mut self) -> Result<Type, String> {
        if self.current == Token::List {
            self.advance()?;
            self.expect(Token::Lt)?;
            let item = self.parse_type()?;
            self.expect(Token::Gt)?;
            return Ok(Type::List(Box::new(item)));
        }
        let type_name = self.expect_ident()?;
        match type_name.as_str() {
            "u32" | "number" => Ok(Type::U32),
            "bool" => Ok(Type::Bool),
            "text" | "string" => Ok(Type::Text),
            other => Ok(Type::Record(other.to_string())),
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<SpecExpr>, String> {
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        while self.current != Token::RParen && self.current != Token::Eof {
            args.push(self.parse_expr(0)?);
            match self.current {
                Token::Comma => {
                    self.advance()?;
                    if self.current == Token::RParen {
                        break;
                    }
                }
                Token::RParen => break,
                ref other => {
                    return Err(self.error(format!(
                        "Expected ',' or ')' in argument list, found {:?}",
                        other
                    )));
                }
            }
        }
        self.expect(Token::RParen)?;
        Ok(args)
    }

    /// Pratt parser for expressions.
    fn parse_expr(&mut self, min_bp: u8) -> Result<SpecExpr, String> {
        let prefix = self.current.clone();
        let prefix_position = self.current_position;
        self.advance()?;
        let mut lhs = match prefix {
            Token::Ident(id) => {
                if self.current == Token::LParen {
                    SpecExpr::Call(id, self.parse_call_args()?)
                } else if self.current == Token::LBrace && self.next_is_record_literal()? {
                    self.advance()?;
                    let mut fields = Vec::new();
                    while self.current != Token::RBrace && self.current != Token::Eof {
                        let field = self.expect_ident()?;
                        self.expect(Token::Colon)?;
                        let value = self.parse_expr(0)?;
                        fields.push((field, value));
                        match self.current {
                            Token::Comma => {
                                self.advance()?;
                                if self.current == Token::RBrace {
                                    break;
                                }
                            }
                            Token::RBrace => break,
                            ref other => {
                                return Err(self.error(format!(
                                    "Expected ',' or '}}' in record literal, found {:?}",
                                    other
                                )));
                            }
                        }
                    }
                    self.expect(Token::RBrace)?;
                    SpecExpr::Record { name: id, fields }
                } else {
                    SpecExpr::Ident(id)
                }
            }
            Token::Number(n) => SpecExpr::Number(n),
            Token::True => SpecExpr::Bool(true),
            Token::False => SpecExpr::Bool(false),
            Token::String(value) => SpecExpr::Text(value),
            Token::LBracket => {
                let mut values = Vec::new();
                while self.current != Token::RBracket && self.current != Token::Eof {
                    values.push(self.parse_expr(0)?);
                    match self.current {
                        Token::Comma => {
                            self.advance()?;
                            if self.current == Token::RBracket {
                                break;
                            }
                        }
                        Token::RBracket => break,
                        ref other => {
                            return Err(self.error(format!(
                                "Expected ',' or ']' in list literal, found {:?}",
                                other
                            )));
                        }
                    }
                }
                self.expect(Token::RBracket)?;
                SpecExpr::List(values)
            }
            Token::Let => {
                let name = self.expect_ident()?;
                self.expect(Token::Assign)?;
                let value = self.parse_expr(0)?;
                self.expect(Token::In)?;
                let body = self.parse_expr(0)?;
                SpecExpr::Let(name, Box::new(value), Box::new(body))
            }
            Token::If => {
                let condition = self.parse_expr(0)?;
                self.expect(Token::LBrace)?;
                let then_branch = self.parse_expr(0)?;
                self.expect(Token::RBrace)?;
                self.expect(Token::Else)?;
                self.expect(Token::LBrace)?;
                let else_branch = self.parse_expr(0)?;
                self.expect(Token::RBrace)?;
                SpecExpr::If(
                    Box::new(condition),
                    Box::new(then_branch),
                    Box::new(else_branch),
                )
            }
            Token::LParen => {
                let inner = self.parse_expr(0)?;
                self.expect(Token::RParen)?;
                inner
            }
            other => {
                return Err(self.error_at(
                    prefix_position,
                    format!("Unexpected token in expression prefix: {:?}", other),
                ));
            }
        };

        loop {
            if self.current == Token::Dot {
                self.advance()?;
                let field = self.expect_ident()?;
                lhs = SpecExpr::Field(Box::new(lhs), field);
                continue;
            }
            let (op, l_bp, r_bp) = match self.current {
                Token::OrOr => (BinaryOp::Or, 1, 2),
                Token::AndAnd => (BinaryOp::And, 3, 4),
                Token::EqEq => (BinaryOp::Eq, 5, 6),
                Token::ExclEq => (BinaryOp::Ne, 5, 6),
                Token::Lt => (BinaryOp::Lt, 7, 8),
                Token::Le => (BinaryOp::Le, 7, 8),
                Token::Gt => (BinaryOp::Gt, 7, 8),
                Token::Ge => (BinaryOp::Ge, 7, 8),
                Token::AddWrap => (BinaryOp::AddWrap, 9, 10),
                Token::Shl => (BinaryOp::Shl, 11, 12),
                Token::MulWrap => (BinaryOp::MulWrap, 13, 14),
                _ => break,
            };

            if l_bp < min_bp {
                break;
            }

            self.advance()?;
            let rhs = self.parse_expr(r_bp)?;
            lhs = SpecExpr::Binary(op, Box::new(lhs), Box::new(rhs));
        }

        Ok(lhs)
    }
}
