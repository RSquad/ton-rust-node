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
use crate::stack::{savelist::SaveList, SliceData, Stack, StackItem};
use std::{fmt, mem};
use ton_block::{error, Cell, ExceptionCode, Result};

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum ContinuationType {
    AgainLoopBody(SliceData),
    TryCatch,
    #[default]
    Ordinary,
    PushInt(i32),
    Quit(i32),
    RepeatLoopBody(SliceData, isize),
    UntilLoopCondition(SliceData),
    WhileLoopCondition(SliceData, SliceData),
    ExcQuit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContinuationData {
    code: SliceData,
    pub nargs: isize,
    pub savelist: SaveList,
    pub stack: Stack,
    pub type_of: ContinuationType,
}

impl ContinuationData {
    pub const fn new_empty() -> Self {
        Self {
            code: SliceData::new_empty(),
            nargs: -1,
            savelist: SaveList::new(),
            stack: Stack::new(),
            type_of: ContinuationType::Ordinary,
        }
    }

    pub fn move_without_stack(cont: &mut ContinuationData, body: SliceData) -> Self {
        debug_assert!(cont.code.is_empty_bitstring());
        debug_assert!(cont.nargs < 0);
        debug_assert!(cont.savelist.is_empty());
        Self {
            code: mem::replace(&mut cont.code, body),
            nargs: -1,
            savelist: Default::default(),
            stack: Stack::new(),
            type_of: mem::take(&mut cont.type_of),
        }
    }

    pub fn copy_without_stack(&self) -> Self {
        Self {
            code: self.code.clone(),
            nargs: self.nargs,
            savelist: self.savelist.clone(),
            stack: Stack::new(),
            type_of: self.type_of.clone(),
        }
    }

    pub fn code(&self) -> &SliceData {
        &self.code
    }

    pub fn code_mut(&mut self) -> &mut SliceData {
        &mut self.code
    }

    pub fn can_put_to_savelist_once(&self, i: usize) -> bool {
        self.savelist.get(i).is_none()
    }

    pub fn move_to_end(&mut self) {
        self.code = SliceData::default()
    }

    pub fn put_to_savelist(&mut self, i: usize, val: StackItem) -> Result<Option<StackItem>> {
        self.savelist.put(i, val)
    }

    pub fn remove_from_savelist(&mut self, i: usize) -> Option<StackItem> {
        self.savelist.remove(i)
    }

    pub fn with_code_and_stack(code: SliceData, stack: Stack) -> Self {
        ContinuationData {
            code,
            nargs: -1,
            savelist: SaveList::new(),
            stack,
            type_of: ContinuationType::Ordinary,
        }
    }

    pub fn with_code(code: SliceData) -> Self {
        Self::with_code_and_stack(code, Stack::new())
    }

    pub fn with_type(type_of: ContinuationType) -> Self {
        ContinuationData {
            code: SliceData::default(),
            nargs: -1,
            savelist: SaveList::new(),
            stack: Stack::new(),
            type_of,
        }
    }

    pub fn withdraw(&mut self) -> Self {
        mem::take(self)
    }

    pub fn drain_reference(&mut self) -> Result<Cell> {
        self.code
            .checked_drain_reference()
            .map_err(|err| error!(ExceptionCode::InvalidOpcode, "drain_reference failed: {}", err))
    }
}

impl Default for ContinuationData {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl fmt::Display for ContinuationData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{{\n    type: {}\n    code: {}    nargs: {}\n    stack: ",
            self.type_of, self.code, self.nargs
        )?;
        if self.stack.depth() == 0 {
            writeln!(f, "empty")?;
        } else {
            writeln!(f)?;
            for x in self.stack.storage.iter() {
                write!(f, "        {}", x)?;
                writeln!(f)?;
            }
        }
        write!(f, "    savelist: ")?;
        if self.savelist.is_empty() {
            writeln!(f, "empty")?;
        } else {
            writeln!(f)?;
            for i in SaveList::REGS {
                if let Some(item) = self.savelist.get(i) {
                    writeln!(f, "        {}: {}", i, item)?
                }
            }
        }
        write!(f, "}}")
    }
}

impl fmt::Display for ContinuationType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let name = match self {
            ContinuationType::AgainLoopBody(_) => "again",
            ContinuationType::TryCatch => "try-catch",
            ContinuationType::Ordinary => "ordinary",
            ContinuationType::PushInt(_) => "pushint",
            ContinuationType::Quit(_) => "quit",
            ContinuationType::RepeatLoopBody(_, _) => "repeat",
            ContinuationType::UntilLoopCondition(_) => "until",
            ContinuationType::WhileLoopCondition(_, _) => "while",
            ContinuationType::ExcQuit => "exception-quit",
        };
        write!(f, "{}", name)
    }
}
