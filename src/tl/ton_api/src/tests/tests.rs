/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{serialize_boxed, ton::adnl::Message as AdnlMessage};

#[test]
fn test_enum_serialization() {
    let msg = AdnlMessage::Adnl_Message_Nop;
    serialize_boxed(&msg).unwrap();
}
