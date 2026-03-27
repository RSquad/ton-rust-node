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

/// Computes the minimum effective stake required to enter the validator set using a two-phase search.
///
/// **Phase 1 (coarse):** starting from an initial estimate (`max_participant_stake /
/// effective_max_factor`), step down by `big_step = ctx.min_stake / 10` while included
/// in the elected set. If the initial estimate is too low, step up by `big_step` first.
///
/// **Phase 2 (fine):** on the first exclusion after an inclusion, compute
/// `fine_step = (last_included_stake - current) / 10` (at least 1) and step up by
/// `fine_step` until included again. Returns the `effective_min_stake` from that election.
///
/// Returns `None` if there are fewer than `min_validators` existing participants,
/// if `min_stake` is zero, or if no valid set can be formed.
///
/// # Example
///
/// ```text
/// participants (stake, max-factor) = [(1000k,3x), (1000k,2x), (800k,3x), (800k,3x), (600k,3x)]
/// min_validators = 5, min_stake = 100k, our_max_factor = 3x
/// big_step = 100k / 10 = 10k
///
/// 1) initial estimate = 1000k / 2.0 = 500k
/// 2) Phase 1 (step down by 10k): 500k(in) → 490k(in) → ... → 340k(in) → 330k(out)
/// 3) Phase 2: fine_step = max(1, (340k - 330k) / 10) = 1k
///    331k(out) → 332k(out) → 333k(out) → 334k(in, eff_min=334k)
/// 4) return Some(334k)
/// ```
pub fn compute_min_effective_stake(ctx: &ElectionContext, our_max_factor: u32) -> Option<u64> {
    // Not enough participants to estimate.
    if ctx.participants.len() < ctx.min_validators as usize {
        return None;
    }

    // Step size would be zero.
    if ctx.min_stake == 0 {
        return None;
    }

    let big_step = (ctx.min_stake / 10).max(1); // Phase 1 (coarse) step
    let mut current = initial_stake_estimate(ctx);
    let mut last_submitted = 0u64; // last stake where we were included
    let mut last_eff: Option<u64> = None; // effective_min_stake at last inclusion
    let mut fine_phase = false;
    let mut fine_step = 1u64; // Phase 2 step, set on phase transition

    loop {
        let Some(res) = emulate_election(ctx, current, our_max_factor) else {
            return last_eff; // no valid election possible, return last known good
        };

        if current >= res.effective_min_stake {
            // Included: record and keep stepping down.
            last_submitted = current;
            last_eff = Some(res.effective_min_stake);
            if fine_phase {
                return last_eff; // Phase 2 converged.
            }
            current = current.saturating_sub(big_step);
        } else if last_eff.is_some() {
            // Excluded after inclusion: switch to fine upward search (Phase 2).
            if !fine_phase {
                fine_step = (last_submitted.saturating_sub(current) / 10).max(1);
                fine_phase = true;
            }
            let next = current.saturating_add(fine_step);
            if next == current {
                return last_eff; // overflow guard
            }
            current = next;
        } else {
            // No success yet: step up coarsely.
            let next = current.saturating_add(big_step);
            if next == current {
                return None; // overflow guard
            }
            current = next;
        }
    }
}

