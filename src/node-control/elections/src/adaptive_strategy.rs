/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::election_emulator::{self, ElectionContext};
use common::ton_utils::nanotons_to_tons_f64;
use contracts::ElectionsInfo;
use ton_block::config_params::{ConfigParam16, ConfigParam17};

/// AdaptiveSplit50 wait logic: check whether enough time has passed and enough
/// participants have joined before proceeding with stake calculation.
///
/// Returns `true` if staking should proceed, `false` if we should defer (return 0).
pub(crate) fn is_adaptive_split50_ready(
    node_id: &str,
    stake_accepted: bool,
    elections_info: &ElectionsInfo,
    cfg15_start_before: u32,
    cfg15_end_before: u32,
    cfg16: &ConfigParam16,
    adaptive_sleep_pct: f64,
    adaptive_waiting_pct: f64,
) -> bool {
    if stake_accepted {
        return true;
    }

    let min_validators = cfg16.min_validators.as_u16() as usize;
    let participants_count = elections_info.participants.len();
    let election_duration = cfg15_start_before.saturating_sub(cfg15_end_before) as u64;

    if election_duration == 0 {
        tracing::warn!(
            "node [{}] adaptive_split50: election_duration=0, skipping wait logic",
            node_id
        );
        return true;
    }

    let election_start = elections_info.elect_close.saturating_sub(election_duration);
    let sleep_deadline = election_start + (election_duration as f64 * adaptive_sleep_pct) as u64;
    let wait_deadline = election_start + (election_duration as f64 * adaptive_waiting_pct) as u64;
    let now = common::time_format::now();

    // Wait if sleep period hasn't passed yet
    if now < sleep_deadline {
        tracing::info!(
            "node [{}] adaptive_split50 - sleep period: now < sleep_deadline={}",
            node_id,
            common::time_format::format_ts(sleep_deadline)
        );
        return false;
    }

    // Wait if not enough participants and waiting period hasn't expired
    if participants_count < min_validators && now < wait_deadline {
        tracing::info!(
            "node [{}] adaptive_split50 - waiting for participants: ({}/{}), deadline={}",
            node_id,
            participants_count,
            min_validators,
            common::time_format::format_ts(wait_deadline)
        );
        return false;
    }

    true
}

