/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use super::election_emulator::{self, ElectionContext, ParticipantStake};
use common::ton_utils::nanotons_to_tons_f64;
use contracts::ElectionsInfo;
use ton_block::config_params::{ConfigParam16, ConfigParam17};

/// AdaptiveSplit50 wait logic: check whether enough time has passed and enough
/// participants have joined before proceeding with stake calculation.
///
/// Why AdaptiveSplit50 defers staking for this tick (`calc_stake` returns 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdaptiveDeferReason {
    SleepPeriod,
    WaitingForParticipants,
}

/// Why AdaptiveSplit50 returns zero stake after the defer window has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdaptiveStakeZero {
    /// Stake already meets min effective — no top-up this tick (not an error).
    NoTopUpNeeded,
    /// Free pool balance is below the required delta to min effective stake.
    InsufficientFree { required: u64, available: u64 },
}

/// Outcome of [`calc_adaptive_stake`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdaptiveStakeResult {
    Stake(u64),
    Zero(AdaptiveStakeZero),
}

/// Returns `true` if staking should proceed, `false` if we should defer (return 0).
pub(crate) fn is_adaptive_split50_ready(
    node_id: &str,
    elections_info: &ElectionsInfo,
    cfg15_start_before: u32,
    cfg15_end_before: u32,
    cfg16: &ConfigParam16,
    sleep_pct: f64,
    waiting_pct: f64,
) -> bool {
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
    let sleep_deadline = election_start + (election_duration as f64 * sleep_pct) as u64;
    let wait_deadline = election_start + (election_duration as f64 * waiting_pct) as u64;
    let now = common::time_format::now();

    // Wait if sleep period hasn't passed yet
    if now < sleep_deadline {
        tracing::info!(
            "node [{}] adaptive_split50: sleep period, now < sleep_deadline={}",
            node_id,
            common::time_format::format_ts(sleep_deadline)
        );
        return false;
    }

    // Wait if not enough participants and waiting period hasn't expired
    if participants_count < min_validators && now < wait_deadline {
        tracing::info!(
            "node [{}] adaptive_split50: waiting for participants ({}/{}), deadline={}",
            node_id,
            participants_count,
            min_validators,
            common::time_format::format_ts(wait_deadline)
        );
        return false;
    }

    true
}

