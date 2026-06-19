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
        engine::{storage::fetch_stack, Engine},
        types::Instruction,
    },
    stack::{
        integer::{behavior::Quiet, conversion::FromInt, math::Round, IntegerData},
        StackItem,
    },
};
use ton_block::{fail, ExceptionCode, Result, Status};

pub mod gas_state;

fn gramtogas(engine: &Engine, nanocoins: &IntegerData) -> Result<i64> {
    let gas_price = IntegerData::from_i64(engine.get_gas().get_gas_price());
    let (gas, _) = nanocoins.div::<Quiet>(&gas_price, Round::FloorToZero)?;
    let ret = gas.take_value_of(|x| i64::from_int(x).ok()).unwrap_or(i64::MAX);
    Ok(ret)
}
fn setgaslimit(engine: &mut Engine, gas_limit: i64) -> Status {
    if gas_limit < engine.gas_used() {
        fail!(ExceptionCode::OutOfGas);
    }
    engine.new_gas_limit(gas_limit);
    Ok(())
}

// Application-specific primitives - A.10; Gas-related primitives - A.10.2
// ACCEPT - F800
pub fn execute_accept(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ACCEPT"))?;
    engine.new_gas_limit(i64::MAX);
    Ok(())
}
// Application-specific primitives - A.11; Gas-related primitives - A.11.2
// SETGASLIMIT - F801
pub fn execute_setgaslimit(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETGASLIMIT"))?;
    fetch_stack(engine, 1)?;
    let gas_limit = engine.cmd.var(0).as_integer()?.take_value_of(|x| i64::from_int(x).ok())?;
    setgaslimit(engine, gas_limit)
}
// Application-specific primitives - A.11; Gas-related primitives - A.11.2
// BUYGAS - F802
pub fn execute_buygas(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BUYGAS"))?;
    fetch_stack(engine, 1)?;
    let nanocoins = engine.cmd.var(0).as_integer()?;
    let gas_limit = gramtogas(engine, nanocoins)?;
    setgaslimit(engine, gas_limit)
}
// Application-specific primitives - A.11; Gas-related primitives - A.11.2
// GRAMTOGAS - F804
pub fn execute_gramtogas(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("GRAMTOGAS"))?;
    fetch_stack(engine, 1)?;
    let nanocoins_input = engine.cmd.var(0);
    let gas = if nanocoins_input.as_integer()?.is_neg() {
        0
    } else {
        let nanocoins = nanocoins_input.as_integer()?;
        gramtogas(engine, nanocoins)?
    };
    engine.cc.stack.push(int!(gas));
    Ok(())
}

// Returns gas consumed by VM so far (including this instruction).
pub fn execute_gasconsumed(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("GASCONSUMED"))?;
    let gas = engine.gas_used();
    engine.cc.stack.push(int!(gas));
    Ok(())
}

// Application-specific primitives - A.10; Gas-related primitives - A.10.2
// GASTOGRAM - F805
pub fn execute_gastogram(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("GASTOGRAM"))?;
    fetch_stack(engine, 1)?;
    let gas = engine.cmd.var(0).as_integer()?;
    let gas_price = engine.get_gas().get_gas_price();
    let nanocoin_output = gas.mul::<Quiet>(&IntegerData::from_i64(gas_price))?;
    engine.cc.stack.push(StackItem::int(nanocoin_output));
    Ok(())
}

// Application-specific primitives - A.11; Gas-related primitives - A.11.2
// COMMIT - F80F
pub fn execute_commit(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("COMMIT"))?;
    engine.try_commit()
}
