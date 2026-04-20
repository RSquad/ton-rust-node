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
#[derive(Debug, thiserror::Error)]
pub enum BlockError {
    /// Fatal error.
    #[error("Fatal error: {0}")]
    FatalError(String),
    /// Invalid argument.
    #[error("Invalid argument: {0}")]
    InvalidArg(String),
    /// Invalid TL-B constructor tag.
    #[error("Invalid TL-B constructor tag `#{:x}` while parsing `{}` struct", .t, .s)]
    InvalidConstructorTag { t: u32, s: String },
    /// Invalid data.
    #[error("Invalid data: {0}")]
    InvalidData(String),
    /// Invalid index.
    #[error("Invalid index: {0}")]
    InvalidIndex(usize),
    /// Invalid operation.
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    /// Item is not found.
    #[error("{0} is not found")]
    NotFound(String),
    /// Other error.
    #[error("{0}")]
    Other(String),
    /// Attempting to read data from pruned branch cell.
    #[error("Attempting to read {0} from pruned branch cell")]
    PrunedCellAccess(String),
    /// Wrong hash.
    #[error("Wrong hash")]
    WrongHash,
    /// Wrong merkle proof.
    #[error("Wrong merkle proof: {0}")]
    WrongMerkleProof(String),
    /// Wrong merkle update.
    #[error("Wrong merkle update: {0}")]
    WrongMerkleUpdate(String),
    #[error("Bad signature")]
    BadSignature,
    #[error("Duplicated pubkey")]
    DuplicatedSignature,
    #[error("Unexpected struct variant: exp={0} real={1}")]
    UnexpectedStructVariant(String, String),
    #[error("Mismatched serde options: {0} exp={1} real={2}")]
    MismatchedSerdeOptions(String, usize, usize),
    #[error("OutAction deserialize error {0}, mode {1}")]
    OutActionError(#[source] crate::Error, u8),
}

// Exception codes *****************************************************************

#[derive(Clone, Copy, Debug, num_derive::FromPrimitive, PartialEq, Eq, thiserror::Error)]
pub enum ExceptionCode {
    #[error("normal termination")]
    NormalTermination = 0,
    #[error("alternative termination")]
    AlternativeTermination = 1,
    #[error("stack underflow")]
    StackUnderflow = 2,
    #[error("stack overflow")]
    StackOverflow = 3,
    #[error("integer overflow")]
    IntegerOverflow = 4,
    #[error("range check error")]
    RangeCheckError = 5,
    #[error("invalid opcode")]
    InvalidOpcode = 6,
    #[error("type check error")]
    TypeCheckError = 7,
    #[error("cell overflow")]
    CellOverflow = 8,
    #[error("cell underflow")]
    CellUnderflow = 9,
    #[error("dictionaty error")]
    DictionaryError = 10,
    #[error("unknown error")]
    UnknownError = 11,
    #[error("fatal error")]
    FatalError = 12,
    #[error("out of gas")]
    OutOfGas = 13,
    #[error("illegal instruction")]
    IllegalInstruction = 14,
    #[error("pruned cell")]
    PrunedCellAccess = 15,
}

impl ExceptionCode {
    pub fn from_i32(number: i32) -> Option<ExceptionCode> {
        num::FromPrimitive::from_i32(number)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ExceptionType {
    System(ExceptionCode),
    Custom(i32),
}

impl ExceptionType {
    fn is_normal_termination(&self) -> Option<i32> {
        match self {
            ExceptionType::System(ExceptionCode::NormalTermination) | ExceptionType::Custom(0) => {
                Some(0)
            }
            ExceptionType::System(ExceptionCode::AlternativeTermination)
            | ExceptionType::Custom(1) => Some(1),
            _ => None,
        }
    }
    fn exception_code(&self) -> Option<ExceptionCode> {
        if let ExceptionType::System(code) = self {
            Some(*code)
        } else {
            None
        }
    }
    fn custom_code(&self) -> Option<i32> {
        if let ExceptionType::Custom(code) = self {
            Some(*code)
        } else {
            None
        }
    }
    pub fn exception_or_custom_code(&self) -> i32 {
        match self {
            ExceptionType::System(code) => *code as i32,
            ExceptionType::Custom(code) => *code,
        }
    }
    fn exception_message(&self) -> String {
        match self {
            ExceptionType::System(code) => format!("{}, code {}", code, *code as u8),
            ExceptionType::Custom(code) => format!("code {}", code),
        }
    }
}

// Exceptions *****************************************************************
// TODO: remove file, line
#[derive(Debug, PartialEq, thiserror::Error)]
pub struct Exception {
    exception: ExceptionType,
    pub comment: String,
    pub file: &'static str,
    pub line: u32,
}

impl From<ExceptionCode> for Exception {
    fn from(code: ExceptionCode) -> Self {
        Exception::from_code(code, String::new(), file!(), line!())
    }
}

impl std::fmt::Display for Exception {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, {}, {}:{}",
            self.exception.exception_message(),
            self.comment,
            self.file,
            self.line
        )
    }
}

impl Exception {
    pub fn from_code(
        code: ExceptionCode,
        comment: String,
        file: &'static str,
        line: u32,
    ) -> Exception {
        Exception { exception: ExceptionType::System(code), comment, file, line }
    }
    pub fn from_number(number: i32, comment: String, file: &'static str, line: u32) -> Exception {
        Exception { exception: ExceptionType::Custom(number), comment, file, line }
    }
    pub fn exception_code(&self) -> Option<ExceptionCode> {
        self.exception.exception_code()
    }
    pub fn custom_code(&self) -> Option<i32> {
        self.exception.custom_code()
    }
    pub fn exception_or_custom_code(&self) -> i32 {
        self.exception.exception_or_custom_code()
    }
    pub fn is_normal_termination(&self) -> Option<i32> {
        self.exception.is_normal_termination()
    }
}

pub type Error = anyhow::Error;
pub type Result<T> = std::result::Result<T, Error>;
pub type Failure = Option<Error>;
pub type Status = Result<()>;

#[macro_export]
macro_rules! error {
    ($error:literal) => {
        anyhow::format_err!($error)
    };
    ($error:expr) => {
        anyhow::Error::from($error)
    };
    ($fmt:literal $(, $args:expr)* $(,)?) => {
        anyhow::format_err!($fmt $(, $args)*)
    };
    ($error:expr, $fmt:literal $(, $args:expr)* $(,)?) => {
        anyhow::Error::from($crate::Exception::from_code(
            $error,
            format!($fmt $(, $args)*),
            file!(),
            line!()
        ))
    };
}

#[macro_export]
macro_rules! fail {
    ($error:literal) => {
        return Err(anyhow::format_err!($error))
    };
    ($error:expr) => {
        return Err(anyhow::Error::from($error))
    };
    ($fmt:literal $(, $args:expr)* $(,)?) => {
        return Err(anyhow::format_err!($fmt $(, $args)*))
    };
    ($error:expr, $fmt:literal $(, $args:expr)* $(,)?) => {
        return Err(anyhow::Error::from($crate::Exception::from_code(
            $error,
            format!($fmt $(, $args)*),
            file!(),
            line!()
        )))
    };
}