/// When [`is_adaptive_split50_ready`] would return `false`, reports which wait gate blocked.
pub(crate) fn adaptive_split50_defer_reason(
    elections_info: &ElectionsInfo,
    cfg15_start_before: u32,
    cfg15_end_before: u32,
    cfg16: &ConfigParam16,
    sleep_pct: f64,
    waiting_pct: f64,
) -> Option<AdaptiveDeferReason> {
    let min_validators = cfg16.min_validators.as_u16() as usize;
    let participants_count = elections_info.participants.len();
    let election_duration = cfg15_start_before.saturating_sub(cfg15_end_before) as u64;
    if election_duration == 0 {
        return None;
    }

    let election_start = elections_info.elect_close.saturating_sub(election_duration);
    let sleep_deadline = election_start + (election_duration as f64 * sleep_pct) as u64;
    let wait_deadline = election_start + (election_duration as f64 * waiting_pct) as u64;
    let now = common::time_format::now();

    if now < sleep_deadline {
        return Some(AdaptiveDeferReason::SleepPeriod);
    }
    if participants_count < min_validators && now < wait_deadline {
        return Some(AdaptiveDeferReason::WaitingForParticipants);
    }
    None
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
    stakes: Vec<ParticipantStake>,
    cfg16: &ConfigParam16,
    cfg17: &ConfigParam17,
    prev_min_stake: Option<u64>,
) -> anyhow::Result<AdaptiveStakeResult> {
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
        stakes.len()
    );

    let ctx = ElectionContext {
        participants: stakes,
        max_validators,
        min_validators,
        global_max_factor: max_stake_factor,
        min_stake: cfg17_min_stake,
        max_stake: cfg17_max_stake,
    };

    let curr_min_eff = election_emulator::compute_min_effective_stake(&ctx, our_max_factor);

    // Calculate estimated curr effective stake. If failed to calculate, use previous elections min_stake.
    let min_eff_stake = match (curr_min_eff, prev_min_stake) {
        (Some(curr), Some(prev)) => {
            tracing::info!(
                "node [{}] adaptive_split50: curr_min_eff={} TON, prev_min={} TON",
                node_id,
                nanotons_to_tons_f64(curr),
                nanotons_to_tons_f64(prev),
            );
            curr
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
                "node [{}] adaptive_split50: not enough current participants < {}, use prev_min={} TON",
                node_id,
                min_validators,
                nanotons_to_tons_f64(prev),
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
        return Ok(AdaptiveStakeResult::Zero(AdaptiveStakeZero::NoTopUpNeeded));
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
        return Ok(AdaptiveStakeResult::Zero(AdaptiveStakeZero::InsufficientFree {
            required,
            available: free_balance,
        }));
    }

    // Decide between staking half or min_eff_stake.
    if half >= min_eff_stake {
        // half is enough — stake half.
        let stake = half.saturating_sub(current_stake);
        tracing::info!(
            "node [{}] adaptive_split50: stake half, current_stake={} TON, left_to_stake={} TON, half={} TON >= min_eff={} TON",
            node_id,
            nanotons_to_tons_f64(current_stake),
            nanotons_to_tons_f64(stake),
            nanotons_to_tons_f64(half),
            nanotons_to_tons_f64(min_eff_stake),
        );
        if stake > free_balance {
            // Not enough free funds to stake half. Skip and let the operator top up.
            tracing::error!(
                "node [{}] adaptive_split50: insufficient free balance, need {} TON to stake half, \
                 but only {} TON available. Consider topping up the pool.",
                node_id,
                nanotons_to_tons_f64(stake),
                nanotons_to_tons_f64(free_balance),
            );
            return Ok(AdaptiveStakeResult::Zero(AdaptiveStakeZero::InsufficientFree {
                required: stake,
                available: free_balance,
            }));
        }
        Ok(AdaptiveStakeResult::Stake(stake))
    } else {
        // half < min_eff — splitting is not viable.
        // Since half < min_eff, it follows that total < 2 * min_eff,
        // so the remainder after staking min_eff would also be < min_eff.
        // The next round won't have enough funds anyway — stake everything.
        tracing::info!(
            "node [{}] adaptive_split50: stake all, half={} TON < min_eff={} TON, staking all free_balance={} TON",
            node_id,
            nanotons_to_tons_f64(half),
            nanotons_to_tons_f64(min_eff_stake),
            nanotons_to_tons_f64(free_balance),
        );
        Ok(AdaptiveStakeResult::Stake(free_balance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stake_amount(r: AdaptiveStakeResult) -> u64 {
        match r {
            AdaptiveStakeResult::Stake(s) => s,
            other => panic!("expected stake, got {other:?}"),
        }
    }

    use ton_block::{
        Coins, Number16,
        config_params::{ConfigParam16, ConfigParam17},
    };

    const NANO: u64 = 1_000_000_000;
    const FACTOR_3X: u32 = 3 * 65536;

    fn default_cfg16() -> ConfigParam16 {
        ConfigParam16 {
            max_validators: Number16::from(400u16),
            max_main_validators: Number16::from(100u16),
            min_validators: Number16::from(13u16),
        }
    }

    fn default_cfg17() -> ConfigParam17 {
        ConfigParam17 {
            min_stake: Coins::from(10_000_000_000_000u64), // 10,000 TON
            max_stake: Coins::from(10_000_000_000_000_000u64), // 10,000,000 TON
            min_total_stake: Coins::from(100_000_000_000_000u64), // 100,000 TON
            max_stake_factor: 3 * 65536,                   // 3x
        }
    }

    /// Build `n` participant stakes each with `stake_per` nanotons.
    fn participant_stakes(n: usize, stake_per: u64) -> Vec<ParticipantStake> {
        vec![ParticipantStake { stake: stake_per, max_factor: FACTOR_3X }; n]
    }

    // ---- half >= min_eff → stake half ----

    #[test]
    fn test_adaptive_stake_half_when_above_min_eff() {
        // 50 participants with 300k TON each, max_validators=400 (set NOT full).
        // effective_min is ~100k TON (300k / factor 3).
        // total_balance = 1_300_000 TON, half = 650_000 TON >> effective_min.
        // Expected: stake half = 650_000 TON.
        let total_balance = 1_300_000 * NANO;
        let free_balance = total_balance; // no frozen, no current
        let current_stake = 0;

        let stakes = participant_stakes(50, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            None,
        )
        .unwrap();

        let half = total_balance / 2;
        assert_eq!(stake_amount(result), half, "should stake half");
    }

    // ---- half < min_eff → stake all ----

    #[test]
    fn test_adaptive_stake_all_when_half_below_min_eff() {
        // 400 participants with ~700k TON each (set FULL, max_validators=400).
        // effective_min ~700k TON. Our total = 1_300_000 TON, half = 650_000 < 700_000.
        // Expected: stake all free_balance.
        let total_balance = 1_300_000 * NANO;
        let free_balance = total_balance;
        let current_stake = 0;

        let stakes: Vec<ParticipantStake> = (0..400)
            .map(|i| ParticipantStake {
                stake: (700_000 + i as u64 * 100) * NANO,
                max_factor: FACTOR_3X,
            })
            .collect();

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes,
            &default_cfg16(),
            &default_cfg17(),
            None,
        )
        .unwrap();

        assert_eq!(
            stake_amount(result),
            free_balance,
            "should stake all free_balance when half < min_eff"
        );
    }

    // ---- current_stake >= min_eff → no top-up ----

    #[test]
    fn test_adaptive_no_topup_when_stake_sufficient() {
        // 50 participants with 300k TON. effective_min ~100k.
        // current_stake = 650_000 TON >> effective_min.
        // Expected: return 0 (no top-up).
        let total_balance = 1_300_000 * NANO;
        let free_balance = 0; // all staked or frozen
        let current_stake = 650_000 * NANO;

        let stakes = participant_stakes(50, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            None,
        )
        .unwrap();

        assert_eq!(result, AdaptiveStakeResult::Zero(AdaptiveStakeZero::NoTopUpNeeded));
    }

    // ---- insufficient funds guard ----

    #[test]
    fn test_adaptive_skip_when_insufficient_funds() {
        // 50 participants with 300k TON. effective_min ~100k.
        // total_balance is high (due to frozen), but free_balance < required.
        // Expected: return 0 (skip).
        let frozen = 900_000 * NANO;
        let free_balance = 50_000 * NANO; // less than effective_min (~100k)
        let current_stake = 0;
        let total_balance = frozen + free_balance + current_stake;

        let stakes = participant_stakes(50, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            None,
        )
        .unwrap();

        assert!(matches!(
            result,
            AdaptiveStakeResult::Zero(AdaptiveStakeZero::InsufficientFree { .. })
        ));
    }

    // ---- cap to free_balance when half > free_balance ----

    #[test]
    fn test_adaptive_skip_when_half_exceeds_free_balance() {
        // Use few participants (< min_validators) so emulation returns None.
        // prev_min_eff = 50k controls the effective_min.
        // total = 1_300_000, half = 650_000 > 50k → half branch.
        // free_balance = 200_000 < half(650k) → skip (not enough to stake half).
        // free_balance (200k) > prev_min_eff (50k) → passes insufficient funds guard.
        let frozen = 1_100_000 * NANO;
        let free_balance = 200_000 * NANO;
        let current_stake = 0;
        let total_balance = frozen + free_balance + current_stake;
        let prev_min_eff = Some(50_000 * NANO);

        let stakes = participant_stakes(5, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            prev_min_eff,
        )
        .unwrap();

        assert!(matches!(
            result,
            AdaptiveStakeResult::Zero(AdaptiveStakeZero::InsufficientFree { .. })
        ));
    }

    // ---- curr vs prev selection ----

    #[test]
    fn test_adaptive_uses_curr_when_both_available() {
        // 50 participants with 300k TON. curr_min_eff ~99.4k TON (≈ 300k / factor 3).
        // prev_min_eff = 80k < curr → should use curr (~99.4k), not prev.
        // total = 200k, half = 100k >= curr (~99.4k) → stake half = 100k.
        let total_balance = 200_000 * NANO;
        let free_balance = total_balance;
        let current_stake = 0;
        let prev_min_eff = Some(80_000 * NANO);

        let stakes = participant_stakes(50, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            prev_min_eff,
        )
        .unwrap();

        let half = total_balance / 2;
        assert_eq!(stake_amount(result), half, "should use curr_min_eff and stake half");
    }

    #[test]
    fn test_adaptive_ignores_prev_when_curr_available() {
        // 50 participants with 300k TON. curr_min_eff ~99.4k TON.
        // prev_min_eff = 80k < curr. total = 190k, half = 95k.
        // half (95k) is between prev (80k) and curr (~99.4k):
        //   - old behavior (min of curr and prev): min_eff = 80k → half ≥ 80k → stake half (95k)
        //   - new behavior (use curr): min_eff = curr (~99.4k) → half < curr → stake all (190k)
        let total_balance = 190_000 * NANO;
        let free_balance = total_balance;
        let current_stake = 0;
        let prev_min_eff = Some(80_000 * NANO);

        let stakes = participant_stakes(50, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            prev_min_eff,
        )
        .unwrap();

        assert_eq!(
            stake_amount(result),
            free_balance,
            "should use curr_min_eff (not prev) and stake all when half < curr"
        );
    }

    // ---- prev only (curr = None, fewer than min_validators) ----

    #[test]
    fn test_adaptive_fallback_to_prev_when_not_enough_participants() {
        // Only 5 participants (< min_validators=13) → emulation returns None.
        // prev_min_eff = 50k.
        // total = 200k, half = 100k >= 50k → stake half.
        let total_balance = 200_000 * NANO;
        let free_balance = total_balance;
        let current_stake = 0;
        let prev_min_eff = Some(50_000 * NANO);

        let stakes = participant_stakes(5, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            prev_min_eff,
        )
        .unwrap();

        let half = total_balance / 2;
        assert_eq!(
            stake_amount(result),
            half,
            "should fallback to prev_min_eff when not enough participants"
        );
    }

    // ---- both None → error ----

    #[test]
    fn test_adaptive_error_when_no_min_eff_available() {
        // Fewer than min_validators (5 < 13) AND no prev_min_eff → error.
        let total_balance = 200_000 * NANO;
        let free_balance = total_balance;
        let current_stake = 0;

        let stakes = participant_stakes(5, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            None,
        );

        assert!(result.is_err(), "should fail when both curr and prev min_eff are unavailable");
    }

    // ---- top-up: half branch, partial top-up ----

    #[test]
    fn test_adaptive_topup_to_half() {
        // Use few participants so emulation returns None; prev_min_eff controls effective.
        // prev_min_eff = 600k. current_stake = 500k < 600k → need top-up.
        // total = 1_300_000, half = 650_000 > 600k → half branch.
        // stake = half - current = 650k - 500k = 150k.
        let total_balance = 1_300_000 * NANO;
        let free_balance = 200_000 * NANO;
        let current_stake = 500_000 * NANO;
        let prev_min_eff = Some(600_000 * NANO);

        // current_stake > 0 → emulation uses our_stake = 0 (already in list).
        // With < min_validators participants, emulation returns None → uses prev_min_eff.
        let stakes = participant_stakes(5, 300_000 * NANO);

        let result = calc_adaptive_stake(
            "node-1",
            total_balance,
            free_balance,
            current_stake,
            FACTOR_3X,
            stakes.clone(),
            &default_cfg16(),
            &default_cfg17(),
            prev_min_eff,
        )
        .unwrap();

        let expected = total_balance / 2 - current_stake;
        assert_eq!(stake_amount(result), expected, "should top up to half");
    }
}