/// Initial stake estimate: max participant stake divided by their smallest effective max_factor.
fn initial_stake_estimate(ctx: &ElectionContext) -> u64 {
    let max_s = ctx
        .participants
        .iter()
        .map(|p| p.stake.min(ctx.max_stake))
        .max()
        .unwrap_or(0);
    if max_s == 0 {
        return ctx.min_stake;
    }
    let min_factor = ctx
        .participants
        .iter()
        .filter(|p| p.stake.min(ctx.max_stake) == max_s)
        .map(|p| p.max_factor.min(ctx.global_max_factor))
        .min()
        .unwrap_or(ctx.global_max_factor);
    if min_factor == 0 { max_s } else { ((max_s as u128 * 65536) / min_factor as u128) as u64 }
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

    // ---- compute_min_effective_stake tests ----

    #[test]
    fn test_compute_min_effective_stake_comment_example() {
        // Scenario from the function comment.
        // participants: [(50,3x),(50,2x),(40,3x),(40,3x),(30,3x)], min_stake=5
        // big_step = max(1, 5/10) = 1
        // initial_estimate = 50 * 65536 / (2 * 65536) = 25
        //
        // Coarse phase (step-1 descent): at S=17, n=6 total=211 > n=5 total=210 → included.
        //   At S=16, n=5 wins (total=206<210), eff_min=30, excluded.
        // Fine phase: fine_step=max(1,(17-16)/10)=1. S=17 → included, eff_min=17.
        let ctx = ElectionContext {
            participants: vec![
                ParticipantStake { stake: 50, max_factor: 3 * 65536 },
                ParticipantStake { stake: 50, max_factor: 2 * 65536 },
                ParticipantStake { stake: 40, max_factor: 3 * 65536 },
                ParticipantStake { stake: 40, max_factor: 3 * 65536 },
                ParticipantStake { stake: 30, max_factor: 3 * 65536 },
            ],
            max_validators: 100,
            min_validators: 5,
            global_max_factor: 3 * 65536,
            min_stake: 5,
            max_stake: MAX_STAKE,
        };
        let result = compute_min_effective_stake(&ctx, 3 * 65536);
        assert_eq!(result, Some(17));
        // Self-consistency: emulate at the returned stake must include us.
        let r = emulate_election(&ctx, 17, 3 * 65536).unwrap();
        assert!(17 >= r.effective_min_stake);
    }

    #[test]
    fn test_compute_min_effective_stake_not_enough_participants() {
        // Fewer than min_validators existing participants → None (not enough data).
        let ctx = ElectionContext {
            participants: vec![
                ParticipantStake { stake: 1000, max_factor: FACTOR_3X },
                ParticipantStake { stake: 1000, max_factor: FACTOR_3X },
            ],
            max_validators: 10,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 10,
            max_stake: MAX_STAKE,
        };
        assert!(compute_min_effective_stake(&ctx, FACTOR_3X).is_none());

        // Exactly min_validators - 1 existing participants → also None.
        let ctx2 = ElectionContext {
            participants: vec![ParticipantStake { stake: 1000, max_factor: FACTOR_3X }; 4],
            max_validators: 10,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 10,
            max_stake: MAX_STAKE,
        };
        assert!(compute_min_effective_stake(&ctx2, FACTOR_3X).is_none());
    }

    #[test]
    fn test_compute_min_effective_stake_sole_validator() {
        // No existing participants → fewer than min_validators → None (not enough data).
        let ctx = ElectionContext {
            participants: vec![],
            max_validators: 10,
            min_validators: 1,
            global_max_factor: FACTOR_3X,
            min_stake: 100,
            max_stake: MAX_STAKE,
        };
        assert_eq!(compute_min_effective_stake(&ctx, FACTOR_3X), None);
    }

    #[test]
    fn test_compute_min_effective_stake_full_set() {
        // 10 participants at 1000 each, max_validators=10 (set exactly full).
        // To displace the weakest we must match the weakest stake (1000).
        // big_step = max(1, 100/10) = 10. initial ≈ 333.
        // Coarse up: 333→…→1003 (first inclusion, eff_min=1000), then down to 993 (excluded).
        // Fine phase: fine_step=max(1,(1003-993)/10)=1. 994…1000 → included, return 1000.
        let ctx = ElectionContext {
            participants: vec![ParticipantStake { stake: 1000, max_factor: FACTOR_3X }; 10],
            max_validators: 10,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 100,
            max_stake: MAX_STAKE,
        };
        assert_eq!(compute_min_effective_stake(&ctx, FACTOR_3X), Some(1000));
    }

    #[test]
    fn test_compute_min_effective_stake_equal_stakes_unfilled_set() {
        // 5 participants at 500 each, max_validators=100, min_stake=50.
        // big_step = max(1, 50/10) = 5.  initial = 500/3 = 166.
        // n=6 total = 16*S  (for S ≤ 166).  n=5 total = 2500.
        // 16S > 2500 iff S ≥ 157.  So min entry stake = 157.
        let ctx = ElectionContext {
            participants: vec![ParticipantStake { stake: 500, max_factor: FACTOR_3X }; 5],
            max_validators: 100,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 50,
            max_stake: MAX_STAKE,
        };
        assert_eq!(compute_min_effective_stake(&ctx, FACTOR_3X), Some(157));
        // Verify boundary: S=157 included, S=156 excluded.
        let r_in = emulate_election(&ctx, 157, FACTOR_3X).unwrap();
        assert!(157 >= r_in.effective_min_stake);
        let r_out = emulate_election(&ctx, 156, FACTOR_3X).unwrap();
        assert!(156 < r_out.effective_min_stake);
    }

    #[test]
    fn test_compute_min_effective_stake_real_mainnet_data() {
        // Top-75 stakes from mainnet validation cycle 1774538504.
        // All validators share max_factor = 3 * 65536 (196608).
        // Config17: min_stake = 100_000 TON, max_stake = unlimited, max_stake_factor = 3x.
        // Config16: max_validators = 1000, min_validators = 75.
        //
        // big_step = min_stake / 10 = 10_000 TON.
        // fine_step = big_step / 10 = 1_000 TON.
        // The returned value should be the minimum stake (± 1_000 TON) to enter the set.
        const STAKES: &[u64] = &[
            2060445595410000, 2060445595410000, 2060445595410000, 2060445595410000,
            2060445595410000, 2060445595410000, 2060445595410000, 2060445595410000,
            2060445595410000, 2043172198470000, 2043172198470000, 2043171198470000,
            2043171198470000, 2043167198470000, 2041919198470000, 2041919198470000,
            2041919198470000, 2041919198470000, 2041919198470000, 2041623198470000,
            2041623198470000, 2041623198470000, 2041623198470000, 2041621198470000,
            2041534198470000, 2041534198470000, 2041534198470000, 2041534198470000,
            2041534198470000, 2041534198470000, 2041534198470000, 2041534198470000,
            2041528198470000, 2040298198470000, 2040298198470000, 2040298198470000,
            2040298198470000, 2040298198470000, 2040298198470000, 2040298198470000,
            2040298198470000, 2040298198470000, 2040298198470000, 2040298198470000,
            2040298198470000, 2040298198470000, 2040298198470000, 2039455198470000,
            2039285198470000, 2035393202020000, 2033875202020000, 2017259202020000,
            2003210617967562, 2003204176979680, 2000435064702159, 1907320760429904,
            1907320760429904, 1907320760429723, 1907320760425920, 1905457988889466,
            1900457012649501, 1900418452888393, 1900418452886194, 1900418452885461,
            1900417444657521, 1890781198470000, 1800418623166831, 1769587198470000,
            1716359202020000, 1698999585994571, 1697132198470000, 1693715198470000,
            1693713198470000, 1693257198470000, 1604101270271938,
        ];
        let ctx = ElectionContext {
            participants: STAKES
                .iter()
                .map(|&s| ParticipantStake { stake: s, max_factor: 3 * 65536 })
                .collect(),
            max_validators: 1000,
            min_validators: 75,
            global_max_factor: 3 * 65536,
            min_stake: 100_000_000_000_000, // 100_000 TON
            max_stake: u64::MAX,
        };

        let result = compute_min_effective_stake(&ctx, 3 * 65536);
        assert!(result.is_some(), "expected a valid entry stake with 309 participants");
        let min_s = result.unwrap();

        // At min_s we must be included.
        let r_in = emulate_election(&ctx, min_s, 3 * 65536).unwrap();
        assert!(
            min_s >= r_in.effective_min_stake,
            "should be included at {}k TON (eff_min={}k TON)",
            min_s / NANO / 1000,
            r_in.effective_min_stake / NANO / 1000,
        );

        // fine_step = big_step / 10 = 10_000 TON / 10 = 1_000 TON.
        // By algorithm design, at min_s - 1_000 TON we must NOT be included.
        let fine_step = 1_000 * NANO;
        let r_out = emulate_election(&ctx, min_s - fine_step, 3 * 65536).unwrap();
        assert!(
            min_s - fine_step < r_out.effective_min_stake,
            "should be excluded at {}k TON (eff_min={}k TON)",
            (min_s - fine_step) / NANO / 1000,
            r_out.effective_min_stake / NANO / 1000,
        );
    }

    #[test]
    fn test_compute_min_effective_stake_initial_estimate_below_threshold() {
        // All participants have high stakes. Our initial estimate may be below the entry
        // threshold so the coarse phase must raise before it can lower.
        // 5 participants at 900, factor=3x, max_validators=10, min_stake=30, min_validators=5.
        // initial = 900/3 = 300.
        // n=5 total at m=900 = 900*5 = 4500.
        // n=6 total at m=S = 16S (for S ≤ 300). 16S > 4500 iff S > 281.25, i.e. S ≥ 282.
        // So initial=300 is already above the threshold → coarse phase descends immediately.
        let ctx = ElectionContext {
            participants: vec![ParticipantStake { stake: 900, max_factor: FACTOR_3X }; 5],
            max_validators: 10,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 30,
            max_stake: MAX_STAKE,
        };
        let result = compute_min_effective_stake(&ctx, FACTOR_3X);
        assert!(result.is_some());
        let min_s = result.unwrap();
        // Returned value must include us.
        let r = emulate_election(&ctx, min_s, FACTOR_3X).unwrap();
        assert!(min_s >= r.effective_min_stake);
        // One step below must exclude us.
        let r2 = emulate_election(&ctx, min_s - 1, FACTOR_3X).unwrap();
        assert!(min_s - 1 < r2.effective_min_stake);
    }

    #[test]
    fn test_compute_min_effective_stake_zero_min_stake() {
        // min_stake = 0 → big_step would be 0 → None early return.
        let ctx = ElectionContext {
            participants: vec![ParticipantStake { stake: 1000, max_factor: FACTOR_3X }; 5],
            max_validators: 100,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 0,
            max_stake: MAX_STAKE,
        };
        assert_eq!(compute_min_effective_stake(&ctx, FACTOR_3X), None);
    }

    #[test]
    fn test_compute_min_effective_stake_emulate_returns_none_mid_loop() {
        // Phase 1 steps current below min_stake → emulate_election returns None
        // → function returns last_eff from the previous successful iteration.
        //
        // 5 participants at 200, min_stake = 150, big_step = 15.
        // initial = 200/3 = 66 < min_stake=150 → excluded, step up by 15.
        // At some point we cross 150 and get included (emulate succeeds).
        // Then stepping down eventually goes below 150 → emulate returns None → return last_eff.
        let ctx = ElectionContext {
            participants: vec![ParticipantStake { stake: 200, max_factor: FACTOR_3X }; 5],
            max_validators: 100,
            min_validators: 5,
            global_max_factor: FACTOR_3X,
            min_stake: 150,
            max_stake: MAX_STAKE,
        };
        let result = compute_min_effective_stake(&ctx, FACTOR_3X);
        assert!(result.is_some());
        let min_s = result.unwrap();
        // Verify we're included at the returned stake.
        let r = emulate_election(&ctx, min_s, FACTOR_3X).unwrap();
        assert!(min_s >= r.effective_min_stake);
    }
}
