//! Minimal Lexer for Axiom DSL

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Import,
    Export,
    From,
    Contract,
    Fn,
    Extern,
    Return,
    Requires,
    Ensures,
    If,
    Else,
    For,
    Invariant,
    Let,
    In,
    Page,
    Record,
    Component,
    Use,
    State,
    Show,
    When,
    Button,
    Updates,
    Adds,
    Runs,
    Reducer,
    Input,
    Placeholder,
    Persist,
    List,
    As,
    With,
    True,
    False,
    Ident(String),
    Number(u32),
    String(String),
    LParen,
    RParen,
    Comma,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Dot,
    Range,
    Arrow,      // ->
    Semi,       // ;
    Assign,     // =
    EqEq,       // ==
    ExclEq,     // !=
    Lt,         // <
    Le,         // <=
    Gt,         // >
    Ge,         // >=
    AndAnd,     // &&
    OrOr,       // ||
    MulWrap,    // *%
    Slash,      // /
    AddWrap,    // +%
    SubWrap,    // -%
    SubChecked, // -!
    Shl,        // <<
    BitAnd,     // &
    BitOr,      // |
    BitXor,     // ^
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone)]
pub struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().peekable(),
            line: 1,
            column: 1,
        }
    }

    pub fn position(&self) -> Position {
        Position {
            line: self.line,
            column: self.column,
        }
    }

    pub fn next_token_with_position(&mut self) -> Result<(Token, Position), String> {
        self.skip_whitespace_and_comments();
        let position = self.position();
        Ok((self.next_token()?, position))
    }

    fn take_char(&mut self) -> Option<char> {
        let character = self.chars.next()?;
        if character == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(character)
    }

    fn error(&self, message: impl std::fmt::Display) -> String {
        format!("{}:{}: {}", self.line, self.column, message)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.chars.peek() {
                Some(&c) if c.is_whitespace() => {
                    self.take_char();
                }
                Some(&'/') => {
                    let mut clone = self.chars.clone();
                    clone.next();
                    if clone.peek() == Some(&'/') {
                        // Skip line comment
                        self.take_char(); // /
                        self.take_char(); // /
                        while let Some(c) = self.take_char() {
                            if c == '\n' {
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
    }

    pub fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace_and_comments();

        let ch = match self.take_char() {
            Some(c) => c,
            None => return Ok(Token::Eof),
        };

        match ch {
            '(' => Ok(Token::LParen),
            ')' => Ok(Token::RParen),
            ',' => Ok(Token::Comma),
            '{' => Ok(Token::LBrace),
            '}' => Ok(Token::RBrace),
            '[' => Ok(Token::LBracket),
            ']' => Ok(Token::RBracket),
            ':' => Ok(Token::Colon),
            '.' => {
                if self.chars.peek() == Some(&'.') {
                    self.take_char();
                    Ok(Token::Range)
                } else {
                    Ok(Token::Dot)
                }
            }
            ';' => Ok(Token::Semi),
            '/' => Ok(Token::Slash),
            '-' => {
                if self.chars.peek() == Some(&'>') {
                    self.take_char();
                    Ok(Token::Arrow)
                } else if self.chars.peek() == Some(&'%') {
                    self.take_char();
                    Ok(Token::SubWrap)
                } else if self.chars.peek() == Some(&'!') {
                    self.take_char();
                    Ok(Token::SubChecked)
                } else {
                    Err(self.error("Unexpected '-' (use '-%' for wrapping sub)"))
                }
            }
            '=' => {
                if self.chars.peek() == Some(&'=') {
                    self.take_char();
                    Ok(Token::EqEq)
                } else {
                    Ok(Token::Assign)
                }
            }
            '!' => {
                if self.chars.peek() == Some(&'=') {
                    self.take_char();
                    Ok(Token::ExclEq)
                } else {
                    Err(self.error("Unexpected '!' (expected '!=')"))
                }
            }
            '&' => {
                if self.chars.peek() == Some(&'&') {
                    self.take_char();
                    Ok(Token::AndAnd)
                } else {
                    Ok(Token::BitAnd)
                }
            }
            '|' => {
                if self.chars.peek() == Some(&'|') {
                    self.take_char();
                    Ok(Token::OrOr)
                } else {
                    Ok(Token::BitOr)
                }
            }
            '^' => Ok(Token::BitXor),
            '*' => {
                if self.chars.peek() == Some(&'%') {
                    self.take_char();
                    Ok(Token::MulWrap)
                } else {
                    Err(self.error("Unexpected '*' (use '*%' for wrapping mul)"))
                }
            }
            '+' => {
                if self.chars.peek() == Some(&'%') {
                    self.take_char();
                    Ok(Token::AddWrap)
                } else {
                    Err(self.error("Unexpected '+' (use '+%' for wrapping add)"))
                }
            }
            '<' => {
                if self.chars.peek() == Some(&'<') {
                    self.take_char();
                    Ok(Token::Shl)
                } else if self.chars.peek() == Some(&'=') {
                    self.take_char();
                    Ok(Token::Le)
                } else {
                    Ok(Token::Lt)
                }
            }
            '>' => {
                if self.chars.peek() == Some(&'=') {
                    self.take_char();
                    Ok(Token::Ge)
                } else {
                    Ok(Token::Gt)
                }
            }
            c if c.is_ascii_digit() => {
                let mut num_str = String::new();
                num_str.push(c);
                while let Some(&next_c) = self.chars.peek() {
                    if next_c.is_ascii_digit() {
                        num_str.push(self.take_char().unwrap());
                    } else {
                        break;
                    }
                }
                let val: u32 = num_str
                    .parse()
                    .map_err(|e| self.error(format!("Invalid integer: {e}")))?;
                Ok(Token::Number(val))
            }
            '"' => {
                let mut value = String::new();
                loop {
                    match self.take_char() {
                        Some('"') => break,
                        Some('\\') => match self.take_char() {
                            Some('"') => value.push('"'),
                            Some('\\') => value.push('\\'),
                            Some('n') => value.push('\n'),
                            Some('r') => value.push('\r'),
                            Some('t') => value.push('\t'),
                            Some(other) => {
                                return Err(
                                    self.error(format!("Unsupported escape sequence: \\{other}"))
                                );
                            }
                            None => return Err(self.error("Unterminated string literal")),
                        },
                        Some('\n') | None => return Err(self.error("Unterminated string literal")),
                        Some(other) => value.push(other),
                    }
                }
                Ok(Token::String(value))
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut ident = String::new();
                ident.push(c);
                while let Some(&next_c) = self.chars.peek() {
                    if next_c.is_alphanumeric() || next_c == '_' {
                        ident.push(self.take_char().unwrap());
                    } else {
                        break;
                    }
                }
                match ident.as_str() {
                    "import" => Ok(Token::Import),
                    "export" => Ok(Token::Export),
                    "from" => Ok(Token::From),
                    "contract" | "program" => Ok(Token::Contract),
                    "fn" => Ok(Token::Fn),
                    "extern" => Ok(Token::Extern),
                    "return" => Ok(Token::Return),
                    "requires" => Ok(Token::Requires),
                    "ensures" | "gives" => Ok(Token::Ensures),
                    "if" => Ok(Token::If),
                    "else" => Ok(Token::Else),
                    "for" => Ok(Token::For),
                    "invariant" => Ok(Token::Invariant),
                    "let" => Ok(Token::Let),
                    "in" => Ok(Token::In),
                    "page" => Ok(Token::Page),
                    "record" => Ok(Token::Record),
                    "component" => Ok(Token::Component),
                    "use" => Ok(Token::Use),
                    "state" => Ok(Token::State),
                    "show" => Ok(Token::Show),
                    "when" => Ok(Token::When),
                    "button" => Ok(Token::Button),
                    "updates" => Ok(Token::Updates),
                    "adds" => Ok(Token::Adds),
                    "runs" => Ok(Token::Runs),
                    "reducer" => Ok(Token::Reducer),
                    "input" => Ok(Token::Input),
                    "placeholder" => Ok(Token::Placeholder),
                    "persist" => Ok(Token::Persist),
                    "list" => Ok(Token::List),
                    "as" => Ok(Token::As),
                    "with" => Ok(Token::With),
                    "true" => Ok(Token::True),
                    "false" => Ok(Token::False),
                    "equals" | "is" => Ok(Token::EqEq),
                    "not_equals" | "is_not" => Ok(Token::ExclEq),
                    "and" => Ok(Token::AndAnd),
                    "or" => Ok(Token::OrOr),
                    "plus" => Ok(Token::AddWrap),
                    "minus" => Ok(Token::SubWrap),
                    "times" => Ok(Token::MulWrap),
                    "shift_left" => Ok(Token::Shl),
                    "answer" => Ok(Token::Ident("result".to_string())),
                    _ => Ok(Token::Ident(ident)),
                }
            }
            other => Err(self.error(format!("Unexpected character: '{other}'"))),
        }
    }
}
