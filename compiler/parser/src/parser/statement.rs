// Copyright (C) 2019-2022 Aleo Systems Inc.
// This file is part of the Leo library.

// The Leo library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The Leo library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the Leo library. If not, see <https://www.gnu.org/licenses/>.

use super::*;

use leo_errors::{ParserError, Result};
use leo_span::sym;

const ASSIGN_TOKENS: &[Token] = &[Token::Assign];

impl ParserContext<'_> {
    ///
    /// Returns an [`Identifier`] AST node if the given [`Expression`] AST node evaluates to an
    /// identifier access. The access is stored in the given accesses.
    ///
    pub fn construct_assignee_access(expr: Expression, _accesses: &mut [AssigneeAccess]) -> Result<Identifier> {
        match expr {
            Expression::Identifier(id) => Ok(id),
            _ => return Err(ParserError::invalid_assignment_target(expr.span()).into()),
        }
    }

    ///
    /// Returns an [`Assignee`] AST node from the given [`Expression`] AST node with accesses.
    ///
    pub fn construct_assignee(expr: Expression) -> Result<Assignee> {
        let expr_span = expr.span().clone();
        let mut accesses = Vec::new();
        let identifier = Self::construct_assignee_access(expr, &mut accesses)?;

        Ok(Assignee {
            span: expr_span,
            identifier,
            accesses,
        })
    }

    ///
    /// Returns a [`Statement`] AST node if the next tokens represent a statement.
    ///
    pub fn parse_statement(&mut self) -> Result<Statement> {
        match &self.token.token {
            Token::Return => Ok(Statement::Return(self.parse_return_statement()?)),
            Token::If => Ok(Statement::Conditional(self.parse_conditional_statement()?)),
            Token::For => Ok(Statement::Iteration(Box::new(self.parse_loop_statement()?))),
            Token::Console => Ok(Statement::Console(self.parse_console_statement()?)),
            Token::Let | Token::Const => Ok(Statement::Definition(self.parse_definition_statement()?)),
            Token::LeftCurly => Ok(Statement::Block(self.parse_block()?)),
            _ => Ok(self.parse_assign_statement()?),
        }
    }

    ///
    /// Returns a [`Block`] AST node if the next tokens represent a assign, or expression statement.
    ///
    pub fn parse_assign_statement(&mut self) -> Result<Statement> {
        let expr = self.parse_expression()?;

        if self.eat_any(ASSIGN_TOKENS) {
            let value = self.parse_expression()?;
            let assignee = Self::construct_assignee(expr)?;
            self.expect(&Token::Semicolon)?;
            Ok(Statement::Assign(Box::new(AssignStatement {
                span: &assignee.span + value.span(),
                assignee,
                // Currently only `=` so this is alright.
                operation: AssignOperation::Assign,
                value,
            })))
        } else {
            self.expect(&Token::Semicolon)?;
            Ok(Statement::Expression(ExpressionStatement {
                span: expr.span().clone(),
                expression: expr,
            }))
        }
    }

    /// Returns a [`Block`] AST node if the next tokens represent a block of statements.
    pub fn parse_block(&mut self) -> Result<Block> {
        let start = self.expect(&Token::LeftCurly)?;

        let mut statements = Vec::new();
        loop {
            if self.eat(&Token::RightCurly) {
                return Ok(Block {
                    span: &start + &self.prev_token.span,
                    statements,
                });
            }

            statements.push(self.parse_statement()?);
        }
    }

    /// Returns a [`ReturnStatement`] AST node if the next tokens represent a return statement.
    pub fn parse_return_statement(&mut self) -> Result<ReturnStatement> {
        let start = self.expect(&Token::Return)?;
        let expression = self.parse_expression()?;
        self.expect(&Token::Semicolon)?;
        let span = &start + expression.span();
        Ok(ReturnStatement { span, expression })
    }

    /// Returns a [`ConditionalStatement`] AST node if the next tokens represent a conditional statement.
    pub fn parse_conditional_statement(&mut self) -> Result<ConditionalStatement> {
        let start = self.expect(&Token::If)?;
        self.disallow_circuit_construction = true;
        let expr = self.parse_conditional_expression()?;
        self.disallow_circuit_construction = false;
        let body = self.parse_block()?;
        let next = if self.eat(&Token::Else) {
            let s = self.parse_statement()?;
            if !matches!(s, Statement::Block(_) | Statement::Conditional(_)) {
                self.emit_err(ParserError::unexpected_statement(&s, "Block or Conditional", s.span()));
            }
            Some(Box::new(s))
        } else {
            None
        };

        Ok(ConditionalStatement {
            span: &start + next.as_ref().map(|x| x.span()).unwrap_or(&body.span),
            condition: expr,
            block: body,
            next,
        })
    }

    /// Returns an [`IterationStatement`] AST node if the next tokens represent an iteration statement.
    pub fn parse_loop_statement(&mut self) -> Result<IterationStatement> {
        let start_span = self.expect(&Token::For)?;
        let ident = self.expect_ident()?;
        self.expect(&Token::In)?;

        // Parse iteration range.
        let start = self.parse_expression()?;
        self.expect(&Token::DotDot)?;
        let inclusive = self.eat(&Token::Assign);
        self.disallow_circuit_construction = true;
        let stop = self.parse_conditional_expression()?;
        self.disallow_circuit_construction = false;

        let block = self.parse_block()?;

        Ok(IterationStatement {
            span: start_span + block.span.clone(),
            variable: ident,
            start,
            stop,
            inclusive,
            block,
        })
    }

    /// Returns a [`ConsoleArgs`] AST node if the next tokens represent a formatted string.
    pub fn parse_console_args(&mut self) -> Result<ConsoleArgs> {
        let mut string = None;
        let (parameters, _, span) = self.parse_paren_comma_list(|p| {
            if string.is_none() {
                p.bump();
                let SpannedToken { token, span } = p.prev_token.clone();
                string = Some(match token {
                    Token::StringLit(chars) => chars,
                    _ => {
                        p.emit_err(ParserError::unexpected_str(token, "formatted string", &span));
                        Vec::new()
                    }
                });
                Ok(None)
            } else {
                p.parse_expression().map(Some)
            }
        })?;

        Ok(ConsoleArgs {
            string: string.unwrap_or_default(),
            span,
            parameters,
        })
    }

    /// Returns a [`ConsoleStatement`] AST node if the next tokens represent a console statement.
    pub fn parse_console_statement(&mut self) -> Result<ConsoleStatement> {
        let keyword = self.expect(&Token::Console)?;
        self.expect(&Token::Dot)?;
        let function = self.expect_ident()?;
        let function = match function.name {
            sym::assert => {
                self.expect(&Token::LeftParen)?;
                let expr = self.parse_expression()?;
                self.expect(&Token::RightParen)?;
                ConsoleFunction::Assert(expr)
            }
            sym::error => ConsoleFunction::Error(self.parse_console_args()?),
            sym::log => ConsoleFunction::Log(self.parse_console_args()?),
            x => {
                // Not sure what it is, assume it's `log`.
                self.emit_err(ParserError::unexpected_ident(
                    x,
                    &["assert", "error", "log"],
                    &function.span,
                ));
                ConsoleFunction::Log(self.parse_console_args()?)
            }
        };
        self.expect(&Token::Semicolon)?;

        Ok(ConsoleStatement {
            span: &keyword + function.span(),
            function,
        })
    }

    /// Returns a [`VariableName`] AST node if the next tokens represent a variable name with
    /// valid keywords.
    pub fn parse_variable_name(&mut self, decl_ty: Declare, span: &Span) -> Result<VariableName> {
        if self.eat(&Token::Mut) {
            self.emit_err(ParserError::let_mut_statement(&(&self.prev_token.span + span)));
        }

        let name = self.expect_ident()?;
        Ok(VariableName {
            span: name.span.clone(),
            mutable: matches!(decl_ty, Declare::Let),
            identifier: name,
        })
    }

    /// Returns a [`DefinitionStatement`] AST node if the next tokens represent a definition statement.
    pub fn parse_definition_statement(&mut self) -> Result<DefinitionStatement> {
        self.expect_any(&[Token::Let, Token::Const])?;
        let decl_span = self.prev_token.span.clone();
        let decl_type = match &self.prev_token.token {
            Token::Let => Declare::Let,
            Token::Const => Declare::Const,
            _ => unreachable!("parse_definition_statement_ shouldn't produce this"),
        };
        dbg!();

        // Parse variable names.
        let variable_names = if self.peek_is_left_par() {
            let vars = self
                .parse_paren_comma_list(|p| p.parse_variable_name(decl_type, &decl_span).map(Some))
                .map(|(vars, ..)| vars)?;

            if vars.len() == 1 {
                self.emit_err(ParserError::invalid_parens_around_single_variable(vars[0].span()));
            }

            vars
        } else {
            vec![self.parse_variable_name(decl_type, &decl_span)?]
        };

        // Parse an optional type ascription.
        let type_ = self
            .eat(&Token::Colon)
            .then(|| self.parse_type().map(|t| t.0))
            .transpose()?;

        self.expect(&Token::Assign)?;
        let expr = self.parse_expression()?;
        self.expect(&Token::Semicolon)?;

        Ok(DefinitionStatement {
            span: &decl_span + expr.span(),
            declaration_type: decl_type,
            variable_names,
            type_,
            value: expr,
        })
    }
}
