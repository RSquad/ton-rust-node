/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

/// A participant's stake and max_factor for election emulation.
#[derive(Clone)]
pub struct ParticipantStake {
    /// Stake in nanotons.
    pub stake: u64,
    /// Per-participant max_factor (multiplied by 65536).
    pub max_factor: u32,
}

/// Election context for emulating TON elector's validator selection algorithm.
pub struct ElectionContext {
    /// Existing participants (NOT including our own stake).
    pub participants: Vec<ParticipantStake>,
    /// Maximum number of validators (from ConfigParam16).
    pub max_validators: u16,
    /// Minimum number of validators (from ConfigParam16).
    pub min_validators: u16,
    /// Global max_factor from ConfigParam17 (multiplied by 65536).
    pub global_max_factor: u32,
    /// Minimum stake from ConfigParam17. A validator set is only valid if the weakest
    /// validator's stake >= min_stake.
    pub min_stake: u64,
    /// Maximum stake from ConfigParam17. Each participant's stake is capped at this value.
    pub max_stake: u64,
}

/// Result of election emulation.
pub struct EmulationResult {
    /// The effective minimum stake to enter the validator set.
    pub effective_min_stake: u64,
    /// Number of validators that would be elected.
    pub elected_count: u16,
}

/// Sorted participant for internal use (descending by stake).
#[derive(Clone)]
struct SortedEntry {
    stake: u64,
    /// Effective max_factor = min(participant_max_factor, global_max_stake_factor).
    max_factor: u32,
}

