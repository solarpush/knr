use std::collections::{HashMap, HashSet};

use crate::error::ParseError;
use super::ast::*;
use super::lexer::Lexer;
use super::token::{Token, TokenKind};

/// Recursive-descent Strict TypeScript / TSX Parser producing a clean typed AST
/// lowered to canonical verified linear SSA.
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    type_aliases: HashMap<String, TypeExpr>,
    namespace_imports: HashSet<String>,
    switch_counter: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            lexer: Lexer::new(src),
            type_aliases: HashMap::new(),
            namespace_imports: HashSet::new(),
            switch_counter: 0,
        }
    }

    fn peek(&mut self) -> Result<Token, ParseError> {
        self.lexer.peek_token()
    }

    fn peek_nth(&mut self, n: usize) -> Result<Token, ParseError> {
        self.lexer.peek_nth_token(n)
    }

    fn next(&mut self) -> Result<Token, ParseError> {
        self.lexer.next_token()
    }

    fn check(&mut self, expected: &TokenKind) -> Result<bool, ParseError> {
        Ok(&self.peek()?.kind == expected)
    }

    fn eat(&mut self, expected: &TokenKind) -> Result<bool, ParseError> {
        if self.check(expected)? {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<Token, ParseError> {
        let tok = self.next()?;
        if &tok.kind == expected {
            Ok(tok)
        } else {
            Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!(
                    "Expected {:?}, found {:?} at line {}, column {}",
                    expected, tok.kind, tok.span.line, tok.span.col
                ),
            })
        }
    }

    fn expect_ident(&mut self) -> Result<Ident, ParseError> {
        let tok = self.next()?;
        match tok.kind {
            TokenKind::Ident(name) => Ok(Ident::new(name, tok.span)),
            // Allow contextual TS identifiers in field/prop/effect names when needed
            TokenKind::Type => Ok(Ident::new("type", tok.span)),
            TokenKind::From => Ok(Ident::new("from", tok.span)),
            TokenKind::Default => Ok(Ident::new("default", tok.span)),
            TokenKind::As => Ok(Ident::new("as", tok.span)),
            other => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!(
                    "Expected identifier, found {:?} at line {}, column {}",
                    other, tok.span.line, tok.span.col
                ),
            }),
        }
    }

    fn expect_ident_strict(&mut self) -> Result<Ident, ParseError> {
        let tok = self.next()?;
        match tok.kind {
            TokenKind::Ident(name) => Ok(Ident::new(name, tok.span)),
            other => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!(
                    "Expected identifier, found {:?} at line {}, column {}",
                    other, tok.span.line, tok.span.col
                ),
            }),
        }
    }

    /// Parses a complete `.tsx` / `.ts` / `.knr` source file.
    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let start = self.lexer.current_span();
        let mut imports = Vec::new();
        let mut declarations = Vec::new();

        while !self.check(&TokenKind::Eof)? {
            if self.check(&TokenKind::Import)? {
                imports.push(self.parse_import()?);
                continue;
            }

            // Collect optional decorators: `@shader`, `@effects("net", "hw")`
            let mut is_shader = false;
            let mut effects: Vec<Ident> = Vec::new();
            while self.eat(&TokenKind::At)? {
                let dec = self.expect_ident()?;
                match dec.name.as_str() {
                    "shader" => {
                        is_shader = true;
                        if self.eat(&TokenKind::LParen)? {
                            self.expect(&TokenKind::RParen)?;
                        }
                    }
                    "effects" | "effect" => {
                        self.expect(&TokenKind::LParen)?;
                        while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                            let tok = self.next()?;
                            let eff_name = match tok.kind {
                                TokenKind::StrLit(s) => canonical_effect_name(&s),
                                TokenKind::Ident(s) => canonical_effect_name(&s),
                                other => {
                                    return Err(ParseError::InvalidSyntax {
                                        line: tok.span.line,
                                        msg: format!(
                                            "Expected effect name string or identifier in @effects(...), found {:?}",
                                            other
                                        ),
                                    });
                                }
                            };
                            effects.push(Ident::new(eff_name, tok.span));
                            if !self.eat(&TokenKind::Comma)? {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RParen)?;
                    }
                    other => {
                        return Err(ParseError::InvalidSyntax {
                            line: dec.span.line,
                            msg: format!(
                                "Unknown decorator '@{other}'. Supported decorators: @shader, @effects(...)"
                            ),
                        });
                    }
                }
            }

            // Optional `export` (including barrel re-exports: `export { A, B } from "./a.tsk";` / `export * from "./a.tsk";`)
            let export_span = self.peek()?.span;
            let is_exported = self.eat(&TokenKind::Export)?;

            if is_exported
                && (self.check(&TokenKind::LBrace)? || self.check(&TokenKind::Star)?)
            {
                imports.push(self.parse_reexport(export_span)?);
                continue;
            }

            let tok = self.peek()?;
            match tok.kind {
                TokenKind::Type => {
                    if let Some(decl) = self.parse_type_decl(is_exported)? {
                        declarations.push(decl);
                    }
                }
                TokenKind::Interface => {
                    declarations.push(Declaration::Struct(self.parse_interface_decl(is_exported)?));
                }
                TokenKind::Enum => {
                    declarations.push(Declaration::Enum(self.parse_enum_decl(is_exported)?));
                }
                TokenKind::Function => {
                    declarations.push(Declaration::Function(
                        self.parse_function_decl(is_shader, effects, is_exported)?,
                    ));
                }
                TokenKind::Const => {
                    declarations.push(Declaration::Const(self.parse_const_decl(is_exported)?));
                }
                other => {
                    return Err(ParseError::InvalidSyntax {
                        line: tok.span.line,
                        msg: format!(
                            "Unexpected top-level token {:?} at line {}, column {}. Expected import, export, type, interface, enum, const, or function.",
                            other, tok.span.line, tok.span.col
                        ),
                    });
                }
            }
        }

        let end = self.lexer.current_span();
        Ok(Program {
            imports,
            declarations,
            span: Span::merge(start, end),
        })
    }

    // ========================================================================
    // Imports & Top-Level Declarations
    // ========================================================================

    fn parse_import(&mut self) -> Result<ImportDecl, ParseError> {
        let start = self.expect(&TokenKind::Import)?.span;
        // Optional `import type ...`
        let _ = self.eat(&TokenKind::Type)?;

        // Case 1: `import "std/ui";`
        if let TokenKind::StrLit(_) = self.peek()?.kind {
            let tok = self.next()?;
            let path = match tok.kind {
                TokenKind::StrLit(s) => s,
                _ => unreachable!(),
            };
            let _ = self.eat(&TokenKind::Semicolon)?;
            return Ok(ImportDecl {
                path,
                kind: ImportKind::SideEffect,
                is_reexport: false,
                span: Span::merge(start, tok.span),
            });
        }

        // Case 2: `import { A, B as C } from "std/ui";`
        let kind = if self.eat(&TokenKind::LBrace)? {
            let mut named = Vec::new();
            while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
                let _ = self.eat(&TokenKind::Type)?;
                let name = self.expect_ident()?;
                let alias = if self.eat(&TokenKind::As)? {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                named.push((name, alias));
                if !self.eat(&TokenKind::Comma)? {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace)?;
            ImportKind::Named(named)
        } else if self.eat(&TokenKind::Star)? {
            // Case 3: `import * as ui from "std/ui";` or shorthand `import * from "./state.tsk";`
            if self.eat(&TokenKind::As)? {
                let alias = self.expect_ident()?;
                self.namespace_imports.insert(alias.name.clone());
                ImportKind::Namespace(alias)
            } else {
                ImportKind::SideEffect
            }
        } else {
            // Case 4: `import DefaultName from "./foo.tsx";`
            let def = self.expect_ident()?;
            let mut named = Vec::new();
            if self.eat(&TokenKind::Comma)? && self.eat(&TokenKind::LBrace)? {
                while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
                    let name = self.expect_ident()?;
                    let alias = if self.eat(&TokenKind::As)? {
                        Some(self.expect_ident()?)
                    } else {
                        None
                    };
                    named.push((name, alias));
                    if !self.eat(&TokenKind::Comma)? {
                        break;
                    }
                }
                self.expect(&TokenKind::RBrace)?;
            }
            ImportKind::Default { name: def, named }
        };

        self.expect(&TokenKind::From)?;
        let path_tok = self.next()?;
        let path = match path_tok.kind {
            TokenKind::StrLit(s) => s,
            other => {
                return Err(ParseError::InvalidSyntax {
                    line: path_tok.span.line,
                    msg: format!("Expected module path string after 'from', found {:?}", other),
                });
            }
        };
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(ImportDecl {
            path,
            kind,
            is_reexport: false,
            span: Span::merge(start, path_tok.span),
        })
    }

    fn parse_reexport(&mut self, start: Span) -> Result<ImportDecl, ParseError> {
        let kind = if self.eat(&TokenKind::LBrace)? {
            let mut named = Vec::new();
            while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
                let _ = self.eat(&TokenKind::Type)?;
                let name = self.expect_ident()?;
                let alias = if self.eat(&TokenKind::As)? {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                named.push((name, alias));
                if !self.eat(&TokenKind::Comma)? {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace)?;
            ImportKind::Named(named)
        } else if self.eat(&TokenKind::Star)? {
            if self.eat(&TokenKind::As)? {
                let alias = self.expect_ident()?;
                ImportKind::Namespace(alias)
            } else {
                ImportKind::SideEffect
            }
        } else {
            return Err(ParseError::InvalidSyntax {
                line: start.line,
                msg: "Expected '{ ... }' or '*' after 'export' for re-export".to_string(),
            });
        };

        self.expect(&TokenKind::From)?;
        let path_tok = self.next()?;
        let path = match path_tok.kind {
            TokenKind::StrLit(s) => s,
            other => {
                return Err(ParseError::InvalidSyntax {
                    line: path_tok.span.line,
                    msg: format!("Expected module path string after 'from', found {:?}", other),
                });
            }
        };
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(ImportDecl {
            path,
            kind,
            is_reexport: true,
            span: Span::merge(start, path_tok.span),
        })
    }

    fn parse_optional_type_params(&mut self) -> Result<Vec<Ident>, ParseError> {
        let mut params = Vec::new();
        if self.eat(&TokenKind::Lt)? {
            while !self.check(&TokenKind::Gt)? && !self.check(&TokenKind::Eof)? {
                params.push(self.expect_ident_strict()?);
                if !self.eat(&TokenKind::Comma)? {
                    break;
                }
            }
            self.expect(&TokenKind::Gt)?;
        }
        Ok(params)
    }

    fn parse_type_decl(&mut self, is_exported: bool) -> Result<Option<Declaration>, ParseError> {
        let start = self.expect(&TokenKind::Type)?.span;
        let name = self.expect_ident_strict()?;
        let mut type_params = Vec::new();
        let mut consumed_assign = false;
        if self.eat(&TokenKind::Lt)? {
            while !self.check(&TokenKind::Gt)?
                && !self.check(&TokenKind::GtEq)?
                && !self.check(&TokenKind::Eof)?
            {
                type_params.push(self.expect_ident_strict()?);
                if !self.eat(&TokenKind::Comma)? {
                    break;
                }
            }
            if self.eat(&TokenKind::GtEq)? {
                // Handle compact `type Pair<A,B>={...}` where `>=` is lexed as GtEq
                consumed_assign = true;
            } else {
                self.expect(&TokenKind::Gt)?;
            }
        }
        if !consumed_assign {
            self.expect(&TokenKind::Assign)?;
        }

        if self.check(&TokenKind::LBrace)? {
            let (fields, end) = self.parse_record_fields()?;
            let _ = self.eat(&TokenKind::Semicolon)?;
            return Ok(Some(Declaration::Struct(StructDef {
                name,
                type_params,
                fields,
                is_exported,
                span: Span::merge(start, end),
            })));
        }

        // Type alias (including string/int literal unions: `type Tab = "overview" | "shaders";`)
        let aliased = self.parse_type_expr()?;
        let _ = self.eat(&TokenKind::Semicolon)?;
        self.type_aliases.insert(name.name, aliased);
        Ok(None)
    }

    fn parse_interface_decl(&mut self, is_exported: bool) -> Result<StructDef, ParseError> {
        let start = self.expect(&TokenKind::Interface)?.span;
        let name = self.expect_ident_strict()?;
        let type_params = self.parse_optional_type_params()?;
        let (fields, end) = self.parse_record_fields()?;
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(StructDef {
            name,
            type_params,
            fields,
            is_exported,
            span: Span::merge(start, end),
        })
    }

    fn parse_record_fields(&mut self) -> Result<(Vec<FieldDef>, Span), ParseError> {
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
            let f_start = self.peek()?.span;
            let _ = self.eat(&TokenKind::Readonly)?;
            let fname = self.expect_ident()?;
            let _ = self.eat(&TokenKind::Question)?; // optional `?` marker tolerated
            self.expect(&TokenKind::Colon)?;
            let fty = self.parse_type_expr()?;
            let f_span = Span::merge(f_start, fty.span);
            fields.push(FieldDef {
                index: Some(fields.len() as u32),
                name: fname,
                ty: fty,
                span: f_span,
            });
            if !self.eat(&TokenKind::Semicolon)? && !self.eat(&TokenKind::Comma)? {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok((fields, end))
    }

    fn parse_enum_decl(&mut self, is_exported: bool) -> Result<EnumDef, ParseError> {
        let start = self.expect(&TokenKind::Enum)?.span;
        let name = self.expect_ident_strict()?;
        let type_params = self.parse_optional_type_params()?;
        self.expect(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
            let vname = self.expect_ident_strict()?;
            let vstart = vname.span;
            let mut fields = Vec::new();
            let mut vend = vstart;
            if self.eat(&TokenKind::LParen)? {
                while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                    fields.push(self.parse_type_expr()?);
                    if !self.eat(&TokenKind::Comma)? {
                        break;
                    }
                }
                vend = self.expect(&TokenKind::RParen)?.span;
            } else if self.eat(&TokenKind::Assign)? {
                // Allow TS numeric/string enum initializer syntax gracefully
                let _ = self.next()?;
            }
            variants.push(EnumVariantDef {
                name: vname,
                fields,
                span: Span::merge(vstart, vend),
            });
            if !self.eat(&TokenKind::Comma)? && !self.eat(&TokenKind::Semicolon)? {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(EnumDef {
            name,
            type_params,
            variants,
            is_exported,
            span: Span::merge(start, end),
        })
    }

    fn parse_const_decl(&mut self, is_exported: bool) -> Result<ConstDef, ParseError> {
        let start = self.expect(&TokenKind::Const)?.span;
        let name = self.expect_ident_strict()?;
        let ty = if self.eat(&TokenKind::Colon)? {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;
        let end = value.span;
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(ConstDef {
            name,
            ty,
            value,
            is_exported,
            span: Span::merge(start, end),
        })
    }

    fn parse_function_decl(
        &mut self,
        is_shader: bool,
        effects: Vec<Ident>,
        is_exported: bool,
    ) -> Result<FunctionDef, ParseError> {
        let start = self.expect(&TokenKind::Function)?.span;
        let name = self.expect_ident_strict()?;
        let type_params = self.parse_optional_type_params()?;
        self.expect(&TokenKind::LParen)?;

        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
            let p_start = self.peek()?.span;
            let pname = self.expect_ident_strict()?;
            self.expect(&TokenKind::Colon)?;
            let is_borrowed = self.eat(&TokenKind::Readonly)? || self.eat(&TokenKind::Amp)?;
            let pty = self.parse_type_expr()?;
            let p_span = Span::merge(p_start, pty.span);
            params.push(ParamDef {
                name: pname,
                ty: pty,
                is_borrowed,
                span: p_span,
            });
            if !self.eat(&TokenKind::Comma)? {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;

        let ret_ty = if self.eat(&TokenKind::Colon)? {
            self.parse_type_expr()?
        } else {
            let sp = self.lexer.current_span();
            TypeExpr::new(TypeExprKind::Named(Ident::new("void", sp)), sp)
        };

        let body = self.parse_block()?;
        let span = Span::merge(start, body.span);
        Ok(FunctionDef {
            name,
            type_params,
            params,
            ret_ty,
            effects,
            is_shader,
            is_exported,
            body,
            span,
        })
    }

    // ========================================================================
    // Type Expressions
    // ========================================================================

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        // Optional leading `|` in multi-line TS union types
        let _ = self.eat(&TokenKind::Pipe)?;
        let mut ty = self.parse_single_type_expr()?;

        // Support union types of string/int/bool literals (e.g. `"inc" | "dec" | "reset"`)
        while self.eat(&TokenKind::Pipe)? {
            let _rhs = self.parse_single_type_expr()?;
        }
        // Handle postfix array syntax: `Widget[]`, `f64[]`
        while self.check(&TokenKind::LBracket)?
            && self.peek_nth(1)?.kind == TokenKind::RBracket
        {
            self.next()?;
            let end = self.next()?.span;
            let sp = Span::merge(ty.span, end);
            ty = TypeExpr::new(TypeExprKind::Vec(Box::new(ty)), sp);
        }
        Ok(ty)
    }

    fn parse_single_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let tok = self.next()?;
        match tok.kind {
            TokenKind::StrLit(_) => Ok(TypeExpr::new(
                TypeExprKind::Named(Ident::new("string", tok.span)),
                tok.span,
            )),
            TokenKind::IntLit(_) => Ok(TypeExpr::new(
                TypeExprKind::Named(Ident::new("i64", tok.span)),
                tok.span,
            )),
            TokenKind::True | TokenKind::False => Ok(TypeExpr::new(
                TypeExprKind::Named(Ident::new("bool", tok.span)),
                tok.span,
            )),
            TokenKind::Ident(name) => {
                if let Some(aliased) = self.type_aliases.get(&name).cloned() {
                    return Ok(TypeExpr::new(aliased.kind, tok.span));
                }
                match name.as_str() {
                    "Array" | "Vec" => {
                        self.expect(&TokenKind::Lt)?;
                        let inner = self.parse_type_expr()?;
                        let end = self.expect(&TokenKind::Gt)?.span;
                        Ok(TypeExpr::new(
                            TypeExprKind::Vec(Box::new(inner)),
                            Span::merge(tok.span, end),
                        ))
                    }
                    "Map" => {
                        self.expect(&TokenKind::Lt)?;
                        let k = self.parse_type_expr()?;
                        self.expect(&TokenKind::Comma)?;
                        let v = self.parse_type_expr()?;
                        let end = self.expect(&TokenKind::Gt)?.span;
                        Ok(TypeExpr::new(
                            TypeExprKind::Map(Box::new(k), Box::new(v)),
                            Span::merge(tok.span, end),
                        ))
                    }
                    "Channel" => {
                        self.expect(&TokenKind::Lt)?;
                        let inner = self.parse_type_expr()?;
                        let end = self.expect(&TokenKind::Gt)?.span;
                        Ok(TypeExpr::new(
                            TypeExprKind::Chan(Box::new(inner)),
                            Span::merge(tok.span, end),
                        ))
                    }
                    _ => {
                        let mut type_args = Vec::new();
                        let mut end_span = tok.span;
                        if self.eat(&TokenKind::Lt)? {
                            while !self.check(&TokenKind::Gt)? && !self.check(&TokenKind::Eof)? {
                                type_args.push(self.parse_type_expr()?);
                                if !self.eat(&TokenKind::Comma)? {
                                    break;
                                }
                            }
                            end_span = self.expect(&TokenKind::Gt)?.span;
                        }
                        let mut ty = TypeExpr::with_type_args(
                            TypeExprKind::Named(Ident::new(name, tok.span)),
                            type_args,
                            Span::merge(tok.span, end_span),
                        );
                        while self.check(&TokenKind::LBracket)?
                            && self.peek_nth(1)?.kind == TokenKind::RBracket
                        {
                            self.next()?;
                            let end = self.next()?.span;
                            let sp = Span::merge(ty.span, end);
                            ty = TypeExpr::new(TypeExprKind::Vec(Box::new(ty)), sp);
                        }
                        Ok(ty)
                    }
                }
            }
            other => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!(
                    "Expected type name, found {:?} at line {}, column {}",
                    other, tok.span.line, tok.span.col
                ),
            }),
        }
    }

    // ========================================================================
    // Blocks & Statements
    // ========================================================================

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        let start = self.expect(&TokenKind::LBrace)?.span;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
            stmts.push(self.parse_stmt()?);
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Block {
            stmts,
            tail_expr: None,
            span: Span::merge(start, end),
        })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        let tok = self.peek()?;
        match tok.kind {
            TokenKind::Const | TokenKind::Let => self.parse_let_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Switch => self.parse_switch_stmt(),
            TokenKind::Break => {
                let sp = self.next()?.span;
                let _ = self.eat(&TokenKind::Semicolon)?;
                Ok(Stmt {
                    kind: StmtKind::Break,
                    span: sp,
                })
            }
            TokenKind::Continue => {
                let sp = self.next()?.span;
                let _ = self.eat(&TokenKind::Semicolon)?;
                Ok(Stmt {
                    kind: StmtKind::Continue,
                    span: sp,
                })
            }
            TokenKind::LBrace => {
                let blk = self.parse_block()?;
                let sp = blk.span;
                Ok(Stmt {
                    kind: StmtKind::Block(blk),
                    span: sp,
                })
            }
            _ => self.parse_expr_or_assign_stmt(),
        }
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.next()?; // const | let
        let is_const = kw.kind == TokenKind::Const;
        let start = kw.span;

        // Object destructuring: `const { a, b: localB } = obj;`
        if self.eat(&TokenKind::LBrace)? {
            let mut fields = Vec::new();
            while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
                let field_name = self.expect_ident()?;
                let alias = if self.eat(&TokenKind::Colon)? {
                    Some(self.expect_ident_strict()?)
                } else {
                    None
                };
                fields.push((field_name, alias));
                if !self.eat(&TokenKind::Comma)? {
                    break;
                }
            }
            let rbrace = self.expect(&TokenKind::RBrace)?.span;
            let ty = if self.eat(&TokenKind::Colon)? {
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::Assign)?;
            let init = self.parse_expr()?;
            let end = init.span;
            let _ = self.eat(&TokenKind::Semicolon)?;
            let primary_name = fields
                .first()
                .map(|(f, a)| a.clone().unwrap_or_else(|| f.clone()))
                .unwrap_or_else(|| Ident::new("_", Span::merge(start, rbrace)));
            return Ok(Stmt {
                kind: StmtKind::Let {
                    name: primary_name,
                    ty,
                    init,
                    is_const,
                    destruct: Some(DestructPattern::Object(fields)),
                },
                span: Span::merge(start, end),
            });
        }

        // Array destructuring: `const [first, second] = arr;`
        if self.eat(&TokenKind::LBracket)? {
            let mut elems = Vec::new();
            while !self.check(&TokenKind::RBracket)? && !self.check(&TokenKind::Eof)? {
                elems.push(self.expect_ident_strict()?);
                if !self.eat(&TokenKind::Comma)? {
                    break;
                }
            }
            let rbracket = self.expect(&TokenKind::RBracket)?.span;
            let ty = if self.eat(&TokenKind::Colon)? {
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::Assign)?;
            let init = self.parse_expr()?;
            let end = init.span;
            let _ = self.eat(&TokenKind::Semicolon)?;
            let primary_name = elems
                .first()
                .cloned()
                .unwrap_or_else(|| Ident::new("_", Span::merge(start, rbracket)));
            return Ok(Stmt {
                kind: StmtKind::Let {
                    name: primary_name,
                    ty,
                    init,
                    is_const,
                    destruct: Some(DestructPattern::Array(elems)),
                },
                span: Span::merge(start, end),
            });
        }

        let name = self.expect_ident_strict()?;
        let ty = if self.eat(&TokenKind::Colon)? {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Assign)?;
        let init = self.parse_expr()?;
        let end = init.span;
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(Stmt {
            kind: StmtKind::Let {
                name,
                ty,
                init,
                is_const,
                destruct: None,
            },
            span: Span::merge(start, end),
        })
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Return)?.span;
        if self.eat(&TokenKind::Semicolon)? || self.check(&TokenKind::RBrace)? {
            return Ok(Stmt {
                kind: StmtKind::Return { value: None },
                span: start,
            });
        }
        let value = self.parse_expr()?;
        let end = value.span;
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(Stmt {
            kind: StmtKind::Return { value: Some(value) },
            span: Span::merge(start, end),
        })
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::If)?.span;
        let cond = if self.eat(&TokenKind::LParen)? {
            let e = self.parse_expr()?;
            self.expect(&TokenKind::RParen)?;
            e
        } else {
            self.parse_expr()?
        };
        let then_block = if self.check(&TokenKind::LBrace)? {
            self.parse_block()?
        } else {
            let s = self.parse_stmt()?;
            let sp = s.span;
            Block {
                stmts: vec![s],
                tail_expr: None,
                span: sp,
            }
        };
        let mut end = then_block.span;
        let else_block = if self.eat(&TokenKind::Else)? {
            if self.check(&TokenKind::If)? {
                let nested = self.parse_if_stmt()?;
                let sp = nested.span;
                end = sp;
                Some(Block {
                    stmts: vec![nested],
                    tail_expr: None,
                    span: sp,
                })
            } else if self.check(&TokenKind::LBrace)? {
                let blk = self.parse_block()?;
                end = blk.span;
                Some(blk)
            } else {
                let s = self.parse_stmt()?;
                let sp = s.span;
                end = sp;
                Some(Block {
                    stmts: vec![s],
                    tail_expr: None,
                    span: sp,
                })
            }
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then_block,
                else_block,
            },
            span: Span::merge(start, end),
        })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::While)?.span;
        let cond = if self.eat(&TokenKind::LParen)? {
            let e = self.parse_expr()?;
            self.expect(&TokenKind::RParen)?;
            e
        } else {
            self.parse_expr()?
        };
        let body = self.parse_block()?;
        let span = Span::merge(start, body.span);
        Ok(Stmt {
            kind: StmtKind::While { cond, body },
            span,
        })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::For)?.span;
        self.expect(&TokenKind::LParen)?;

        let init_stmt = if self.eat(&TokenKind::Semicolon)? {
            None
        } else if self.check(&TokenKind::Let)? || self.check(&TokenKind::Const)? {
            Some(self.parse_let_stmt()?)
        } else {
            Some(self.parse_expr_or_assign_stmt()?)
        };

        let cond = if self.check(&TokenKind::Semicolon)? {
            Expr {
                kind: ExprKind::Bool(true),
                span: start,
            }
        } else {
            self.parse_expr()?
        };
        self.expect(&TokenKind::Semicolon)?;

        let step_stmt = if self.check(&TokenKind::RParen)? {
            None
        } else {
            Some(self.parse_expr_or_assign_no_semi()?)
        };
        self.expect(&TokenKind::RParen)?;

        let mut body = self.parse_block()?;
        if let Some(step) = step_stmt {
            body.stmts.push(step);
        }
        let end = body.span;
        let while_stmt = Stmt {
            kind: StmtKind::While { cond, body },
            span: Span::merge(start, end),
        };
        if let Some(init) = init_stmt {
            Ok(Stmt {
                kind: StmtKind::Block(Block {
                    stmts: vec![init, while_stmt],
                    tail_expr: None,
                    span: Span::merge(start, end),
                }),
                span: Span::merge(start, end),
            })
        } else {
            Ok(while_stmt)
        }
    }

    fn parse_switch_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.expect(&TokenKind::Switch)?.span;
        self.expect(&TokenKind::LParen)?;
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::LBrace)?;

        // Collect all case arms and optional default arm
        enum SwitchCase {
            Value(Expr, Block, Span),
            EnumVariant(Option<Ident>, Ident, Vec<Ident>, Block, Span),
        }

        let mut cases: Vec<SwitchCase> = Vec::new();
        let mut default_blk: Option<(Block, Span)> = None;
        let mut has_enum_variant = false;

        while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
            if self.eat(&TokenKind::Case)? {
                let c_start = self.peek()?.span;
                // Check for enum variant pattern: `Enum.Variant(x)`, `Enum.Variant`, or `Enum<T>.Variant(x)`
                let parsed_case = if let TokenKind::Ident(_) = self.peek()?.kind {
                    let saved = self.lexer.checkpoint();
                    let mut first = self.expect_ident_strict()?;
                    let mut generic_ok = true;
                    if self.eat(&TokenKind::Lt)? {
                        let mut targs = Vec::new();
                        while !self.check(&TokenKind::Gt)? && !self.check(&TokenKind::Eof)? {
                            if let Ok(te) = self.parse_type_expr() {
                                targs.push(te);
                            } else {
                                generic_ok = false;
                                break;
                            }
                            if !self.eat(&TokenKind::Comma)? {
                                break;
                            }
                        }
                        if generic_ok && self.eat(&TokenKind::Gt)? {
                            let core_args: Result<Vec<_>, _> =
                                targs.iter().map(|t| t.to_core_type()).collect();
                            if let Ok(ca) = core_args {
                                first.name = mangle_generic_name(&first.name, &ca);
                            }
                        } else {
                            generic_ok = false;
                        }
                    }
                    if generic_ok && self.eat(&TokenKind::Dot)? {
                        let variant = self.expect_ident_strict()?;
                        let mut bindings = Vec::new();
                        if self.eat(&TokenKind::LParen)? {
                            while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                                bindings.push(self.expect_ident_strict()?);
                                if !self.eat(&TokenKind::Comma)? {
                                    break;
                                }
                            }
                            self.expect(&TokenKind::RParen)?;
                        }
                        if self.check(&TokenKind::Colon)? {
                            has_enum_variant = true;
                            Some((Some(first), variant, bindings))
                        } else {
                            self.lexer.restore(saved);
                            None
                        }
                    } else {
                        self.lexer.restore(saved);
                        None
                    }
                } else {
                    None
                };

                let case_val = if parsed_case.is_none() {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Colon)?;

                let blk = self.parse_switch_case_body(c_start)?;
                let c_span = Span::merge(c_start, blk.span);
                if let Some((enum_name, variant, bindings)) = parsed_case {
                    cases.push(SwitchCase::EnumVariant(enum_name, variant, bindings, blk, c_span));
                } else if let Some(val) = case_val {
                    cases.push(SwitchCase::Value(val, blk, c_span));
                }
            } else if self.eat(&TokenKind::Default)? {
                let d_start = self.peek()?.span;
                self.expect(&TokenKind::Colon)?;
                let blk = self.parse_switch_case_body(d_start)?;
                let d_span = Span::merge(d_start, blk.span);
                default_blk = Some((blk, d_span));
            } else {
                let tok = self.peek()?;
                return Err(ParseError::InvalidSyntax {
                    line: tok.span.line,
                    msg: format!("Expected 'case' or 'default' in switch, found {:?}", tok.kind),
                });
            }
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        let full_span = Span::merge(start, end);

        if has_enum_variant {
            let mut arms = Vec::new();
            for c in cases {
                match c {
                    SwitchCase::EnumVariant(enum_name, variant, bindings, blk, sp) => {
                        arms.push(MatchArm {
                            pattern: MatchPattern::Variant {
                                enum_name,
                                variant,
                                bindings,
                            },
                            body: Expr {
                                kind: ExprKind::Block(blk),
                                span: sp,
                            },
                            span: sp,
                        });
                    }
                    SwitchCase::Value(val, blk, sp) => {
                        let pat = match val.kind {
                            ExprKind::Int(i) => MatchPattern::Int(i),
                            ExprKind::Bool(b) => MatchPattern::Bool(b),
                            ExprKind::Str(s) => MatchPattern::Str(s),
                            _ => MatchPattern::Wildcard,
                        };
                        arms.push(MatchArm {
                            pattern: pat,
                            body: Expr {
                                kind: ExprKind::Block(blk),
                                span: sp,
                            },
                            span: sp,
                        });
                    }
                }
            }
            if let Some((blk, sp)) = default_blk {
                arms.push(MatchArm {
                    pattern: MatchPattern::Wildcard,
                    body: Expr {
                        kind: ExprKind::Block(blk),
                        span: sp,
                    },
                    span: sp,
                });
            }
            return Ok(Stmt {
                kind: StmtKind::Expr(Expr {
                    kind: ExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms,
                    },
                    span: full_span,
                }),
                span: full_span,
            });
        }

        // Desugar value switch to nested if / else-if chain over a single-evaluated scrutinee var
        let (bind_stmt, scr_expr) = match &scrutinee.kind {
            ExprKind::Variable(_) => (None, scrutinee),
            _ => {
                self.switch_counter += 1;
                let tmp_name = format!("__switch_val_{}", self.switch_counter);
                let tmp_ident = Ident::new(tmp_name, scrutinee.span);
                let sp = scrutinee.span;
                (
                    Some(Stmt {
                        kind: StmtKind::Let {
                            name: tmp_ident.clone(),
                            ty: None,
                            init: scrutinee,
                            is_const: true,
                            destruct: None,
                        },
                        span: sp,
                    }),
                    Expr {
                        kind: ExprKind::Variable(tmp_ident),
                        span: sp,
                    },
                )
            }
        };

        let mut else_acc: Option<Block> = default_blk.map(|(b, _)| b);
        for c in cases.into_iter().rev() {
            if let SwitchCase::Value(val, then_block, sp) = c {
                let cond = Expr {
                    kind: ExprKind::Binary {
                        op: BinaryOp::Eq,
                        lhs: Box::new(scr_expr.clone()),
                        rhs: Box::new(val),
                    },
                    span: sp,
                };
                let if_stmt = Stmt {
                    kind: StmtKind::If {
                        cond,
                        then_block,
                        else_block: else_acc,
                    },
                    span: sp,
                };
                else_acc = Some(Block {
                    stmts: vec![if_stmt],
                    tail_expr: None,
                    span: sp,
                });
            }
        }

        let mut outer_stmts = Vec::new();
        if let Some(b) = bind_stmt {
            outer_stmts.push(b);
        }
        if let Some(blk) = else_acc {
            outer_stmts.extend(blk.stmts);
        }
        Ok(Stmt {
            kind: StmtKind::Block(Block {
                stmts: outer_stmts,
                tail_expr: None,
                span: full_span,
            }),
            span: full_span,
        })
    }

    fn parse_switch_case_body(&mut self, start: Span) -> Result<Block, ParseError> {
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::Case)?
            && !self.check(&TokenKind::Default)?
            && !self.check(&TokenKind::RBrace)?
            && !self.check(&TokenKind::Eof)?
        {
            if self.eat(&TokenKind::Break)? {
                let _ = self.eat(&TokenKind::Semicolon)?;
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        let end = stmts.last().map(|s| s.span).unwrap_or(start);
        Ok(Block {
            stmts,
            tail_expr: None,
            span: Span::merge(start, end),
        })
    }

    fn parse_expr_or_assign_stmt(&mut self) -> Result<Stmt, ParseError> {
        let stmt = self.parse_expr_or_assign_no_semi()?;
        let _ = self.eat(&TokenKind::Semicolon)?;
        Ok(stmt)
    }

    fn parse_expr_or_assign_no_semi(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.parse_expr()?;
        let sp = expr.span;
        if self.eat(&TokenKind::Assign)? {
            let value = self.parse_expr()?;
            let end = value.span;
            return Ok(Stmt {
                kind: StmtKind::Assign {
                    target: expr,
                    value,
                },
                span: Span::merge(sp, end),
            });
        }
        let compound_op = if self.eat(&TokenKind::PlusEq)? {
            Some(BinaryOp::Add)
        } else if self.eat(&TokenKind::MinusEq)? {
            Some(BinaryOp::Sub)
        } else if self.eat(&TokenKind::StarEq)? {
            Some(BinaryOp::Mul)
        } else if self.eat(&TokenKind::SlashEq)? {
            Some(BinaryOp::Div)
        } else {
            None
        };
        if let Some(op) = compound_op {
            let rhs = self.parse_expr()?;
            let end = rhs.span;
            let value = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(expr.clone()),
                    rhs: Box::new(rhs),
                },
                span: Span::merge(sp, end),
            };
            return Ok(Stmt {
                kind: StmtKind::Assign {
                    target: expr,
                    value,
                },
                span: Span::merge(sp, end),
            });
        }
        if self.eat(&TokenKind::PlusPlus)? {
            let value = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::Add,
                    lhs: Box::new(expr.clone()),
                    rhs: Box::new(Expr {
                        kind: ExprKind::Int(1),
                        span: sp,
                    }),
                },
                span: sp,
            };
            return Ok(Stmt {
                kind: StmtKind::Assign {
                    target: expr,
                    value,
                },
                span: sp,
            });
        }
        if self.eat(&TokenKind::MinusMinus)? {
            let value = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::Sub,
                    lhs: Box::new(expr.clone()),
                    rhs: Box::new(Expr {
                        kind: ExprKind::Int(1),
                        span: sp,
                    }),
                },
                span: sp,
            };
            return Ok(Stmt {
                kind: StmtKind::Assign {
                    target: expr,
                    value,
                },
                span: sp,
            });
        }

        Ok(Stmt {
            kind: StmtKind::Expr(expr),
            span: sp,
        })
    }

    // ========================================================================
    // Expressions
    // ========================================================================

    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        if let Some(arrow) = self.try_parse_arrow()? {
            return Ok(arrow);
        }
        self.parse_ternary()
    }

    /// Speculatively parses an arrow function expression:
    /// `x => expr`, `x => { ... }`, `() => expr`, `(x) => expr`, `(x, i) => expr`, `(x) => { ... }`.
    fn try_parse_arrow(&mut self) -> Result<Option<Expr>, ParseError> {
        let tok = self.peek()?;
        match tok.kind {
            TokenKind::Ident(_) => {
                if self.peek_nth(1)?.kind != TokenKind::FatArrow {
                    return Ok(None);
                }
                let param = self.expect_ident_strict()?;
                let start = param.span;
                self.expect(&TokenKind::FatArrow)?;
                let (body, end) = self.parse_arrow_body()?;
                Ok(Some(Expr {
                    kind: ExprKind::Arrow {
                        params: vec![param],
                        body,
                    },
                    span: Span::merge(start, end),
                }))
            }
            TokenKind::LParen => {
                let saved = self.lexer.checkpoint();
                let start = self.next()?.span;
                let mut params = Vec::new();
                let mut valid = true;
                while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                    match self.peek()?.kind {
                        TokenKind::Ident(_) => {
                            let p = self.expect_ident_strict()?;
                            // Tolerate optional TS type annotation on arrow parameter: `(x: Item, i: i64) => ...`
                            if self.eat(&TokenKind::Colon)? {
                                let _ = self.eat(&TokenKind::Readonly)?;
                                if self.parse_type_expr().is_err() {
                                    valid = false;
                                    break;
                                }
                            }
                            params.push(p);
                            if !self.eat(&TokenKind::Comma)? {
                                break;
                            }
                        }
                        _ => {
                            valid = false;
                            break;
                        }
                    }
                }
                if valid && self.eat(&TokenKind::RParen)? && self.eat(&TokenKind::FatArrow)? {
                    let (body, end) = self.parse_arrow_body()?;
                    return Ok(Some(Expr {
                        kind: ExprKind::Arrow { params, body },
                        span: Span::merge(start, end),
                    }));
                }
                self.lexer.restore(saved);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn parse_arrow_body(&mut self) -> Result<(ArrowBody, Span), ParseError> {
        if self.check(&TokenKind::LBrace)? {
            let blk = self.parse_block()?;
            let end = blk.span;
            Ok((ArrowBody::Block(blk), end))
        } else {
            let e = self.parse_expr()?;
            let end = e.span;
            Ok((ArrowBody::Expr(Box::new(e)), end))
        }
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let cond = self.parse_nullish()?;
        if self.eat(&TokenKind::Question)? {
            let then_expr = self.parse_expr()?;
            self.expect(&TokenKind::Colon)?;
            let else_expr = self.parse_ternary()?;
            let sp = Span::merge(cond.span, else_expr.span);
            let then_sp = then_expr.span;
            let else_sp = else_expr.span;
            return Ok(Expr {
                kind: ExprKind::If {
                    cond: Box::new(cond),
                    then_branch: Block {
                        stmts: Vec::new(),
                        tail_expr: Some(Box::new(then_expr)),
                        span: then_sp,
                    },
                    else_branch: Block {
                        stmts: Vec::new(),
                        tail_expr: Some(Box::new(else_expr)),
                        span: else_sp,
                    },
                },
                span: sp,
            });
        }
        Ok(cond)
    }

    fn parse_nullish(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_or()?;
        while self.eat(&TokenKind::QuestionQuestion)? {
            let rhs = self.parse_or()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::NullishCoalesce,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.eat(&TokenKind::OrOr)? {
            let rhs = self.parse_and()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_equality()?;
        while self.eat(&TokenKind::AndAnd)? {
            let rhs = self.parse_equality()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_relational()?;
        loop {
            let op = if self.eat(&TokenKind::EqEq)? {
                BinaryOp::Eq
            } else if self.eat(&TokenKind::BangEq)? {
                BinaryOp::NotEq
            } else {
                break;
            };
            let rhs = self.parse_relational()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_relational(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_additive()?;
        loop {
            let op = if self.eat(&TokenKind::Lt)? {
                BinaryOp::Lt
            } else if self.eat(&TokenKind::LtEq)? {
                BinaryOp::LtEq
            } else if self.eat(&TokenKind::Gt)? {
                BinaryOp::Gt
            } else if self.eat(&TokenKind::GtEq)? {
                BinaryOp::GtEq
            } else {
                break;
            };
            let rhs = self.parse_additive()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = if self.eat(&TokenKind::Plus)? {
                BinaryOp::Add
            } else if self.eat(&TokenKind::Minus)? {
                BinaryOp::Sub
            } else {
                break;
            };
            let rhs = self.parse_multiplicative()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = if self.eat(&TokenKind::Star)? {
                BinaryOp::Mul
            } else if self.eat(&TokenKind::Slash)? {
                BinaryOp::Div
            } else if self.eat(&TokenKind::Percent)? {
                BinaryOp::Rem
            } else {
                break;
            };
            let rhs = self.parse_unary()?;
            let sp = Span::merge(lhs.span, rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: sp,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek()?;
        match tok.kind {
            TokenKind::Minus => {
                self.next()?;
                let operand = self.parse_unary()?;
                let sp = Span::merge(tok.span, operand.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span: sp,
                })
            }
            TokenKind::Bang => {
                self.next()?;
                let operand = self.parse_unary()?;
                let sp = Span::merge(tok.span, operand.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span: sp,
                })
            }
            TokenKind::Amp => {
                self.next()?;
                let operand = self.parse_unary()?;
                let sp = Span::merge(tok.span, operand.span);
                Ok(Expr {
                    kind: ExprKind::Borrow(Box::new(operand)),
                    span: sp,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            // Speculative generic call or generic enum literal:
            // `fn_name<i64>(arg)`, `Option<i64>.Some(42)`, `Option<i64>.None`
            if self.check(&TokenKind::Lt)? {
                if let ExprKind::Variable(ref base_ident) = expr.kind {
                    let base_clone = base_ident.clone();
                    let saved = self.lexer.checkpoint();
                    let _ = self.next(); // consume '<'
                    let mut type_args = Vec::new();
                    let mut ok = true;
                    while !self.check(&TokenKind::Gt)? && !self.check(&TokenKind::Eof)? {
                        match self.parse_type_expr() {
                            Ok(te) => type_args.push(te),
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                        if !self.eat(&TokenKind::Comma)? {
                            break;
                        }
                    }
                    if ok && !type_args.is_empty() && self.eat(&TokenKind::Gt)? {
                        if self.eat(&TokenKind::LParen)? {
                            let mut args = Vec::new();
                            while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                                args.push(self.parse_expr()?);
                                if !self.eat(&TokenKind::Comma)? {
                                    break;
                                }
                            }
                            let end = self.expect(&TokenKind::RParen)?.span;
                            let sp = Span::merge(expr.span, end);
                            expr = Expr {
                                kind: ExprKind::Call {
                                    callee: base_clone,
                                    type_args,
                                    args,
                                },
                                span: sp,
                            };
                            continue;
                        } else if self.eat(&TokenKind::Dot)? {
                            let variant = self.expect_ident()?;
                            let mut args = Vec::new();
                            let end = if self.eat(&TokenKind::LParen)? {
                                while !self.check(&TokenKind::RParen)?
                                    && !self.check(&TokenKind::Eof)?
                                {
                                    args.push(self.parse_expr()?);
                                    if !self.eat(&TokenKind::Comma)? {
                                        break;
                                    }
                                }
                                self.expect(&TokenKind::RParen)?.span
                            } else {
                                variant.span
                            };
                            let sp = Span::merge(expr.span, end);
                            expr = Expr {
                                kind: ExprKind::EnumLit {
                                    enum_name: base_clone,
                                    type_args,
                                    variant,
                                    args,
                                },
                                span: sp,
                            };
                            continue;
                        }
                    }
                    self.lexer.restore(saved);
                }
            }

            if self.eat(&TokenKind::Dot)? {
                let field = self.expect_ident()?;
                // Method / Math / Enum variant call: `Math.sin(x)`, `Action.Inc(1)`, `arr.push(x)`, `arr.map(...)`
                if self.eat(&TokenKind::LParen)? {
                    let mut args = Vec::new();
                    while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                        args.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma)? {
                            break;
                        }
                    }
                    let end = self.expect(&TokenKind::RParen)?.span;
                    let sp = Span::merge(expr.span, end);

                    if let ExprKind::Variable(ref obj_ident) = expr.kind {
                        if obj_ident.name == "Math" {
                            expr = Expr {
                                kind: ExprKind::Call {
                                    callee: field,
                                    type_args: Vec::new(),
                                    args,
                                },
                                span: sp,
                            };
                            continue;
                        }
                        // Capitalized target -> Enum variant constructor (`Action.Inc(5)`)
                        if obj_ident
                            .name
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_uppercase())
                        {
                            expr = Expr {
                                kind: ExprKind::EnumLit {
                                    enum_name: obj_ident.clone(),
                                    type_args: Vec::new(),
                                    variant: field,
                                    args,
                                },
                                span: sp,
                            };
                            continue;
                        }
                    }

                    // Namespace module calls: `import * as st from "./state.tsk"; st.init(...)`
                    if let ExprKind::Variable(ref ns_ident) = expr.kind {
                        if self.namespace_imports.contains(&ns_ident.name) {
                            expr = Expr {
                                kind: ExprKind::MethodCall {
                                    receiver: Box::new(expr),
                                    method: field,
                                    args,
                                },
                                span: sp,
                            };
                            continue;
                        }
                    }

                    // Higher-order & functional collection methods
                    if matches!(
                        field.name.as_str(),
                        "map"
                            | "filter"
                            | "reduce"
                            | "some"
                            | "every"
                            | "count"
                            | "findIndex"
                            | "includes"
                            | "indexOf"
                            | "flatMap"
                            | "slice"
                            | "concat"
                            | "reverse"
                    ) {
                        expr = Expr {
                            kind: ExprKind::MethodCall {
                                receiver: Box::new(expr),
                                method: field,
                                args,
                            },
                            span: sp,
                        };
                        continue;
                    }

                    // Ergonomic collection method sugar: `v.push(x)` -> `vec_push(v, x)`, etc.
                    let mapped_callee = match field.name.as_str() {
                        "push" => Some("vec_push"),
                        "get" => Some("map_get"),
                        "set" => Some("map_set"),
                        "send" => Some("chan_send"),
                        "recv" => Some("chan_recv"),
                        _ => None,
                    };
                    if let Some(fn_name) = mapped_callee {
                        let mut full_args = Vec::with_capacity(args.len() + 1);
                        full_args.push(expr);
                        full_args.extend(args);
                        expr = Expr {
                            kind: ExprKind::Call {
                                callee: Ident::new(fn_name, field.span),
                                type_args: Vec::new(),
                                args: full_args,
                            },
                            span: sp,
                        };
                        continue;
                    }

                    return Err(ParseError::InvalidSyntax {
                        line: field.span.line,
                        msg: format!(
                            "Unsupported method '.{}()' — use top-level function call or supported methods (.map, .filter, .reduce, .some, .every, .count, .findIndex, .includes, .indexOf, .flatMap, .slice, .concat, .reverse, .push, .get, .set, Math.*)",
                            field.name
                        ),
                    });
                }

                let sp = Span::merge(expr.span, field.span);
                // `x.length` -> handled cleanly in lowerer for both vec and string!
                expr = Expr {
                    kind: ExprKind::FieldAccess {
                        target: Box::new(expr),
                        field,
                    },
                    span: sp,
                };
            } else if self.eat(&TokenKind::LParen)? {
                let mut args = Vec::new();
                while !self.check(&TokenKind::RParen)? && !self.check(&TokenKind::Eof)? {
                    args.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma)? {
                        break;
                    }
                }
                let end = self.expect(&TokenKind::RParen)?.span;
                let sp = Span::merge(expr.span, end);
                if let ExprKind::Variable(callee) = expr.kind {
                    expr = Expr {
                        kind: ExprKind::Call {
                            callee,
                            type_args: Vec::new(),
                            args,
                        },
                        span: sp,
                    };
                } else {
                    return Err(ParseError::InvalidSyntax {
                        line: sp.line,
                        msg: "Call target must be a function identifier".to_string(),
                    });
                }
            } else if self.eat(&TokenKind::LBracket)? {
                let index = self.parse_expr()?;
                let end = self.expect(&TokenKind::RBracket)?.span;
                let sp = Span::merge(expr.span, end);
                expr = Expr {
                    kind: ExprKind::Index {
                        target: Box::new(expr),
                        index: Box::new(index),
                    },
                    span: sp,
                };
            } else if self.eat(&TokenKind::As)? {
                let ty = self.parse_type_expr()?;
                let sp = Span::merge(expr.span, ty.span);
                if let TypeExprKind::Named(ref tn) = ty.kind {
                    let conv = match tn.name.as_str() {
                        "f64" | "number" => Some("to_f64"),
                        "i64" | "int" => Some("to_i64"),
                        "string" | "str" => Some("to_str"),
                        _ => None,
                    };
                    if let Some(cname) = conv {
                        expr = Expr {
                            kind: ExprKind::Call {
                                callee: Ident::new(cname, ty.span),
                                type_args: Vec::new(),
                                args: vec![expr],
                            },
                            span: sp,
                        };
                        continue;
                    }
                }
                // Non-primitive `as` cast is a no-op type hint in strict TS subset
                expr.span = sp;
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek()?;
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::Int(v),
                    span: tok.span,
                })
            }
            TokenKind::FloatLit(v) => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::Float(v),
                    span: tok.span,
                })
            }
            TokenKind::True => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    span: tok.span,
                })
            }
            TokenKind::False => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    span: tok.span,
                })
            }
            TokenKind::StrLit(s) => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::Str(s),
                    span: tok.span,
                })
            }
            TokenKind::HexColor(h) => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::ColorHex(h),
                    span: tok.span,
                })
            }
            TokenKind::TemplateLit(raw) => {
                self.next()?;
                self.parse_template_parts(&raw, tok.span)
            }
            TokenKind::LParen => {
                self.next()?;
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(inner)
            }
            TokenKind::LBracket => self.parse_array_lit(),
            TokenKind::LBrace => self.parse_object_lit(None),
            TokenKind::Lt => self.parse_jsx_element(),
            TokenKind::Ident(name) => {
                self.next()?;
                Ok(Expr {
                    kind: ExprKind::Variable(Ident::new(name, tok.span)),
                    span: tok.span,
                })
            }
            other => Err(ParseError::InvalidSyntax {
                line: tok.span.line,
                msg: format!(
                    "Unexpected token {:?} in expression at line {}, column {}",
                    other, tok.span.line, tok.span.col
                ),
            }),
        }
    }

    fn parse_array_lit(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::LBracket)?.span;
        let mut elems = Vec::new();
        while !self.check(&TokenKind::RBracket)? && !self.check(&TokenKind::Eof)? {
            elems.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma)? {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBracket)?.span;
        Ok(Expr {
            kind: ExprKind::ArrayLit(elems),
            span: Span::merge(start, end),
        })
    }

    fn parse_object_lit(&mut self, explicit_name: Option<Ident>) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::LBrace)?.span;
        let mut fields = Vec::new();
        let mut spread: Option<Box<Expr>> = None;

        while !self.check(&TokenKind::RBrace)? && !self.check(&TokenKind::Eof)? {
            if self.eat(&TokenKind::DotDotDot)? {
                let sp_expr = self.parse_expr()?;
                spread = Some(Box::new(sp_expr));
            } else {
                let fname = self.expect_ident()?;
                let fstart = fname.span;
                let value = if self.eat(&TokenKind::Colon)? {
                    self.parse_expr()?
                } else {
                    // ES6/TS shorthand property: `{ r, g, b, a }` -> `{ r: r, ... }`
                    Expr {
                        kind: ExprKind::Variable(fname.clone()),
                        span: fstart,
                    }
                };
                let fspan = Span::merge(fstart, value.span);
                fields.push(FieldInit {
                    name: fname,
                    value,
                    span: fspan,
                });
            }
            if !self.eat(&TokenKind::Comma)? {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Expr {
            kind: ExprKind::StructLit {
                name: explicit_name,
                fields,
                spread,
            },
            span: Span::merge(start, end),
        })
    }

    fn parse_template_parts(&self, raw: &str, span: Span) -> Result<Expr, ParseError> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                match chars[i + 1] {
                    'n' => lit.push('\n'),
                    'r' => lit.push('\r'),
                    't' => lit.push('\t'),
                    '\\' => lit.push('\\'),
                    '`' => lit.push('`'),
                    '$' => lit.push('$'),
                    other => {
                        lit.push('\\');
                        lit.push(other);
                    }
                }
                i += 2;
                continue;
            }
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                if !lit.is_empty() {
                    parts.push(TemplatePart::Literal(std::mem::take(&mut lit)));
                }
                i += 2;
                let mut depth = 1usize;
                let mut expr_src = String::new();
                while i < chars.len() && depth > 0 {
                    let ch = chars[i];
                    if ch == '{' {
                        depth += 1;
                        expr_src.push(ch);
                    } else if ch == '}' {
                        depth -= 1;
                        if depth > 0 {
                            expr_src.push(ch);
                        }
                    } else if ch == '"' || ch == '\'' {
                        let q = ch;
                        expr_src.push(ch);
                        i += 1;
                        while i < chars.len() {
                            expr_src.push(chars[i]);
                            if chars[i] == '\\' && i + 1 < chars.len() {
                                i += 1;
                                expr_src.push(chars[i]);
                            } else if chars[i] == q {
                                break;
                            }
                            i += 1;
                        }
                    } else {
                        expr_src.push(ch);
                    }
                    i += 1;
                }
                let mut sub = Parser::new(&expr_src);
                sub.type_aliases = self.type_aliases.clone();
                let parsed_expr = sub.parse_expr().map_err(|e| ParseError::InvalidSyntax {
                    line: span.line,
                    msg: format!("Error in template interpolation '${{{expr_src}}}': {e}"),
                })?;
                parts.push(TemplatePart::Expr(parsed_expr));
                continue;
            }
            lit.push(chars[i]);
            i += 1;
        }
        if !lit.is_empty() {
            parts.push(TemplatePart::Literal(lit));
        }

        if parts.is_empty() {
            return Ok(Expr {
                kind: ExprKind::Str(String::new()),
                span,
            });
        }
        if parts.len() == 1 {
            if let TemplatePart::Literal(s) = &parts[0] {
                return Ok(Expr {
                    kind: ExprKind::Str(s.clone()),
                    span,
                });
            }
        }
        Ok(Expr {
            kind: ExprKind::TemplateStr(parts),
            span,
        })
    }

    // ========================================================================
    // First-Class Declarative TSX / JSX Parsing
    // ========================================================================

    fn parse_jsx_element(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::Lt)?.span;

        // Fragment shorthand: `<> ... </>` -> `<Column> ... </Column>`
        if self.eat(&TokenKind::Gt)? {
            let tag = Ident::new("Column", start);
            let (children, end) = self.parse_jsx_children("")?;
            return Ok(Expr {
                kind: ExprKind::Jsx {
                    tag,
                    props: Vec::new(),
                    children,
                },
                span: Span::merge(start, end),
            });
        }

        let tag = self.expect_ident()?;
        let mut props = Vec::new();

        while !self.check(&TokenKind::Gt)?
            && !self.check(&TokenKind::Slash)?
            && !self.check(&TokenKind::Eof)?
        {
            let pname = self.expect_ident()?;
            let pstart = pname.span;
            if self.eat(&TokenKind::Assign)? {
                let ptok = self.peek()?;
                let val = match ptok.kind {
                    TokenKind::StrLit(s) => {
                        self.next()?;
                        Expr {
                            kind: ExprKind::Str(s),
                            span: ptok.span,
                        }
                    }
                    TokenKind::LBrace => {
                        self.next()?;
                        let e = self.parse_expr()?;
                        self.expect(&TokenKind::RBrace)?;
                        e
                    }
                    other => {
                        return Err(ParseError::InvalidSyntax {
                            line: ptok.span.line,
                            msg: format!(
                                "Expected string literal or '{{expr}}' for JSX prop '{}', found {:?}",
                                pname.name, other
                            ),
                        });
                    }
                };
                let pspan = Span::merge(pstart, val.span);
                props.push(JsxProp {
                    name: pname,
                    value: Some(val),
                    span: pspan,
                });
            } else {
                // Boolean shorthand prop: `<Toggle checked />` -> `checked={true}`
                props.push(JsxProp {
                    name: pname,
                    value: None,
                    span: pstart,
                });
            }
        }

        // Self-closing `<Tag ... />`
        if self.eat(&TokenKind::Slash)? {
            let end = self.expect(&TokenKind::Gt)?.span;
            return Ok(Expr {
                kind: ExprKind::Jsx {
                    tag,
                    props,
                    children: Vec::new(),
                },
                span: Span::merge(start, end),
            });
        }

        self.expect(&TokenKind::Gt)?;
        let (children, end) = self.parse_jsx_children(&tag.name)?;
        Ok(Expr {
            kind: ExprKind::Jsx {
                tag,
                props,
                children,
            },
            span: Span::merge(start, end),
        })
    }

    fn parse_jsx_children(&mut self, open_tag: &str) -> Result<(Vec<JsxChild>, Span), ParseError> {
        let mut children = Vec::new();
        loop {
            if self.lexer.is_eof() {
                return Err(ParseError::InvalidSyntax {
                    line: self.lexer.current_span().line,
                    msg: format!("Unclosed JSX element '<{open_tag}>'"),
                });
            }
            match self.lexer.peek_raw_char() {
                Some('<') => {
                    // Check if closing tag `</Tag>` or `</>`
                    let saved = self.lexer.checkpoint();
                    self.expect(&TokenKind::Lt)?;
                    if self.eat(&TokenKind::Slash)? {
                        if open_tag.is_empty() {
                            let end = self.expect(&TokenKind::Gt)?.span;
                            return Ok((children, end));
                        }
                        let close_tag = self.expect_ident()?;
                        if close_tag.name != open_tag {
                            return Err(ParseError::InvalidSyntax {
                                line: close_tag.span.line,
                                msg: format!(
                                    "Mismatched JSX closing tag '</{}>', expected '</{}>'",
                                    close_tag.name, open_tag
                                ),
                            });
                        }
                        let end = self.expect(&TokenKind::Gt)?.span;
                        return Ok((children, end));
                    }
                    // Otherwise it's a nested JSX element
                    self.lexer.restore(saved);
                    let child_el = self.parse_jsx_element()?;
                    children.push(JsxChild::Element(child_el));
                }
                Some('{') => {
                    self.expect(&TokenKind::LBrace)?;
                    // Allow empty JSX comment `{/* ... */}` which skips to `}`
                    if self.eat(&TokenKind::RBrace)? {
                        continue;
                    }
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::RBrace)?;
                    children.push(JsxChild::Expr(expr));
                }
                Some(_) => {
                    let (raw_text, span) = self.lexer.read_jsx_text();
                    let cleaned = normalize_jsx_text(&raw_text);
                    if !cleaned.is_empty() {
                        children.push(JsxChild::Text(cleaned, span));
                    }
                }
                None => break,
            }
        }
        Err(ParseError::InvalidSyntax {
            line: self.lexer.current_span().line,
            msg: format!("Unclosed JSX element '<{open_tag}>'"),
        })
    }
}

/// Normalizes React/JSX raw text runs: removes inter-line indentation while preserving
/// meaningful single spaces around inline expressions.
fn normalize_jsx_text(raw: &str) -> String {
    if !raw.contains('\n') && !raw.contains('\r') {
        if raw.trim().is_empty() {
            return String::new();
        }
        return raw.to_string();
    }
    let lines: Vec<&str> = raw.split('\n').collect();
    let mut parts = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = if idx == 0 {
            line.trim_end()
        } else if idx + 1 == lines.len() {
            line.trim_start()
        } else {
            line.trim()
        };
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    parts.join(" ")
}

fn canonical_effect_name(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "net" => "Net".to_string(),
        "ui" => "Ui".to_string(),
        "hw" | "fs" | "sys" => "Hw".to_string(),
        "async" => "Async".to_string(),
        other => other.to_string(),
    }
}
