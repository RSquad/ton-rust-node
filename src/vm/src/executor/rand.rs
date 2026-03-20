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
        integer::{behavior::Signaling, IntegerData},
        StackItem,
    },
};
use ton_block::{sha512_digest, Sha256, Status};

// (x - )
pub(crate) fn execute_addrand(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ADDRAND"))?;
    fetch_stack(engine, 1)?;
    let mut hasher = Sha256::new();
    hasher.update(engine.rand()?.as_u256()?);
    hasher.update(engine.cmd.var(0).as_integer()?.as_u256()?);
    let sha256 = hasher.finalize();
    engine.set_rand(IntegerData::from_u256(sha256)?)?;
    Ok(())
}

// (y - z)
pub(crate) fn execute_rand(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RAND"))?;
    fetch_stack(engine, 1)?;
    let sha512 = sha512_digest(engine.rand()?.as_u256()?);
    let value = IntegerData::from_unsigned_bytes_be(&sha512[32..]);
    let rand = value.mul_shr256::<Signaling>(engine.cmd.var(0).as_integer()?)?;
    engine.cc.stack.push(StackItem::integer(rand));
    engine.set_rand(IntegerData::from_u256(&sha512[..32])?)?;
    Ok(())
}

// ( - x)
pub(crate) fn execute_randu256(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RANDU256"))?;
    let sha512 = sha512_digest(engine.rand()?.as_u256()?);
    engine.set_rand(IntegerData::from_u256(&sha512[..32])?)?;
    engine.cc.stack.push(StackItem::int(IntegerData::from_u256(&sha512[32..])?));
    Ok(())
}

// (x - )
pub(crate) fn execute_setrand(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SETRAND"))?;
    fetch_stack(engine, 1)?;
    let rand = engine.cmd.var(0).as_integer()?.clone();
    engine.set_rand(rand)?;
    Ok(())
}
