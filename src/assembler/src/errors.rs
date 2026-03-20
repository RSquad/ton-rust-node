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
use std::fmt;
use ton_block::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub filename: String,
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(filename: String, line: usize, column: usize) -> Self {
        Self { filename, line, column }
    }
}

pub type OperationName = String;
pub type ParameterName = String;
pub type Explanation = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterError {
    UnexpectedType,
    NotSupported,
    OutOfRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationError {
    Parameter(ParameterName, ParameterError),
    TooManyParameters,
    LogicErrorInParameters(&'static str),
    MissingRequiredParameters,
    MissingBlock,
    Nested(Box<CompileError>),
    NotFitInSlice,
    CellComputeError,
    CellComputeNotACell,
    CellComputeInternal,
    FragmentIsAlreadyDefined(String),
    FragmentIsNotDefined(String),
    CodeDictConstruction(String),
    Internal(String),
}

impl From<Error> for OperationError {
    fn from(e: Error) -> Self {
        Self::Internal(e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    Syntax(Position, Explanation),
    UnknownOperation(Position, OperationName),
    Operation(Position, OperationName, OperationError),
}

impl CompileError {
    pub fn syntax<S: ToString>(line: usize, column: usize, explanation: S) -> Self {
        CompileError::Syntax(
            Position { filename: String::new(), line, column },
            explanation.to_string(),
        )
    }
    pub fn unknown<S: ToString>(line: usize, column: usize, name: S) -> Self {
        CompileError::UnknownOperation(
            Position { filename: String::new(), line, column },
            name.to_string(),
        )
    }
    pub fn operation<S: ToString>(
        line: usize,
        column: usize,
        name: S,
        error: OperationError,
    ) -> Self {
        CompileError::Operation(
            Position { filename: String::new(), line, column },
            name.to_string(),
            error,
        )
    }
    pub fn missing_params<S: ToString>(line: usize, column: usize, name: S) -> Self {
        CompileError::Operation(
            Position { filename: String::new(), line, column },
            name.to_string(),
            OperationError::MissingRequiredParameters,
        )
    }
    pub fn missing_block<S: ToString>(line: usize, column: usize, name: S) -> Self {
        CompileError::Operation(
            Position { filename: String::new(), line, column },
            name.to_string(),
            OperationError::MissingBlock,
        )
    }
    pub fn too_many_params<S: ToString>(line: usize, column: usize, name: S) -> Self {
        CompileError::Operation(
            Position { filename: String::new(), line, column },
            name.to_string(),
            OperationError::TooManyParameters,
        )
    }
    pub fn out_of_range<S1: ToString, S2: ToString>(
        line: usize,
        column: usize,
        name: S1,
        param: S2,
    ) -> Self {
        let operation = OperationError::Parameter(param.to_string(), ParameterError::OutOfRange);
        CompileError::Operation(
            Position { filename: String::new(), line, column },
            name.to_string(),
            operation,
        )
    }
    pub fn with_filename(mut self, filename: String) -> Self {
        match self {
            Self::Syntax(ref mut pos, _) => {
                pos.filename = filename;
            }
            Self::UnknownOperation(ref mut pos, _) => {
                pos.filename = filename;
            }
            Self::Operation(ref mut pos, _, _) => {
                pos.filename = filename;
            }
        };
        self
    }
    pub fn unexpected_type<S1: ToString, S2: ToString>(
        line: usize,
        column: usize,
        name: S1,
        param: S2,
    ) -> Self {
        let operation =
            OperationError::Parameter(param.to_string(), ParameterError::UnexpectedType);
        CompileError::operation(line, column, name.to_string(), operation)
    }
    pub fn logic_error<S: ToString>(
        line: usize,
        column: usize,
        name: S,
        error: &'static str,
    ) -> Self {
        let operation = OperationError::LogicErrorInParameters(error);
        CompileError::operation(line, column, name.to_string(), operation)
    }
}

pub trait ToOperationParameterError<T>
where
    T: Into<ParameterName>,
{
    type Output;
    fn parameter(self, name: T) -> Self::Output;
}

impl<T, S> ToOperationParameterError<S> for Result<T, ParameterError>
where
    S: Into<ParameterName>,
{
    type Output = Result<T, OperationError>;

    fn parameter(self, name: S) -> Result<T, OperationError> {
        self.map_err(|e| e.parameter(name))
    }
}

impl<S> ToOperationParameterError<S> for ParameterError
where
    S: Into<ParameterName>,
{
    type Output = OperationError;
    fn parameter(self, name: S) -> OperationError {
        OperationError::Parameter(name.into(), self)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}:{}", self.filename, self.line, self.column)
    }
}

impl fmt::Display for ParameterError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParameterError::UnexpectedType => write!(f, "Unexpected parameter type."),
            ParameterError::NotSupported => {
                write!(f, "Parameter value is correct, however it's not supported yet.")
            }
            ParameterError::OutOfRange => write!(f, "Parameter value is out of range"),
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fn indent(text: String) -> String {
            let mut indented = "".to_string();
            for line in text.split('\n') {
                if line.is_empty() {
                    break;
                }
                indented += "  ";
                indented += line;
            }
            indented
        }
        use OperationError::*;
        match self {
            Parameter(name, error) => {
                write!(f, "Operation parameter {} has the following problem: {}", name, error)
            }
            TooManyParameters => write!(f, "Operation has too many parameters."),
            LogicErrorInParameters(ref error) => write!(f, "Logic error {}", error),
            MissingRequiredParameters => {
                write!(f, "Operation requires more parameters.")
            }
            MissingBlock => {
                write!(f, "Operation requires block in {{}} braces.")
            }
            Nested(error) => write!(f, "\n{}", indent(error.to_string())),
            NotFitInSlice => write!(f, "Command bytecode is too long for single slice"),
            CellComputeError => {
                write!(f, "Cell computation results in an error or non-zero exit code")
            }
            CellComputeNotACell => write!(f, "Top of the stack is not a cell"),
            CellComputeInternal => write!(f, "Failed to compute the cell"),
            FragmentIsAlreadyDefined(name) => write!(f, "Fragment {} is already defined", name),
            FragmentIsNotDefined(name) => write!(f, "Fragment {} is not defined", name),
            CodeDictConstruction(message) => {
                write!(f, "Failed to construct code dictionary {}", message)
            }
            Internal(message) => write!(f, "{}", message),
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CompileError::Syntax(position, explanation) => {
                write!(f, "{} Syntax error: {}", position, explanation)
            }
            CompileError::UnknownOperation(position, name) => {
                write!(f, "{} Unknown operation {}", position, name)
            }
            CompileError::Operation(position, name, error) => {
                write!(f, "Instruction {} at {}: {}", name, position, error)
            }
        }
    }
}
