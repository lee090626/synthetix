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
        let mut contracts = Vec::new();
        let mut pages = Vec::new();

        while self.current != Token::Eof {
            match self.current {
                Token::Import => imports.push(self.parse_import()?),
                Token::Export => {
                    self.advance()?;
                    contracts.push(self.parse_contract_decl(true)?);
                }
                Token::Contract => contracts.push(self.parse_contract_decl(false)?),
                Token::Page => pages.push(self.parse_page()?),
                ref other => {
                    return Err(self.error(format!(
                        "Expected import, contract, or page, found {:?}",
                        other
                    )));
                }
            }
        }

        Ok(Module {
            imports,
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
                Token::Requires => {
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
                Token::Show => {
                    self.advance()?;
                    let state = self.expect_ident()?;
                    self.expect(Token::Semi)?;
                    elements.push(PageElement::Show { state });
                }
                Token::Button => {
                    self.advance()?;
                    let label = match &self.current {
                        Token::String(label) => {
                            let label = label.clone();
                            self.advance()?;
                            label
                        }
                        other => {
                            return Err(self.error(format!(
                                "Expected button label string, found {:?}",
                                other
                            )));
                        }
                    };
                    self.expect(Token::Updates)?;
                    let state = self.expect_ident()?;
                    self.expect(Token::With)?;
                    let function = self.expect_ident()?;
                    let args = self.parse_call_args()?;
                    self.expect(Token::Semi)?;
                    elements.push(PageElement::Button {
                        label,
                        state,
                        function,
                        args,
                    });
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
            elements,
        })
    }

    fn parse_type(&mut self) -> Result<Type, String> {
        let type_name = self.expect_ident()?;
        match type_name.as_str() {
            "u32" | "number" => Ok(Type::U32),
            "bool" => Ok(Type::Bool),
            other => Err(self.error(format!("Unsupported type: {other}"))),
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
                } else {
                    SpecExpr::Ident(id)
                }
            }
            Token::Number(n) => SpecExpr::Number(n),
            Token::True => SpecExpr::Bool(true),
            Token::False => SpecExpr::Bool(false),
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
