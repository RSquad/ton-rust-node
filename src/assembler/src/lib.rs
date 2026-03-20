/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub use debug::DbgInfo;
use std::{collections::HashMap, ops::RangeInclusive};
use ton_block::{BuilderData, Cell, SliceData};

mod errors;
pub use errors::{
    CompileError, OperationError, ParameterError, Position, ToOperationParameterError,
};

mod complex;
mod convert;
mod debug;
mod macros;
mod parse;
mod simple;

mod writer;
pub use debug::DbgPos;
pub use writer::{Unit, Units};

pub mod disasm;

// Basic types *****************************************************************
/// Operation Compilation result
type CompileResult = Result<(), OperationError>;
type CompileHandler =
    fn(&mut Engine, &[&str], destination: &mut Units, pos: DbgPos) -> CompileResult;

// CompileError::Operation handlers ***********************************************************
trait EnsureParametersCountInRange {
    fn assert_empty(&self) -> Result<(), OperationError>;
    fn assert_len(&self, _n: usize) -> Result<(), OperationError>;
    fn assert_len_in(&self, _r: RangeInclusive<usize>) -> Result<(), OperationError>;
}

impl<T> EnsureParametersCountInRange for [T] {
    fn assert_empty(&self) -> Result<(), OperationError> {
        self.assert_len_in(0..=0)
    }

    fn assert_len(&self, n: usize) -> Result<(), OperationError> {
        self.assert_len_in(n..=n)
    }

    fn assert_len_in(&self, range: RangeInclusive<usize>) -> Result<(), OperationError> {
        if &self.len() < range.start() {
            Err(OperationError::MissingRequiredParameters)
        } else if &self.len() > range.end() {
            Err(OperationError::TooManyParameters)
        } else {
            Ok(())
        }
    }
}

// Command compilation context ************************************************

#[derive(Default)]
struct CommandContext {
    operation: String,
    line_no_cmd: usize,
    char_no_cmd: usize,
    line_no_par: usize,
    char_no_par: usize,
    rule_option: Option<CompileHandler>,
}

impl CommandContext {
    fn new(
        operation: String,
        char_no_cmd: usize,
        line_no_cmd: usize,
        rule_option: Option<CompileHandler>,
    ) -> Self {
        Self { operation, line_no_cmd, char_no_cmd, line_no_par: 0, char_no_par: 0, rule_option }
    }
    fn abort<X>(&self, error: OperationError) -> Result<X, CompileError> {
        Err(CompileError::operation(
            self.line_no_cmd,
            self.char_no_cmd,
            self.operation.clone(),
            error,
        ))
    }
    fn has_command(&self) -> bool {
        self.rule_option.is_some()
    }
    fn compile(
        &mut self,
        destination: &mut Units,
        par: &mut Vec<Token>,
        engine: &mut Engine,
    ) -> Result<(), CompileError> {
        let rule = match self.rule_option.as_ref() {
            Some(rule) => rule,
            None => return Ok(()),
        };
        let (line_no, char_no) = engine.set_pos(self.line_no_par, self.char_no_par);
        let mut n = par.len();
        loop {
            let par = par[0..n].iter().map(|p| p.token).collect::<Vec<_>>();
            let pos = engine.dbgpos.clone().unwrap_or_else(|| DbgPos {
                filename: engine.source_name.clone(),
                line: self.line_no_cmd,
            });
            match rule(engine, &par, destination, pos) {
                Ok(_) => break,
                Err(OperationError::TooManyParameters) if n != 0 => {
                    n -= 1;
                }
                Err(e) => return self.abort(e),
            }
        }
        engine.set_pos(line_no, char_no);
        self.rule_option = None;
        // detecting some errors here
        if n > 1 && self.operation != "IFREFELSEREF" {
            // the only insn taking two blocks without comma between
            for token in &par[1..n] {
                if !token.was_comma {
                    return Err(CompileError::syntax(token.line, token.column, "Missing comma"));
                }
            }
        }
        par.drain(..n);
        if !par.is_empty() {
            let token = par.remove(0);
            let position = Position::new(engine.source_name.clone(), token.line, token.column);
            if token.was_comma || n == 0 {
                return Err(CompileError::Operation(
                    position,
                    self.operation.clone(),
                    OperationError::TooManyParameters,
                ));
            } else {
                // or CompileError::Syntax "missing comma"
                return Err(CompileError::UnknownOperation(position, token.token.into()));
            }
        }
        Ok(())
    }
}

// Compilation engine *********************************************************

