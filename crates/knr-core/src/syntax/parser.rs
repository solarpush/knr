use super::ast::*;
use super::lexer::Lexer;
use super::token::{Token, TokenKind};
use crate::error::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest = 0,
    Or = 1,             // ||
    And = 2,            // &&
    Equality = 3,       // ==, !=
    Relational = 4,     // <, <=, >, >=
    Additive = 5,       // +, -
    Multiplicative = 6, // *, /, %
    Prefix = 7,         // -, !
    Postfix = 8,        // call (), member ., index []
}

impl Precedence {
    pub fn of_binary(kind: &TokenKind) -> Option<(Precedence, BinaryOp)> {
        match kind {
            TokenKind::OrOr => Some((Precedence::Or, BinaryOp::Or)),
            TokenKind::AndAnd => Some((Precedence::And, BinaryOp::And)),
            TokenKind::EqEq => Some((Precedence::Equality, BinaryOp::Eq)),
            TokenKind::BangEq => Some((Precedence::Equality, BinaryOp::NotEq)),
            TokenKind::Lt => Some((Precedence::Relational, BinaryOp::Lt)),
            TokenKind::LtEq => Some((Precedence::Relational, BinaryOp::LtEq)),
            TokenKind::Gt => Some((Precedence::Relational, BinaryOp::Gt)),
            TokenKind::GtEq => Some((Precedence::Relational, BinaryOp::GtEq)),
            TokenKind::Plus => Some((Precedence::Additive, BinaryOp::Add)),
            TokenKind::Minus => Some((Precedence::Additive, BinaryOp::Sub)),
            TokenKind::Star => Some((Precedence::Multiplicative, BinaryOp::Mul)),
            TokenKind::Slash => Some((Precedence::Multiplicative, BinaryOp::Div)),
            TokenKind::Percent => Some((Precedence::Multiplicative, BinaryOp::Rem)),
            _ => None,
        }
    }
}