/// Emulates the TON elector's `try_elect` algorithm to determine
/// the effective minimum stake to enter the validator set.
///
/// `our_stake` is the hypothetical stake we would submit.
/// `our_max_factor` is our participant max_factor (raw u32).
/// Returns `None` if fewer than `min_validators` participants exist (including us).
///
/// Reference: elector-code.fc `try_elect` + `compute_total_stake`.
pub fn emulate_election(
    ctx: &ElectionContext,
    our_stake: u64,
    our_max_factor: u32,
) -> Option<EmulationResult> {
    let global_factor = ctx.global_max_factor;

    // Build sorted list (descending by stake), capping each stake at max_stake.
    // Apply min(participant_max_factor, global_max_stake_factor) per the elector logic.
    let mut entries: Vec<SortedEntry> = ctx
        .participants
        .iter()
        .map(|p| SortedEntry {
            stake: p.stake.min(ctx.max_stake),
            max_factor: p.max_factor.min(global_factor),
        })
        .collect();

    // Add our entry (skip if our_stake is 0 to avoid a phantom participant).
    if our_stake > 0 {
        entries.push(SortedEntry {
            stake: our_stake.min(ctx.max_stake),
            max_factor: our_max_factor.min(global_factor),
        });
    }

    if entries.len() < ctx.min_validators as usize {
        return None;
    }

    entries.sort_unstable_by(|a, b| b.stake.cmp(&a.stake)); // descending by stake

    let max_n = std::cmp::min(ctx.max_validators as usize, entries.len());
    let min_n = ctx.min_validators as usize;

    let mut best_total_effective = 0u128;
    let mut best_n = 0usize;

    for n in min_n..=max_n {
        // m_stake = stake of the weakest validator in the top-n set
        let m_stake = entries[n - 1].stake;

        if m_stake < ctx.min_stake {
            // No need to check further, as the weakest validator is below the minimum stake
            break;
        }

        // compute_total_stake: for each of top-n participants,
        // effective = min(stake_i, (max_factor_i * m_stake) >> 16)
        let total_effective = entries[..n]
            .iter()
            .map(|e| {
                let cap = ((e.max_factor as u128) * (m_stake as u128)) >> 16;
                std::cmp::min(e.stake as u128, cap)
            })
            .sum();

        if total_effective > best_total_effective {
            best_total_effective = total_effective;
            best_n = n;
        }
    }

    if best_n == 0 {
        return None;
    }

    let effective_min_stake = entries[best_n - 1].stake;

    Some(EmulationResult { effective_min_stake, elected_count: best_n as u16 })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NANO: u64 = 1_000_000_000;
    const FACTOR_3X: u32 = 3 * 65536;
    const MAX_STAKE: u64 = u64::MAX;

    fn simple_ctx(stakes: Vec<u64>, max_validators: u16, min_validators: u16) -> ElectionContext {
        ElectionContext {
            participants: stakes
                .into_iter()
                .map(|s| ParticipantStake { stake: s, max_factor: FACTOR_3X })
                .collect(),
            max_validators,
            min_validators,
            global_max_factor: FACTOR_3X,
            min_stake: 0,
            max_stake: MAX_STAKE,
        }
    }

    #[test]
    fn test_single_participant_is_elected() {
        let ctx = simple_ctx(vec![], 100, 1);
        let result = emulate_election(&ctx, 1_000_000, FACTOR_3X).unwrap();
        assert_eq!(result.effective_min_stake, 1_000_000);
        assert_eq!(result.elected_count, 1);
    }

    #[test]
    fn test_fewer_than_min_validators_returns_none() {
        let ctx = simple_ctx(vec![], 100, 5);
        assert!(emulate_election(&ctx, 1_000_000, FACTOR_3X).is_none());
    }

    #[test]
    fn test_kiln_scenario_half_below_effective_min() {
        // 400 participants with ~700k TON each, max_validators=400 (set is FULL)
        // Half (650k) is below the weakest validator, can't displace anyone
        let stakes: Vec<u64> = (0..400).map(|i| (700_000 + i * 100) * NANO).collect();
        let ctx = ElectionContext {
            participants: stakes
                .into_iter()
                .map(|s| ParticipantStake { stake: s, max_factor: FACTOR_3X })
                .collect(),
            max_validators: 400,
            min_validators: 13,
            global_max_factor: FACTOR_3X,
            min_stake: 0,
            max_stake: MAX_STAKE,
        };

        let half = 650_000 * NANO;
        let result = emulate_election(&ctx, half, FACTOR_3X).unwrap();
        assert!(
            result.effective_min_stake > half,
            "effective_min={} should be above half={}",
            result.effective_min_stake,
            half
        );
        assert_eq!(result.elected_count, 400);
    }

    #[test]
    fn test_kiln_scenario_full_stake_works() {
        // Same setup: 400 validators ~700k (set full), we stake full 1.3M
        let stakes: Vec<u64> = (0..400).map(|i| (700_000 + i * 100) * NANO).collect();
        let ctx = ElectionContext {
            participants: stakes
                .into_iter()
                .map(|s| ParticipantStake { stake: s, max_factor: FACTOR_3X })
                .collect(),
            max_validators: 400,
            min_validators: 13,
            global_max_factor: FACTOR_3X,
            min_stake: 0,
            max_stake: MAX_STAKE,
        };

        let full = 1_300_000 * NANO;
        let result = emulate_election(&ctx, full, FACTOR_3X).unwrap();
        assert!(result.effective_min_stake <= full);
        assert_eq!(result.elected_count, 400);
    }

    #[test]
    fn test_effective_min_with_unfilled_set() {
        // 100 participants with 700k each, max_validators=400 (set NOT full)
        // With factor 3.0, effective min ≈ 700k / 3 ≈ 233k
        let ctx = simple_ctx(vec![700_000 * NANO; 100], 400, 13);

        // 234k should be enough to join the set (room for more validators)
        let our_stake = 234_000 * NANO;
        let result = emulate_election(&ctx, our_stake, FACTOR_3X).unwrap();
        assert!(
            result.effective_min_stake <= our_stake,
            "234k should be sufficient: effective_min={}",
            result.effective_min_stake as f64 / NANO as f64
        );
        assert_eq!(result.elected_count, 101);

        // 230k should NOT be enough (below 700k/3 ≈ 233k threshold)
        let too_small = 230_000 * NANO;
        let result = emulate_election(&ctx, too_small, FACTOR_3X).unwrap();
        assert_eq!(
            result.elected_count, 100,
            "230k should be excluded: elected_count should be 100"
        );
    }

    #[test]
    fn test_split_works_when_half_above_effective_min() {
        // 50 participants with ~300k TON each, max_validators=400 (set NOT full)
        // Half (650k) should be well above effective min (~300k/3 = 100k)
        let ctx = simple_ctx(vec![300_000 * NANO; 50], 400, 13);

        let half = 650_000 * NANO;
        let result = emulate_election(&ctx, half, FACTOR_3X).unwrap();
        assert!(result.effective_min_stake <= half);
        assert_eq!(result.elected_count, 51);
    }

    #[test]
    fn test_factor_one_clamps_effective_stakes() {
        // With factor 1.0, effective = min(stake, m_stake * 1.0) = m_stake for all above it
        // Sorted desc: [1000, 800, 500, 200]
        // n=2: m_stake=800, total = 800+800 = 1600
        // n=3: m_stake=500, total = 500+500+500 = 1500
        // n=4: m_stake=200, total = 200*4 = 800
        // n=2 wins → only top 2 elected
        let ctx = ElectionContext {
            participants: vec![1000, 500, 200]
                .into_iter()
                .map(|s| ParticipantStake { stake: s, max_factor: 65536 })
                .collect(),
            max_validators: 10,
            min_validators: 1,
            global_max_factor: 65536,
            min_stake: 0,
            max_stake: MAX_STAKE,
        };

        let result = emulate_election(&ctx, 800, 65536).unwrap();
        assert_eq!(result.elected_count, 2);
        assert_eq!(result.effective_min_stake, 800);
    }

    #[test]
    fn test_equal_stakes() {
        let ctx = simple_ctx(vec![500_000 * NANO; 100], 400, 13);

        let result = emulate_election(&ctx, 500_000 * NANO, FACTOR_3X).unwrap();
        assert_eq!(result.effective_min_stake, 500_000 * NANO);
        assert_eq!(result.elected_count, 101);
    }

    #[test]
    fn test_max_validators_limit() {
        // 200 participants but max_validators = 50
        let stakes: Vec<u64> = (0..200).map(|i| (1_000_000 - i * 1000) * NANO).collect();
        let ctx = ElectionContext {
            participants: stakes
                .into_iter()
                .map(|s| ParticipantStake { stake: s, max_factor: FACTOR_3X })
                .collect(),
            max_validators: 50,
            min_validators: 13,
            global_max_factor: FACTOR_3X,
            min_stake: 0,
            max_stake: MAX_STAKE,
        };

        let result = emulate_election(&ctx, 900_000 * NANO, FACTOR_3X).unwrap();
        assert!(result.elected_count <= 50);
    }

    #[test]
    fn test_per_participant_max_factor() {
        // Two participants: one with factor 1.0, one with factor 3.0
        // With m_stake = 100, participant with factor 1.0 gets capped at 100,
        // participant with factor 3.0 gets capped at 300
        let ctx = ElectionContext {
            participants: vec![
                ParticipantStake { stake: 500, max_factor: 65536 }, // factor 1.0
                ParticipantStake { stake: 500, max_factor: 3 * 65536 }, // factor 3.0
            ],
            max_validators: 10,
            min_validators: 1,
            global_max_factor: 3 * 65536,
            min_stake: 0,
            max_stake: MAX_STAKE,
        };

        let result = emulate_election(&ctx, 100, 3 * 65536).unwrap();
        // m_stake = 100 (our stake, the weakest)
        // Participant 1 (500, factor 1.0): effective = min(500, 100 * 1.0) = 100
        // Participant 2 (500, factor 3.0): effective = min(500, 100 * 3.0) = 300
        // Us (100, factor 3.0): effective = min(100, 100 * 3.0) = 100
        // Total with n=3: 100 + 300 + 100 = 500
        // Without us (n=2): m_stake = 500, effective per each = 500, total = 1000
        // 1000 > 500, so n=2 is better → we're excluded
        assert_eq!(result.elected_count, 2);
    }

    #[test]
    fn test_min_stake_threshold() {
        // ConfigParam17 min_stake = 100. Participants below this can't form a valid set.
        let ctx = ElectionContext {
            participants: vec![ParticipantStake { stake: 50, max_factor: FACTOR_3X }],
            max_validators: 10,
            min_validators: 1,
            global_max_factor: FACTOR_3X,
            min_stake: 100,
            max_stake: MAX_STAKE,
        };

        // Both participants below min_stake → no valid election
        assert!(emulate_election(&ctx, 50, FACTOR_3X).is_none());

        // One participant above min_stake
        let result = emulate_election(&ctx, 150, FACTOR_3X).unwrap();
        // n=1 with our 150: m_stake=150 >= 100 ✓, but only if we're the weakest in top-1
        // Sorted: [150, 50]. n=1: m_stake=150, total=150. n=2: m_stake=50 < 100, skip.
        assert_eq!(result.elected_count, 1);
        assert_eq!(result.effective_min_stake, 150);
    }
}
