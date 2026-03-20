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
use crate::network::full_node_overlay_client::FullNodeOverlayClient;
use std::io::{Cursor, Seek, SeekFrom, Write};
use storage::types::PersistentStatePartId;
use ton_block::{fail, BlockIdExt, BocReader, Result};

#[allow(clippy::too_many_arguments)]
pub async fn download_pss_part<T: Write + Seek>(
    id: &PersistentStatePartId,
    master_id: &BlockIdExt,
    dest: &mut T,
    overlay: &FullNodeOverlayClient,
    mut attempts: Option<usize>,
    check_stop: &(dyn Fn() -> Result<()> + Sync + Send),
) -> Result<(usize, usize)> {
    if id.block_id().seq_no == 0 {
        fail!("download_pss_part: zerostate is not supported");
    }

    // Check
    let (peer, size) = loop {
        check_stop()?;
        if let Some(remained) = attempts.as_mut() {
            if *remained == 0 {
                fail!("download_pss_part: can't find peer for {}", id)
            }
            *remained -= 1;
        }
        match overlay.check_persistent_state(id, master_id).await {
            Err(e) => log::warn!("download_pss_part prepare {}: {}", id, e),
            Ok(None) => log::warn!("download_pss_part {} not found!", id),
            Ok(Some(p)) => break p,
        }
        futures_timer::Delay::new(std::time::Duration::from_millis(100)).await;
    };

    // Download

    let now = std::time::Instant::now();

    let max_size = 1 << 20; // part max size
    let mut offset = dest.seek(SeekFrom::End(0))? as usize;
    let mut peer_attempt = 0;
    let mut part_attempt = 0;
    let mut errors = 0;
    let mut part = 0;
    let mut cells_count = 0;

    log::info!(
        "download_pss_part: starting from offset {}, id: {}, master_id: {}",
        offset,
        id,
        master_id
    );

    loop {
        check_stop()?;
        let result = overlay
            .download_persistent_state_part(id, master_id, offset, max_size, &peer, peer_attempt)
            .await;
        match result {
            Ok(next_bytes) => {
                if offset == 0 {
                    let (boc_header, _) =
                        BocReader::new().read_header(&mut Cursor::new(&next_bytes))?;
                    cells_count = boc_header.cells_count;
                    if cells_count == 0 {
                        fail!("download_pss_part: got header with cells_count == 0");
                    }
                }
                part_attempt = 0;
                let len = next_bytes.len();
                dest.write(&next_bytes)?;
                if part % 10 == 0 {
                    if size > 0 {
                        log::info!(
                            "download_pss_part {}: downloaded: {}% ({}/{})",
                            id,
                            (offset * 100) / size,
                            offset,
                            size
                        );
                    } else {
                        log::info!("download_pss_part {}: downloaded: {}", id, offset);
                    };
                }
                part += 1;
                if len < max_size {
                    offset += len;
                    log::info!(
                        "download_pss_part {}: DONE in {:#?}, total length is {}",
                        id,
                        now.elapsed(),
                        offset
                    );
                    break;
                }
                offset += max_size;
            }
            Err(e) => {
                errors += 1;
                part_attempt += 1;
                peer_attempt += 1;
                log::error!(
                    "download_pss_part {}: {}, attempt: {}, total errors: {}",
                    id,
                    e,
                    part_attempt,
                    errors
                );
                if part_attempt > 30 {
                    fail!("Error download_pss_part after {} attempts: {}", part_attempt, e)
                }
                futures_timer::Delay::new(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    Ok((offset, cells_count))
}