pub struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    pub fn new(src: &str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize_all()?;
        Ok(Self { tokens, cursor: 0 })
    }

    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let start_span = self.current_span();
        let mut imports = Vec::new();
        let mut declarations = Vec::new();

        while !self.is_at_end() {
            if self.check(&TokenKind::Import) {
                imports.push(self.parse_import()?);
            } else if self.check(&TokenKind::Struct) || self.check(&TokenKind::Schema) {
                declarations.push(Declaration::Struct(self.parse_struct_def()?));
            } else if self.check(&TokenKind::Enum) {
                declarations.push(Declaration::Enum(self.parse_enum_def()?));
            } else if self.check(&TokenKind::Fn) {
                declarations.push(Declaration::Function(self.parse_function_def()?));
            } else {
                let tok = self.peek();
                return Err(ParseError::InvalidSyntax {
                    line: tok.span.line,
                    msg: format!("col {}: unexpected token {:?}", tok.span.col, tok.kind),
                });
            }
        }

        let end_span = self.current_span();
        Ok(Program {
            imports,
            declarations,
            span: Span::merge(start_span, end_span),
        })
    }

    fn parse_import(&mut self) -> Result<ImportDecl, ParseError> {
        let start_span = self.expect(&TokenKind::Import)?.span;
        let tok = self.advance();
        let path = match tok.kind {
            TokenKind::StrLit(s) => s,
            _ => {
                return Err(ParseError::InvalidSyntax {
                    line: tok.span.line,
                    msg: format!("col {}: expected string literal after import", tok.span.col),
                });
            }
        };
        let end_span = tok.span;
        // Optional semicolon
        if self.check(&TokenKind::Semicolon) {
            self.advance();
        }
        Ok(ImportDecl {
            path,
            span: Span::merge(start_span, end_span),
        })
    }

    fn parse_struct_def(&mut self) -> Result<StructDef, ParseError> {
        let start_tok = self.advance(); // consume struct or schema
        let name = self.parse_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let field = self.parse_field_def()?;
            fields.push(field);
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        let end_tok = self.expect(&TokenKind::RBrace)?;
        Ok(StructDef {
            name,
            fields,
            span: Span::merge(start_tok.span, end_tok.span),
        })
    }

    fn parse_field_def(&mut self) -> Result<FieldDef, ParseError> {
        let start_span = self.current_span();
        let mut index = None;

        // Check if there's an index: `0: field_name`
        if let TokenKind::IntLit(idx) = self.peek().kind {
            if self.peek_nth(1).map(|t| &t.kind) == Some(&TokenKind::Colon) {
                index = Some(idx as u32);
                self.advance(); // consume int
                self.advance(); // consume colon
            }
        }

        let name = self.parse_ident()?;

        // Optional colon between name and type: `name: Type` or `name Type`
        if self.check(&TokenKind::Colon) {
            self.advance();
        }

        let ty = self.parse_type_expr()?;
        let end_span = ty.span;

        Ok(FieldDef {
            index,
            name,
            ty,
            span: Span::merge(start_span, end_span),
        })
    }

    fn parse_enum_def(&mut self) -> Result<EnumDef, ParseError> {
        let start_tok = self.expect(&TokenKind::Enum)?;
        let name = self.parse_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let v_name = self.parse_ident()?;
            let mut end_span = v_name.span;
            let mut fields = Vec::new();
            if self.check(&TokenKind::LParen) {
                self.advance();
                while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                    fields.push(self.parse_type_expr()?);
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                end_span = self.expect(&TokenKind::RParen)?.span;
            }
            variants.push(EnumVariantDef {
                span: Span::merge(v_name.span, end_span),
                name: v_name,
                fields,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        let end_tok = self.expect(&TokenKind::RBrace)?;
        Ok(EnumDef {
            name,
            variants,
            span: Span::merge(start_tok.span, end_tok.span),
        })
    }

    fn parse_function_def(&mut self) -> Result<FunctionDef, ParseError> {
        let start_tok = self.expect(&TokenKind::Fn)?;
        let name = self.parse_ident()?;

        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_at_end() {
            let p_start = self.current_span();
            let p_name = self.parse_ident()?;
            self.expect(&TokenKind::Colon)?;
            let is_borrowed = if self.check(&TokenKind::Amp) {
                self.advance();
                true
            } else {
                false
            };
            let p_ty = self.parse_type_expr()?;
            let p_span = Span::merge(p_start, p_ty.span);
            params.push(ParamDef {
                name: p_name,
                ty: p_ty,
                is_borrowed,
                span: p_span,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;

        // Return type
        let ret_ty = if self.check(&TokenKind::Arrow) {
            self.advance();
            self.parse_type_expr()?
        } else {
            TypeExpr::new(
                TypeExprKind::Named(Ident::new("void", self.current_span())),
                self.current_span(),
            )
        };

        // Effects
        let mut effects = Vec::new();
        if self.check(&TokenKind::Effects) {
            self.advance();
            self.expect(&TokenKind::LParen)?;
            while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                effects.push(self.parse_ident()?);
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(&TokenKind::RParen)?;
        }

        let body = self.parse_block()?;
        let end_span = body.span;

        Ok(FunctionDef {
            name,
            params,
            ret_ty,
            effects,
            body,
            span: Span::merge(start_tok.span, end_span),
        })
    }

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        let start_tok = self.expect(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail_expr = None;

        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            if self.check(&TokenKind::Let) {
                stmts.push(self.parse_let_stmt()?);
            } else if self.check(&TokenKind::Return) {
                stmts.push(self.parse_return_stmt()?);
            } else if self.check(&TokenKind::If) {
                stmts.push(self.parse_if_stmt()?);
            } else if self.check(&TokenKind::While) {
                stmts.push(self.parse_while_stmt()?);
            } else if self.check(&TokenKind::Break) {
                let tok = self.advance();
                let end = self.expect(&TokenKind::Semicolon)?.span;
                stmts.push(Stmt {
                    kind: StmtKind::Break,
                    span: Span::merge(tok.span, end),
                });
            } else if self.check(&TokenKind::Continue) {
                let tok = self.advance();
                let end = self.expect(&TokenKind::Semicolon)?.span;
                stmts.push(Stmt {
                    kind: StmtKind::Continue,
                    span: Span::merge(tok.span, end),
                });
            } else if self.check(&TokenKind::Match) {
                let match_expr = self.parse_match_expr()?;
                if self.check(&TokenKind::Semicolon) {
                    self.advance();
                    let span = match_expr.span;
                    stmts.push(Stmt {
                        kind: StmtKind::Expr(match_expr),
                        span,
                    });
                } else if self.check(&TokenKind::RBrace) {
                    tail_expr = Some(Box::new(match_expr));
                    break;
                } else {
                    let span = match_expr.span;
                    stmts.push(Stmt {
                        kind: StmtKind::Expr(match_expr),
                        span,
                    });
                }
            } else if self.check(&TokenKind::Syscall) {
                stmts.push(self.parse_syscall_stmt()?);
            } else if self.check(&TokenKind::LBrace) {
                let blk = self.parse_block()?;
                let span = blk.span;
                stmts.push(Stmt {
                    kind: StmtKind::Block(blk),
                    span,
                });
            } else {
                // Expression or assignment or tail expression
                let expr = self.parse_expr(Precedence::Lowest, true)?;
                if self.check(&TokenKind::Assign) {
                    self.advance();
                    let val = self.parse_expr(Precedence::Lowest, true)?;
                    self.expect(&TokenKind::Semicolon)?;
                    let span = Span::merge(expr.span, val.span);
                    stmts.push(Stmt {
                        kind: StmtKind::Assign {
                            target: expr,
                            value: val,
                        },
                        span,
                    });
                } else if self.check(&TokenKind::Semicolon) {
                    self.advance();
                    let span = expr.span;
                    stmts.push(Stmt {
                        kind: StmtKind::Expr(expr),
                        span,
                    });
                } else if self.check(&TokenKind::RBrace) {
                    tail_expr = Some(Box::new(expr));
                    break;
                } else {
                    let tok = self.peek();
                    return Err(ParseError::InvalidSyntax {
                        line: tok.span.line,
                        msg: format!("col {}: expected ';' after expression", tok.span.col),
                    });
                }
            }
        }

        let end_tok = self.expect(&TokenKind::RBrace)?;
        Ok(Block {
            stmts,
            tail_expr,
            span: Span::merge(start_tok.span, end_tok.span),
        })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start_tok = self.expect(&TokenKind::Let)?;
        let name = self.parse_ident()?;

        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        self.expect(&TokenKind::Assign)?;
        let init = self.parse_expr(Precedence::Lowest, true)?;
        self.expect(&TokenKind::Semicolon)?;

        let span = Span::merge(start_tok.span, init.span);
        Ok(Stmt {
            kind: StmtKind::Let { name, ty, init },
            span,
        })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start_tok = self.expect(&TokenKind::Return)?;
        let value = if !self.check(&TokenKind::Semicolon) {
            Some(self.parse_expr(Precedence::Lowest, true)?)
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon)?;

        let end_span = value.as_ref().map(|v| v.span).unwrap_or(start_tok.span);
        Ok(Stmt {
            kind: StmtKind::Return { value },
            span: Span::merge(start_tok.span, end_span),
        })
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start_tok = self.expect(&TokenKind::If)?;
        // Struct literals disallowed in condition to prevent consuming '{'
        let cond = self.parse_expr(Precedence::Lowest, false)?;
        let then_block = self.parse_block()?;

        let else_block = if self.check(&TokenKind::Else) {
            self.advance();
            if self.check(&TokenKind::If) {
                // Else if: wrap as nested if statement inside a synthetic block
                let nested_if = self.parse_if_stmt()?;
                let span = nested_if.span;
                Some(Block {
                    stmts: vec![nested_if],
                    tail_expr: None,
                    span,
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        let end_span = else_block.as_ref().map(|b| b.span).unwrap_or(then_block.span);
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then_block,
                else_block,
            },
            span: Span::merge(start_tok.span, end_span),
        })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start_tok = self.expect(&TokenKind::While)?;
        let cond = self.parse_expr(Precedence::Lowest, false)?;
        let body = self.parse_block()?;

        let end_span = body.span;
        Ok(Stmt {
            kind: StmtKind::While { cond, body },
            span: Span::merge(start_tok.span, end_span),
        })
    }

    fn parse_syscall_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start_tok = self.expect(&TokenKind::Syscall)?;
        let effect = self.parse_ident()?;
        let op_tok = self.advance();
        let op = match op_tok.kind {
            TokenKind::StrLit(s) => s,
            _ => {
                return Err(ParseError::InvalidSyntax {
                    line: op_tok.span.line,
                    msg: format!("col {}: expected string literal operation name in syscall", op_tok.span.col),
                });
            }
        };
        let payload = self.parse_expr(Precedence::Lowest, true)?;
        self.expect(&TokenKind::Semicolon)?;

        let span = Span::merge(start_tok.span, payload.span);
        Ok(Stmt {
            kind: StmtKind::Syscall {
                effect,
                op,
                payload,
            },
            span,
        })
    }

    pub fn parse_expr(&mut self, prec: Precedence, struct_allowed: bool) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_prefix(struct_allowed)?;

        while !self.is_at_end() {
            // Postfix: function call ()
            if self.check(&TokenKind::LParen) {
                if prec >= Precedence::Postfix {
                    break;
                }
                lhs = self.parse_call(lhs)?;
                continue;
            }

            // Postfix: field access .ident
            if self.check(&TokenKind::Dot) {
                if prec >= Precedence::Postfix {
                    break;
                }
                self.advance();
                let field = self.parse_ident()?;
                let span = Span::merge(lhs.span, field.span);
                lhs = Expr {
                    kind: ExprKind::FieldAccess {
                        target: Box::new(lhs),
                        field,
                    },
                    span,
                };
                continue;
            }

            // Postfix: indexing [index]
            if self.check(&TokenKind::LBracket) {
                if prec >= Precedence::Postfix {
                    break;
                }
                self.advance();
                let index = self.parse_expr(Precedence::Lowest, true)?;
                let end_tok = self.expect(&TokenKind::RBracket)?;
                let span = Span::merge(lhs.span, end_tok.span);
                lhs = Expr {
                    kind: ExprKind::Index {
                        target: Box::new(lhs),
                        index: Box::new(index),
                    },
                    span,
                };
                continue;
            }

            // Infix binary operators
            let tok = self.peek();
            if let Some((op_prec, bin_op)) = Precedence::of_binary(&tok.kind) {
                if op_prec <= prec {
                    break;
                }
                self.advance(); // consume operator
                let rhs = self.parse_expr(op_prec, struct_allowed)?;
                let span = Span::merge(lhs.span, rhs.span);
                lhs = Expr {
                    kind: ExprKind::Binary {
                        op: bin_op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                };
                continue;
            }

            break;
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self, struct_allowed: bool) -> Result<Expr, ParseError> {
        let tok = self.peek().clone();

        // Unary prefix: -, !
        if tok.kind == TokenKind::Minus {
            self.advance();
            let operand = self.parse_expr(Precedence::Prefix, struct_allowed)?;
            let span = Span::merge(tok.span, operand.span);
            return Ok(Expr {
                kind: ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                },
                span,
            });
        }

        if tok.kind == TokenKind::Bang {
            self.advance();
            let operand = self.parse_expr(Precedence::Prefix, struct_allowed)?;
            let span = Span::merge(tok.span, operand.span);
            return Ok(Expr {
                kind: ExprKind::Unary {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                },
                span,
            });
        }

        // Borrow prefix: &expr
        if tok.kind == TokenKind::Amp {
            self.advance();
            let operand = self.parse_expr(Precedence::Prefix, struct_allowed)?;
            let span = Span::merge(tok.span, operand.span);
            return Ok(Expr {
                kind: ExprKind::Borrow(Box::new(operand)),
                span,
            });
        }

        // Literals
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Int(v),
                    span: tok.span,
                })
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Float(v),
                    span: tok.span,
                })
            }
            TokenKind::StrLit(ref s) => {
                let s_val = s.clone();
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Str(s_val),
                    span: tok.span,
                })
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    span: tok.span,
                })
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    span: tok.span,
                })
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr(Precedence::Lowest, true)?;
                self.expect(&TokenKind::RParen)?;
                Ok(inner)
            }
            TokenKind::LBracket => self.parse_array_lit(),
            TokenKind::LBrace => {
                let blk = self.parse_block()?;
                let span = blk.span;
                Ok(Expr {
                    kind: ExprKind::Block(blk),
                    span,
                })
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::Syscall => self.parse_syscall_expr(),
            TokenKind::Ident(ref name) => {
                let ident = Ident::new(name, tok.span);
                self.advance();

                // Enum variant literal: EnumName::Variant or EnumName::Variant(args...)
                if self.check(&TokenKind::ColonColon) {
                    self.advance();
                    let variant = self.parse_ident()?;
                    let mut end_span = variant.span;
                    let mut args = Vec::new();
                    if self.check(&TokenKind::LParen) {
                        self.advance();
                        while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                            args.push(self.parse_expr(Precedence::Lowest, true)?);
                            if self.check(&TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        end_span = self.expect(&TokenKind::RParen)?.span;
                    }
                    return Ok(Expr {
                        kind: ExprKind::EnumLit {
                            enum_name: ident,
                            variant,
                            args,
                        },
                        span: Span::merge(tok.span, end_span),
                    });
                }

                // Disambiguate struct literal: Name { ... }
                if struct_allowed && self.check(&TokenKind::LBrace) {
                    return self.parse_struct_lit(ident);
                }

                Ok(Expr {
                    kind: ExprKind::Variable(ident),
                    span: tok.span,
                })
            }
            _ => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!("col {}: expected expression, found {:?}", tok.span.col, tok.kind),
            }),
        }
    }

    fn parse_call(&mut self, callee_expr: Expr) -> Result<Expr, ParseError> {
        let callee = match callee_expr.kind {
            ExprKind::Variable(ident) => ident,
            _ => {
                return Err(ParseError::InvalidSyntax {
                    line: callee_expr.span.line,
                    msg: format!("col {}: callee must be a named function identifier", callee_expr.span.col),
                });
            }
        };

        self.expect(&TokenKind::LParen)?;
        let mut args = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_at_end() {
            args.push(self.parse_expr(Precedence::Lowest, true)?);
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let end_tok = self.expect(&TokenKind::RParen)?;

        let span = Span::merge(callee_expr.span, end_tok.span);
        Ok(Expr {
            kind: ExprKind::Call { callee, args },
            span,
        })
    }

    fn parse_struct_lit(&mut self, name: Ident) -> Result<Expr, ParseError> {
        let start_span = name.span;
        self.expect(&TokenKind::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let f_name = self.parse_ident()?;
            self.expect(&TokenKind::Colon)?;
            let f_val = self.parse_expr(Precedence::Lowest, true)?;
            let f_span = Span::merge(f_name.span, f_val.span);
            fields.push(FieldInit {
                name: f_name,
                value: f_val,
                span: f_span,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        let end_tok = self.expect(&TokenKind::RBrace)?;
        Ok(Expr {
            kind: ExprKind::StructLit { name, fields },
            span: Span::merge(start_span, end_tok.span),
        })
    }

    fn parse_array_lit(&mut self) -> Result<Expr, ParseError> {
        let start_tok = self.expect(&TokenKind::LBracket)?;
        let mut elements = Vec::new();

        while !self.check(&TokenKind::RBracket) && !self.is_at_end() {
            elements.push(self.parse_expr(Precedence::Lowest, true)?);
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        let end_tok = self.expect(&TokenKind::RBracket)?;
        Ok(Expr {
            kind: ExprKind::ArrayLit(elements),
            span: Span::merge(start_tok.span, end_tok.span),
        })
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start_tok = self.expect(&TokenKind::If)?;
        let cond = self.parse_expr(Precedence::Lowest, false)?;
        let then_branch = self.parse_block()?;
        self.expect(&TokenKind::Else)?;
        let else_branch = if self.check(&TokenKind::If) {
            let nested = self.parse_if_expr()?;
            let span = nested.span;
            Block {
                stmts: Vec::new(),
                tail_expr: Some(Box::new(nested)),
                span,
            }
        } else {
            self.parse_block()?
        };

        let span = Span::merge(start_tok.span, else_branch.span);
        Ok(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch,
                else_branch,
            },
            span,
        })
    }

    fn parse_syscall_expr(&mut self) -> Result<Expr, ParseError> {
        let start_tok = self.expect(&TokenKind::Syscall)?;
        let effect = self.parse_ident()?;
        let op_tok = self.advance();
        let op = match op_tok.kind {
            TokenKind::StrLit(s) => s,
            _ => {
                return Err(ParseError::InvalidSyntax {
                    line: op_tok.span.line,
                    msg: format!("col {}: expected string literal operation name in syscall", op_tok.span.col),
                });
            }
        };
        let payload = self.parse_expr(Precedence::Lowest, true)?;
        let span = Span::merge(start_tok.span, payload.span);
        Ok(Expr {
            kind: ExprKind::Syscall {
                effect,
                op,
                payload: Box::new(payload),
            },
            span,
        })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let start_tok = self.expect(&TokenKind::Match)?;
        let scrutinee = self.parse_expr(Precedence::Lowest, false)?;
        self.expect(&TokenKind::LBrace)?;

        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let arm_start = self.current_span();
            let pattern = self.parse_match_pattern()?;
            self.expect(&TokenKind::FatArrow)?;
            let body = self.parse_expr(Precedence::Lowest, true)?;
            let arm_span = Span::merge(arm_start, body.span);
            arms.push(MatchArm {
                pattern,
                body,
                span: arm_span,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            }
        }

        let end_tok = self.expect(&TokenKind::RBrace)?;
        Ok(Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: Span::merge(start_tok.span, end_tok.span),
        })
    }

    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(MatchPattern::Int(v))
            }
            TokenKind::Minus => {
                self.advance();
                let num_tok = self.advance();
                if let TokenKind::IntLit(v) = num_tok.kind {
                    Ok(MatchPattern::Int(-v))
                } else {
                    Err(ParseError::InvalidSyntax {
                        line: num_tok.span.line,
                        msg: "Expected integer after '-' in match pattern".to_string(),
                    })
                }
            }
            TokenKind::True => {
                self.advance();
                Ok(MatchPattern::Bool(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(MatchPattern::Bool(false))
            }
            TokenKind::StrLit(ref s) => {
                let s = s.clone();
                self.advance();
                Ok(MatchPattern::Str(s))
            }
            TokenKind::Ident(ref name) if name == "_" => {
                self.advance();
                Ok(MatchPattern::Wildcard)
            }
            TokenKind::Ident(ref first) => {
                let first_ident = Ident::new(first, tok.span);
                self.advance();
                let (enum_name, variant) = if self.check(&TokenKind::ColonColon) {
                    self.advance();
                    let v = self.parse_ident()?;
                    (Some(first_ident), v)
                } else {
                    (None, first_ident)
                };
                let mut bindings = Vec::new();
                if self.check(&TokenKind::LParen) {
                    self.advance();
                    while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                        bindings.push(self.parse_ident()?);
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                }
                Ok(MatchPattern::Variant {
                    enum_name,
                    variant,
                    bindings,
                })
            }
            _ => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!("col {}: expected match pattern, found {:?}", tok.span.col, tok.kind),
            }),
        }
    }

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::LBracket => {
                self.advance();
                let inner = self.parse_type_expr()?;
                let end_tok = self.expect(&TokenKind::RBracket)?;
                let span = Span::merge(tok.span, end_tok.span);
                Ok(TypeExpr::new(TypeExprKind::Vec(Box::new(inner)), span))
            }
            TokenKind::Struct => {
                self.advance();
                self.expect(&TokenKind::LBracket)?;
                let s_name = self.parse_ident()?;
                let end_tok = self.expect(&TokenKind::RBracket)?;
                let span = Span::merge(tok.span, end_tok.span);
                Ok(TypeExpr::new(TypeExprKind::Struct(s_name), span))
            }
            TokenKind::Ident(ref name) => {
                match name.as_str() {
                    "vec" => {
                        self.advance();
                        self.expect(&TokenKind::LBracket)?;
                        let inner = self.parse_type_expr()?;
                        let end_tok = self.expect(&TokenKind::RBracket)?;
                        let span = Span::merge(tok.span, end_tok.span);
                        Ok(TypeExpr::new(TypeExprKind::Vec(Box::new(inner)), span))
                    }
                    "map" => {
                        self.advance();
                        self.expect(&TokenKind::LBracket)?;
                        let k = self.parse_type_expr()?;
                        self.expect(&TokenKind::Comma)?;
                        let v = self.parse_type_expr()?;
                        let end_tok = self.expect(&TokenKind::RBracket)?;
                        let span = Span::merge(tok.span, end_tok.span);
                        Ok(TypeExpr::new(
                            TypeExprKind::Map(Box::new(k), Box::new(v)),
                            span,
                        ))
                    }
                    "chan" => {
                        self.advance();
                        self.expect(&TokenKind::LBracket)?;
                        let inner = self.parse_type_expr()?;
                        let end_tok = self.expect(&TokenKind::RBracket)?;
                        let span = Span::merge(tok.span, end_tok.span);
                        Ok(TypeExpr::new(TypeExprKind::Chan(Box::new(inner)), span))
                    }
                    "struct" => {
                        self.advance();
                        self.expect(&TokenKind::LBracket)?;
                        let s_name = self.parse_ident()?;
                        let end_tok = self.expect(&TokenKind::RBracket)?;
                        let span = Span::merge(tok.span, end_tok.span);
                        Ok(TypeExpr::new(TypeExprKind::Struct(s_name), span))
                    }
                    _ => {
                        let ident = Ident::new(name, tok.span);
                        self.advance();
                        Ok(TypeExpr::new(TypeExprKind::Named(ident), tok.span))
                    }
                }
            }
            _ => Err(ParseError::InvalidType {
                line: tok.span.line,
                token: format!("{:?}", tok.kind),
            }),
        }
    }

    fn parse_ident(&mut self) -> Result<Ident, ParseError> {
        let tok = self.advance();
        match tok.kind {
            TokenKind::Ident(name) => Ok(Ident::new(name, tok.span)),
            _ => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!("col {}: expected identifier, found {:?}", tok.span.col, tok.kind),
            }),
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.cursor).unwrap_or_else(|| {
            self.tokens.last().expect("Tokens vector is never empty")
        })
    }

    fn peek_nth(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.cursor + n)
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.cursor < self.tokens.len().saturating_sub(1) {
            self.cursor += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, ParseError> {
        let tok = self.peek().clone();
        if &tok.kind == kind {
            Ok(self.advance())
        } else {
            if kind == &TokenKind::RBrace && tok.kind == TokenKind::Eof {
                return Err(ParseError::MissingClosingBrace { line: tok.span.line });
            }
            Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!("col {}: expected {:?}, found {:?}", tok.span.col, kind, tok.kind),
            })
        }
    }

    fn is_at_end(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn current_span(&self) -> Span {
        self.peek().span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_fn() {
        let src = r#"
            fn add(a: i64, b: i64) -> i64 {
                return a + b;
            }
        "#;
        let mut parser = Parser::new(src).unwrap();
        let prog = parser.parse_program().unwrap();
        assert_eq!(prog.declarations.len(), 1);
        if let Declaration::Function(ref f) = prog.declarations[0] {
            assert_eq!(f.name.name, "add");
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.body.stmts.len(), 1);
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn test_parse_precedence() {
        let src = "fn test() -> i64 { return 2 + 3 * 4; }";
        let mut parser = Parser::new(src).unwrap();
        let prog = parser.parse_program().unwrap();
        if let Declaration::Function(ref f) = prog.declarations[0] {
            if let StmtKind::Return { value: Some(ref expr) } = f.body.stmts[0].kind {
                if let ExprKind::Binary { op, lhs, rhs } = &expr.kind {
                    assert_eq!(*op, BinaryOp::Add);
                    assert!(matches!(lhs.kind, ExprKind::Int(2)));
                    assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
                } else {
                    panic!("Expected binary add");
                }
            }
        }
    }

    #[test]
    fn test_parse_struct_literal_and_array() {
        let src = r#"
            fn build() -> Card {
                let c = Card {
                    tag: "div",
                    children: [
                        Leaf { tag: "p" },
                    ],
                };
                return c;
            }
        "#;
        let mut parser = Parser::new(src).unwrap();
        let prog = parser.parse_program().unwrap();
        assert_eq!(prog.declarations.len(), 1);
    }
}
