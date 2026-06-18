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
use crate::{
    executor::{
        engine::storage::swap,
        microcode::{CTRL, SAVELIST, VAR},
        Engine,
    },
    stack::{
        continuation::{ContinuationData, ContinuationType},
        StackItem,
    },
};
use ton_block::{Cell, ExceptionCode, SliceData};

#[test]
fn test_swap_with_any() {
    let mut engine =
        Engine::with_capabilities(0).setup(Default::default(), None, None, None, vec![]).unwrap();
    let mut c0 = ContinuationData::new_empty();
    let mut c1 = ContinuationData::new_empty();
    let s0 = StackItem::Cell(SliceData::new(vec![1, 2, 3, 4, 5]).into_cell().unwrap());
    let s1 = StackItem::Cell(SliceData::new(vec![6, 7, 8, 9, 0]).into_cell().unwrap());
    c0.put_to_savelist(4, s0.clone()).unwrap();
    c1.put_to_savelist(5, s1.clone()).unwrap();
    engine.cmd.push_var(StackItem::continuation(c0));
    engine.cmd.push_var(StackItem::continuation(c1));
    swap(&mut engine, var!(0), ctrl!(0)).unwrap();
    swap(&mut engine, var!(1), ctrl!(1)).unwrap();
    swap(&mut engine, savelist!(ctrl!(0), 4), savelist!(ctrl!(1), 5)).unwrap();
    let ctrls = engine.ctrl(0).unwrap();
    let cont = ctrls.as_continuation().unwrap();
    assert_eq!(cont.savelist.get(4).unwrap(), &s1);
    let ctrls = engine.ctrl(1).unwrap();
    let cont = ctrls.as_continuation().unwrap();
    assert_eq!(cont.savelist.get(5).unwrap(), &s0);
}

#[test]
fn test_swap_with_none() {
    let mut engine =
        Engine::with_capabilities(0).setup(Default::default(), None, None, None, vec![]).unwrap();
    engine.cmd.push_var(StackItem::Cell(Cell::default()));
    engine.cmd.push_var(StackItem::None);
    //try to put CELL to c4 - Ok
    swap(&mut engine, var!(0), ctrl!(4)).unwrap();
    assert_ne!(engine.cmd.var(0), &StackItem::None);
    //try to put NULL to c4 - Type Check Error
    swap(&mut engine, var!(0), ctrl!(4)).unwrap();
    assert_eq!(
        crate::error::tvm_exception_code(&swap(&mut engine, var!(1), ctrl!(4)).unwrap_err()),
        Some(ExceptionCode::TypeCheckError)
    );
    // try to put NULL to c2 - Type Check Error
    assert!(!engine.ctrl(2).unwrap().is_null());
    assert_eq!(
        crate::error::tvm_exception_code(&swap(&mut engine, var!(1), ctrl!(2)).unwrap_err()),
        Some(ExceptionCode::TypeCheckError)
    );
    assert_ne!(engine.ctrl(2).unwrap(), &StackItem::None);
    // try to put CONT to c2 - Ok
    let c2_before = engine.ctrl(2).unwrap().clone();
    assert_eq!(c2_before.as_continuation().unwrap().type_of, ContinuationType::ExcQuit);
    engine.cmd.vars[0] = StackItem::continuation(ContinuationData::new_empty());
    swap(&mut engine, var!(0), ctrl!(2)).unwrap();
    assert_eq!(engine.cmd.var(0), &c2_before);
}

#[test]
fn test_swap_with_ctrl() {
    let mut engine =
        Engine::with_capabilities(0).setup(Default::default(), None, None, None, vec![]).unwrap();
    let c0 = ContinuationData::new_empty();
    let c1 = ContinuationData::new_empty();
    engine.cmd.push_var(StackItem::continuation(c0));
    engine.cmd.push_var(StackItem::continuation(c1));
    swap(&mut engine, var!(0), ctrl!(0)).unwrap();
    swap(&mut engine, var!(0), savelist!(ctrl!(0), 0)).unwrap();
    assert_eq!(engine.cmd.var(0), &StackItem::None);
    swap(&mut engine, var!(1), ctrl!(1)).unwrap();
    swap(&mut engine, var!(1), savelist!(ctrl!(1), 1)).unwrap();
    assert_eq!(engine.cmd.var(1), &StackItem::None);
}