#[allow(non_snake_case)]
pub struct Engine {
    line_no: usize,
    char_no: usize,
    source_name: String,
    handlers: HashMap<&'static str, CompileHandler>,
    named_units: HashMap<String, Unit>,
    dbgpos: Option<DbgPos>,
}

#[derive(Debug)]
struct Token<'a> {
    line: usize,
    column: usize,
    token: &'a str,
    was_comma: bool,
}

impl<'a> Token<'a> {
    fn new(line: usize, column: usize, token: &'a str, was_comma: bool) -> Self {
        Self { line, column, token, was_comma }
    }
}

impl Engine {
    pub fn new(source_name: &str) -> Self {
        let mut ret = Self {
            line_no: 1,
            char_no: 1,
            source_name: source_name.to_string(),
            handlers: HashMap::new(),
            named_units: HashMap::new(),
            dbgpos: None,
        };
        ret.add_complex_commands();
        ret.add_simple_commands();
        ret
    }

    fn is_whitespace(x: char) -> bool {
        matches!(x, ' ' | '\n' | '\r' | '\t')
    }

    fn set_pos(&mut self, line_no: usize, char_no: usize) -> (usize, usize) {
        let l = std::mem::replace(&mut self.line_no, line_no);
        let c = std::mem::replace(&mut self.char_no, char_no);
        (l, c)
    }

    pub fn build(&mut self, name: Option<String>, source: &str) -> Result<Unit, CompileError> {
        let (builder, dbg) = self.compile(source)?.finalize();
        let unit = Unit::new(builder, dbg);
        if let Some(name) = name {
            self.named_units.insert(name, unit.clone());
        }
        Ok(unit)
    }

    pub fn reset(&mut self, source_name: String) {
        self.line_no = 1;
        self.char_no = 1;
        self.source_name = source_name;
        self.dbgpos = None;
    }

    fn compile(&mut self, source: &str) -> Result<Units, CompileError> {
        self.compile_impl(source, false)
    }

    pub fn compile_toplevel(&mut self, source: &str) -> Result<Units, CompileError> {
        self.compile_impl(source, true)
    }

