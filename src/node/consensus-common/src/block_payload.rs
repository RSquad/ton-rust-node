/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{BlockPayload, BlockPayloadPtr, RawBuffer};
use std::{fmt, sync::Arc, time::SystemTime};

/*
    Implementation details for BlockPayload
*/

pub(crate) struct BlockPayloadImpl {
    data: RawBuffer,           //raw data
    creation_time: SystemTime, //time of block creation
}

/*
    Implementation for public BlockPayload trait
*/

impl BlockPayload for BlockPayloadImpl {
    fn data(&self) -> &RawBuffer {
        &self.data
    }

    fn get_creation_time(&self) -> std::time::SystemTime {
        self.creation_time
    }
}

/*
    Implementation for public Debug trait
*/

impl fmt::Debug for BlockPayloadImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.data)
    }
}

/*
    Implementation of BlockPayloadImpl
*/

impl BlockPayloadImpl {
    pub(crate) fn create(data: RawBuffer) -> BlockPayloadPtr {
        Arc::new(Self { data, creation_time: SystemTime::now() })
    }
}