/// Calculate stake for AdaptiveSplit50 policy.
///
/// Determines min_eff_stake from current emulation and/or past elections,
/// then decides whether to split funds (stake half) or stake min_eff_stake.
///
/// Note: On subsequent ticks, tops up if min_eff_stake grew above current_stake.
/// If the remainder for the next round would be below min_eff_stake, stakes everything.
pub(crate) fn calc_adaptive_stake(
    node_id: &str,
    total_balance: u64,
    free_balance: u64,
    current_stake: u64,
    our_max_factor: u32,
    elections_info: &ElectionsInfo,
    cfg16: &ConfigParam16,
    cfg17: &ConfigParam17,
    prev_min_eff_stake: Option<u64>,
) -> anyhow::Result<u64> {
    let min_validators = cfg16.min_validators.as_u16();
    let max_validators = cfg16.max_validators.as_u16();
    let max_stake_factor = cfg17.max_stake_factor;
    let cfg17_min_stake = cfg17.min_stake.as_u64().unwrap_or(0);
    let cfg17_max_stake = cfg17.max_stake.as_u64().unwrap_or(u64::MAX);
    let half = total_balance / 2;

    // Compute curr_min_eff_stake from current participants.
    tracing::info!(
        "node [{}] adaptive_split50: emulate elections on {} participants",
        node_id,
        elections_info.participants.len()
    );
    let participants = elections_info
        .participants
        .iter()
        .map(|p| election_emulator::ParticipantStake { stake: p.stake, max_factor: p.max_factor })
        .collect();

    let ctx = ElectionContext {
        participants,
        max_validators,
        min_validators,
        global_max_factor: max_stake_factor,
        min_stake: cfg17_min_stake,
        max_stake: cfg17_max_stake,
    };

    let curr_min_eff = election_emulator::compute_min_effective_stake(&ctx, our_max_factor);

    // Choose the smallest min_eff_stake from curr and prev.
    let min_eff_stake = match (curr_min_eff, prev_min_eff_stake) {
        (Some(curr), Some(prev)) => {
            tracing::info!(
                "node [{}] adaptive_split50: curr_min_eff={} TON, prev_min_eff={} TON, using min={} TON",
                node_id,
                nanotons_to_tons_f64(curr),
                nanotons_to_tons_f64(prev),
                nanotons_to_tons_f64(curr.min(prev))
            );
            curr.min(prev)
        }
        (Some(curr), None) => {
            tracing::info!(
                "node [{}] adaptive_split50: curr_min_eff={} TON (no past elections data)",
                node_id,
                nanotons_to_tons_f64(curr)
            );
            curr
        }
        (None, Some(prev)) => {
            tracing::info!(
                "node [{}] adaptive_split50: prev_min_eff={} TON (not enough current participants < {})",
                node_id,
                nanotons_to_tons_f64(prev),
                min_validators
            );
            prev
        }
        (None, None) => {
            anyhow::bail!(
                "node [{}] adaptive_split50: cannot determine min effective stake \
                 (not enough participants and no past elections)",
                node_id
            );
        }
    };

    // If we already have enough stake, no need to top-up.
    if current_stake >= min_eff_stake {
        tracing::debug!(
            "node [{}] adaptive_split50: stake={} TON >= min_eff={} TON, no top-up needed",
            node_id,
            nanotons_to_tons_f64(current_stake),
            nanotons_to_tons_f64(min_eff_stake)
        );
        return Ok(0);
    }

    // Insufficient funds guard — if the pool doesn't have enough free
    // funds to cover min_eff_stake, skip the election entirely.
    // On the initial submission (current_stake == 0) we need at least min_eff_stake
    // free; on top-ups we need at least the delta.
    let required = min_eff_stake.saturating_sub(current_stake);
    if free_balance < required {
        tracing::error!(
            "node [{}] adaptive_split50: insufficient funds free_balance={} TON < required={} TON (min_eff={} TON), skipping election",
            node_id,
            nanotons_to_tons_f64(free_balance),
            nanotons_to_tons_f64(required),
            nanotons_to_tons_f64(min_eff_stake),
        );
        return Ok(0);
    }

    // Decide between staking half or min_eff_stake.
    if half >= min_eff_stake {
        // half is enough — stake half.
        let stake = half.saturating_sub(current_stake);
        tracing::info!(
            "node [{}] adaptive_split50 - stake half: current_stake={} TON, left_to_stake={} TON, half={} TON >= min_eff={} TON",
            node_id,
            nanotons_to_tons_f64(current_stake),
            nanotons_to_tons_f64(stake),
            nanotons_to_tons_f64(half),
            nanotons_to_tons_f64(min_eff_stake),
        );
        if stake > free_balance {
            // Not enough free funds to stake half. Skip and let the operator top up.
            tracing::error!(
                "node [{}] adaptive_split50 - insufficient free balance: need {} TON to stake half, \
                 but only {} TON available. Consider topping up the pool.",
                node_id,
                nanotons_to_tons_f64(stake),
                nanotons_to_tons_f64(free_balance),
            );
            return Ok(0);
        }
        Ok(stake)
    } else {
        // half < min_eff — splitting is not viable.
        // Since half < min_eff, it follows that total < 2 * min_eff,
        // so the remainder after staking min_eff would also be < min_eff.
        // The next round won't have enough funds anyway — stake everything.
        tracing::info!(
            "node [{}] adaptive_split50 - stake all: half={} TON < min_eff={} TON, staking all free_balance={} TON",
            node_id,
            nanotons_to_tons_f64(half),
            nanotons_to_tons_f64(min_eff_stake),
            nanotons_to_tons_f64(free_balance),
        );
        Ok(free_balance)
    }
}