    fn compile_impl(&mut self, source: &str, toplevel: bool) -> Result<Units, CompileError> {
        let mut ret = Units::new();
        let mut par = Vec::new();
        let mut acc = (0, 0);
        let mut expect_comma = false;
        let mut comma_found = false;
        let mut was_comma = false; // was comma before token
        let mut was_newline = false; // was line break before token
        let mut in_block = 0;
        let mut in_comment = false;
        let mut command_ctx = CommandContext::default();
        let mut was_dot_inline = false;
        for ch in source.chars().chain(" ".chars()) {
            let mut newline_found = false;
            // Adjust line/char information
            let mut x = self.char_no;
            let y = self.line_no;
            if ch == '\n' {
                self.line_no += 1;
                self.char_no = 1
            } else {
                self.char_no += 1
            }
            let (s0, s1) = acc;
            let new_s1 = s1 + ch.len_utf8();
            // Process internal block if any
            if in_block > 0 {
                if ch == '{' {
                    in_block += 1
                } else if ch == '}' {
                    in_block -= 1
                }
                if in_block == 0 {
                    par.push(Token::new(y, x, &source[s0..s1], comma_found));
                    acc = (new_s1, new_s1)
                } else {
                    acc = (s0, new_s1)
                }
                continue;
            }
            // Process comment if any
            if in_comment {
                if (ch == '\r') || (ch == '\n') {
                    in_comment = false;
                    was_newline = true;
                }
                acc = (new_s1, new_s1);
                continue;
            }
            // Analyze char
            if Engine::is_whitespace(ch) {
                if (ch == '\r') || (ch == '\n') {
                    newline_found = true;
                    was_newline = true;
                }
                acc = (new_s1, new_s1);
                if s0 == s1 {
                    continue;
                }
            } else if ch == ';' {
                acc = (new_s1, new_s1);
                in_comment = true;
                if s0 == s1 {
                    continue;
                }
            } else if ch == ',' {
                if !expect_comma {
                    return Err(
                        CompileError::syntax(y, x, ",").with_filename(self.source_name.clone())
                    );
                }
                acc = (new_s1, new_s1);
                expect_comma = false;
                comma_found = true;
                if s0 == s1 {
                    continue;
                }
            } else if ch == '{' {
                if expect_comma || !command_ctx.has_command() {
                    return Err(
                        CompileError::syntax(y, x, ch).with_filename(self.source_name.clone())
                    );
                }
                acc = (new_s1, new_s1);
                in_block = 1;
                command_ctx.line_no_par = self.line_no;
                command_ctx.char_no_par = self.char_no;
                continue;
            } else if ch == '}' {
                return Err(CompileError::syntax(y, x, ch).with_filename(self.source_name.clone()));
            } else if ch.is_ascii_alphanumeric()
                || (ch == '-')
                || (ch == '_')
                || (ch == '.')
                || (ch == '/')
                || (ch == '\\')
                || (ch == '$')
                || (ch == '@')
            {
                acc = (s0, new_s1);
                if s0 == s1 {
                    //start of new token
                    was_comma = comma_found;
                    comma_found = false;
                    expect_comma = true
                }
                continue;
            } else {
                // TODO: (message for the owner: please write descriptive explanation)
                return Err(
                    CompileError::syntax(y, x, "Bad char").with_filename(self.source_name.clone())
                );
            }
            // Token extracted
            let token = source[s0..s1].to_ascii_uppercase();
            log::trace!(target: "tvm", "--> {}\n", token);
            x -= token.chars().count();
            let rule = if was_dot_inline {
                // Do not try matching the token if the previous one is .inline,
                // otherwise `.inline setCode` won't work since setCode gets matched as an insn
                None
            } else {
                self.handlers.get(token.as_str())
            };
            was_dot_inline = token == ".INLINE";
            match rule {
                None => {
                    if command_ctx.has_command() {
                        par.push(Token::new(y, x, &source[s0..s1], was_comma));
                        was_comma = false;
                        continue;
                    } else {
                        return Err(CompileError::unknown(y, x, &token)
                            .with_filename(self.source_name.clone()));
                    }
                }
                Some(&new_rule) => {
                    if !toplevel && token == ".FRAGMENT" {
                        return Err(CompileError::syntax(
                            y,
                            x,
                            ".fragment can be defined at toplevel scope only",
                        ));
                    }
                    match command_ctx.compile(&mut ret, &mut par, self) {
                        Ok(_) => {
                            command_ctx = CommandContext::new(token, x, y, Some(new_rule));
                            expect_comma = false;
                            was_comma = false;
                            was_newline = newline_found;
                        }
                        Err(
                            e @ CompileError::Operation(
                                _,
                                _,
                                OperationError::MissingRequiredParameters,
                            ),
                        ) => {
                            if was_newline {
                                // it seems realy new command - rturn correct missing params error
                                return Err(e);
                            } else {
                                par.push(Token::new(y, x, &source[s0..s1], was_comma));
                                was_comma = false;
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        // Compile last pending command if any
        command_ctx.compile(&mut ret, &mut par, self)?;
        if in_block != 0 {
            return Err(CompileError::syntax(self.line_no, 0, "Missing }")
                .with_filename(self.source_name.clone()));
        }
        Ok(ret)
    }
}

pub fn compile_code_to_builder(code: &str) -> Result<BuilderData, CompileError> {
    log::trace!(target: "tvm", "begin compile\n");
    Ok(Engine::new("").compile_toplevel(code)?.finalize().0)
}

pub fn compile_code(code: &str) -> Result<SliceData, CompileError> {
    let code = compile_code_to_builder(code)?;
    match SliceData::load_builder(code) {
        Ok(code) => Ok(code),
        Err(_) => Err(CompileError::unknown(0, 0, "failure while convert BuilderData to cell")),
    }
}

pub fn compile_code_to_cell(code: &str) -> Result<Cell, CompileError> {
    log::trace!(target: "tvm", "begin compile\n");
    let code = compile_code_to_builder(code)?;
    match code.into_cell() {
        Ok(code) => Ok(code),
        Err(_) => Err(CompileError::unknown(0, 0, "failure while convert BuilderData to cell")),
    }
}

pub fn compile_code_debuggable(
    source: &str,
    source_name: &str,
) -> Result<(SliceData, DbgInfo), CompileError> {
    log::trace!(target: "tvm", "begin compile\n");
    let (builder, dbg) = Engine::new(source_name).compile_toplevel(source)?.finalize();
    let cell = builder.into_cell().unwrap();
    match SliceData::load_cell(cell.clone()) {
        Ok(code) => {
            let dbg_info = DbgInfo::from(cell, dbg);
            Ok((code, dbg_info))
        }
        Err(_) => Err(CompileError::unknown(0, 0, "failure while convert BuilderData to cell")),
    }
}
