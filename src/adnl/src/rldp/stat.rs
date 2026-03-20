/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    adnl::{common::AtomicPair, telemetry::AverageTotal},
    rldp::{send::SendPacketV2, RldpNode},
};
use rand::{Rng, SeedableRng};
use std::{
    cmp::{max, Ordering as CmpOrdering},
    sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering},
    time::Instant,
};
use ton_block::{fail, Result};

struct AtomicFixedPoint(AtomicU64);

impl Default for AtomicFixedPoint {
    fn default() -> Self {
        Self(AtomicU64::new(0))
    }
}

impl AtomicFixedPoint {
    const MAX: f64 = u64::MAX as f64;
    const SCALE: u64 = 1000000;

    fn new(value: f64) -> Result<Self> {
        let ret = Self(AtomicU64::new(0));
        ret.store(value, None)?;
        Ok(ret)
    }

    fn load(&self) -> (f64, u64) {
        let AtomicFixedPoint(shared) = self;
        let value = shared.load(Ordering::Acquire);
        if value == u64::MAX {
            (f64::NAN, value)
        } else {
            (value as f64 / Self::SCALE as f64, value)
        }
    }

    fn store(&self, value: f64, old_value: Option<u64>) -> Result<bool> {
        let scaled = if value.is_nan() {
            u64::MAX
        } else {
            let scaled = value * Self::SCALE as f64;
            if scaled.is_infinite() || (scaled > Self::MAX) {
                fail!("Atomic fixed point overflow on value {}", value)
            }
            scaled as u64
        };
        let AtomicFixedPoint(shared) = self;
        let ret = if let Some(old_value) = old_value {
            shared.compare_exchange(old_value, scaled, Ordering::Release, Ordering::Relaxed).is_ok()
        } else {
            shared.store(scaled, Ordering::Release);
            true
        };
        Ok(ret)
    }
}

#[derive(Default)]
struct BandwidthStats {
    delivered_at_micros: AtomicU64,
    delivered_count: AtomicU32,
    paused_at_micros: AtomicU64,
    windowed_max: AtomicU32,
    windowed_max_stat: MinMaxStat,
}

impl BandwidthStats {
    const LOCK_BIT: u32 = 0x80000000;
    const STAT_DURATION_MS: u64 = 5000;

    fn new(initial_bandwidth: u32) -> Self {
        Self {
            windowed_max: AtomicU32::new(initial_bandwidth),
            windowed_max_stat: MinMaxStat::with_params(
                Self::STAT_DURATION_MS,
                std::cmp::Ordering::Greater,
            ),
            ..Default::default()
        }
    }

    fn on_send(&self, first_sent_at_micros: u64) -> PacketBandwidthInfo {
        loop {
            let delivered_count = self.delivered_count.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (delivered_count & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            let delivered_at_micros = self.delivered_at_micros.load(Ordering::Relaxed);
            let is_paused = self.paused_at_micros.load(Ordering::Relaxed) != 0;
            self.delivered_count
                .compare_exchange(
                    delivered_count | Self::LOCK_BIT,
                    delivered_count,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .ok();
            break PacketBandwidthInfo {
                delivered_at_micros,
                delivered_count,
                first_sent_at_micros,
                is_paused,
            };
        }
    }

    fn update(
        &self,
        max_packet: SendPacketV2,
        new_received: u32,
        timestamp_micros: u64,
    ) -> Result<()> {
        loop {
            let delivered_count = self.delivered_count.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (delivered_count & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            self.delivered_at_micros.store(timestamp_micros, Ordering::Relaxed);
            let max_delivered_at_micros = max_packet.bandwidth_info.delivered_at_micros;
            if self.paused_at_micros.load(Ordering::Relaxed) < max_delivered_at_micros {
                self.paused_at_micros.store(0, Ordering::Relaxed);
            }
            let passed_micros = max(
                max_packet.sent_at_micros - max_packet.bandwidth_info.first_sent_at_micros, // sent
                timestamp_micros - max_delivered_at_micros,                                 // ack
            ) as u32;
            if passed_micros == 0 {
                fail!("Zero delivery time")
            }
            let delivered =
                delivered_count + new_received - max_packet.bandwidth_info.delivered_count;
            let rate = delivered * 1000000 / passed_micros;
            if !max_packet.bandwidth_info.is_paused
                || (rate >= self.windowed_max.load(Ordering::Relaxed))
            {
                let windowed_max = self.windowed_max_stat.on_value(rate, timestamp_micros);
                if windowed_max > 0 {
                    self.windowed_max.store(windowed_max, Ordering::Relaxed)
                }
            }
            self.delivered_count
                .compare_exchange(
                    delivered_count | Self::LOCK_BIT,
                    delivered_count + new_received,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .ok();
            break;
        }
        //  bdw_stats.on_packet_ack(packets_.max_packet().bdw_packet_info, packets_.max_packet().sent_at, now);
        /*
          if (paused_at_.is_in_past(info.delivered_now)) {
            paused_at_ = {};
          }
          auto sent_passed = sent_at.at() - info.first_sent_at.at();
          auto ack_passed = now.at() - info.delivered_now.at();
          auto passed = td::max(sent_passed, ack_passed);
          if (passed < 0.01) {
            VLOG(RLDP_INFO) << "Invalid passed " << passed;
          }
          auto delivered = delivered_count - info.delivered_count;
          on_rate_sample((double)delivered / passed, now, info.is_paused);
        */
        /*
          // ignore decrease of rate if is_paused == true
          if (is_paused && rate < windowed_max_bdw) {
            return;
          }
          windowed_max_bdw_stat.add_event(rate, now.at());
          auto windowed_max_bdw_sample = windowed_max_bdw_stat.get_stat(now.at()).get_stat();
          if (windowed_max_bdw_sample) {
            windowed_max_bdw = windowed_max_bdw_sample.value();
          }
        */
        Ok(())
    }
}

// Bottleneck Bandwidth and Round-trip propagation time (BBR)
struct Bbr {
    bandwidth_peak_round: AtomicPair,
    probe_bandwidth_cycle: AtomicPair,
    state: AtomicU8,
}

impl Bbr {
    const FACTOR_HIGH: f64 = 2.8;
    const FACTOR_LOW: f64 = 2.0;
    const MAX_CYCLES: u64 = 6;
    const MAX_GAIN: f64 = 1.25;
    const MIN_WINDOW: u32 = 100;

    const LOCK_BIT: u8 = 0x80;
    const STATE_START: u8 = 0;
    const STATE_DRAIN: u8 = 1;
    const STATE_PROBE_BANDWIDTH: u8 = 2;

    fn new() -> Self {
        Self {
            bandwidth_peak_round: AtomicPair::new(0, 0),
            probe_bandwidth_cycle: AtomicPair::new(0, 0),
            state: AtomicU8::new(Self::STATE_START),
        }
    }

    fn step(
        &self,
        roundtrip_stats: &RoundtripStats,
        bandwidth_stats: &BandwidthStats,
        in_flight_count: u32,
        timestamp_micros: u64,
    ) -> (f64, u32) {
        const PROBE_GAIN: [f64; Bbr::MAX_CYCLES as usize] =
            [0.75, 1.0, 1.0, 1.0, 1.0, Bbr::MAX_GAIN];

        fn window(bandwidth_max: u32, roundtrip_min_micros: u32, factor: f64) -> u32 {
            ((bandwidth_max as f64) * (roundtrip_min_micros as f64) * factor / 1000000.0) as u32
        }

        let (roundtrip_min_micros, bandwidth_max, roundtrip_round, bandwidth_round) = loop {
            let (roundtrip_min_micros, roundtrip_round) = roundtrip_stats.get_min_and_round();
            let bandwidth_max = bandwidth_stats.windowed_max.load(Ordering::Relaxed);
            let (bandwidth_round, bandwidth_peak) = self.bandwidth_peak_round.get();
            let mut bandwidth_round = bandwidth_round as u32;
            if bandwidth_max as f64 > (bandwidth_peak as f64) * Self::MAX_GAIN {
                let round = roundtrip_round as u64;
                let peak = bandwidth_max as u64;
                if !self.bandwidth_peak_round.update(round, peak, |prev, _| prev <= round) {
                    continue;
                }
                bandwidth_round = roundtrip_round;
            }
            break (roundtrip_min_micros, bandwidth_max, roundtrip_round, bandwidth_round);
        };

        let (state, probe_bandwidth_cycle) = loop {
            let mut state = self.state.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (state & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            let old_state = state;
            let mut probe_bandwidth_cycle_changed = false;
            let (mut cycle_at_micros, mut cycle) = self.probe_bandwidth_cycle.get();
            loop {
                match state {
                    Self::STATE_START => {
                        if bandwidth_round + 3 < roundtrip_round {
                            state = Self::STATE_DRAIN;
                            continue;
                        }
                    }
                    Self::STATE_DRAIN => {
                        if in_flight_count < window(bandwidth_max, roundtrip_min_micros, 1.0) {
                            cycle = rand::rngs::SmallRng::from_entropy()
                                .gen_range(1..Self::MAX_CYCLES - 1);
                            cycle_at_micros = timestamp_micros;
                            probe_bandwidth_cycle_changed = true;
                            state = Self::STATE_PROBE_BANDWIDTH;
                            continue;
                        }
                    }
                    _ => {
                        if cycle_at_micros + (roundtrip_min_micros as u64) < timestamp_micros {
                            cycle = (cycle + 1) % Self::MAX_CYCLES;
                            cycle_at_micros = timestamp_micros;
                            probe_bandwidth_cycle_changed = true
                        }
                    } //TODO: ProbeRoundtrip state
                }
                break;
            }
            if probe_bandwidth_cycle_changed {
                if !self
                    .probe_bandwidth_cycle
                    .update(cycle_at_micros, cycle, |at_micros, _| at_micros < cycle_at_micros)
                {
                    continue;
                }
            }
            if self
                .state
                .compare_exchange(
                    old_state | Self::LOCK_BIT,
                    state,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break (state, cycle);
            }
        };

        let speed = match state {
            Self::STATE_START => bandwidth_max as f64 * Self::FACTOR_HIGH,
            Self::STATE_DRAIN => bandwidth_max as f64 / Self::FACTOR_LOW,
            _ => PROBE_GAIN[probe_bandwidth_cycle as usize] * bandwidth_max as f64,
        };

        let factor = match state {
            Self::STATE_START | Self::STATE_DRAIN => Self::FACTOR_HIGH,
            _ => Self::FACTOR_LOW,
        };
        let window = window(bandwidth_max, roundtrip_min_micros, factor);
        //println!("window = {} bdw {} rtt {}", window, bandwidth_max, roundtrip_min_micros);
        let window = Self::MIN_WINDOW.max(window + 1);
        (speed, window)
    }
}

/*
template <class T, class Cmp>
struct MinMaxStat {
 public:
  using Event = T;
  td::optional<T> get_stat() const {
    return best_.copy();
  }

 private:
  td::optional<T> best_;
};

template <class T>
using MinStat = MinMaxStat<T, std::less<>>;

template <class T>
using MaxStat = MinMaxStat<T, std::greater<>>;
*/

struct BestStat(AtomicPair);

impl Default for BestStat {
    fn default() -> Self {
        Self(AtomicPair::new(0, 0))
    }
}

impl BestStat {
    fn get(&self) -> (u32, u64) {
        let BestStat(shared) = self;
        let (timestamp_micros, value) = shared.get();
        (value as u32, timestamp_micros)
    }

    fn update(&self, value: u32, timestamp_micros: u64) -> bool {
        let BestStat(shared) = self;
        shared.update(timestamp_micros, value as u64, |old_timestamp, new_timestamp| {
            old_timestamp <= new_timestamp
        })
    }
}

struct LossStats {
    ack_lost: AtomicU64, // ack(32) | lost(32)
    loss: AtomicFixedPoint,
    s2: f64,
}

impl LossStats {
    const MAX_LOSS: f64 = 0.2;
    const MIN_LOSS: f64 = 0.001;
    const P: f64 = 0.000000001;
    const UPDATE_WINDOW: u32 = 1000;

    fn new() -> Result<Self> {
        let s = Self::ndtri_fast(Self::P);
        let ret =
            Self { ack_lost: AtomicU64::new(0), loss: AtomicFixedPoint::new(0.1)?, s2: s * s };
        Ok(ret)
    }

    fn calc_extra_symbols(&self, new_ack: u32, new_lost: u32, left_symbols: u32) -> Result<u32> {
        let loss = loop {
            let (old_loss, org_loss) = self.loss.load();
            let ack_lost = self.ack_lost.load(Ordering::Acquire);
            let ack = (ack_lost >> 32) as u32 + new_ack;
            let lost = ack_lost as u32 + new_lost;
            if ack + lost > Self::UPDATE_WINDOW {
                let mut loss = (lost as f64) / (ack + lost) as f64;
                if loss < Self::MIN_LOSS {
                    loss = Self::MIN_LOSS
                } else if loss > Self::MAX_LOSS {
                    loss = Self::MAX_LOSS
                }
                if !self.loss.store(loss, Some(org_loss))? {
                    continue;
                }
                self.ack_lost.store(0, Ordering::Release);
                break loss;
            } else {
                let new_ack_lost = ((ack as u64) << 32) | (lost as u64);
                if !self
                    .ack_lost
                    .compare_exchange(ack_lost, new_ack_lost, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
                {
                    continue;
                }
                break old_loss;
            }
        };
        let n = left_symbols as f64;
        let l = loss * (loss - 1.0);
        let a = (1.0 - loss) * (1.0 - loss);
        let b = l * (2.0 * n + self.s2);
        let c = loss * loss * n * n + self.s2 * n * l;
        let x = (-b + (b * b - 4.0 * a * c).sqrt()) / (2.0 * a);
        Ok((x + n + 1.0) as u32)
    }

    fn ndtri_fast(p: f64) -> f64 {
        if p < 0.0000000002 {
            6.361340902404
        } else if p < 0.000000002 {
            5.997807015008
        } else if p < 0.00000002 {
            5.612001244175
        } else if p < 0.0000002 {
            5.199337582193
        } else if p < 0.000002 {
            4.753424308823
        } else if p < 0.00002 {
            4.264890793923
        } else if p < 0.0002 {
            3.719016485456
        } else if p < 0.002 {
            3.090232306168
        } else if p < 0.02 {
            2.326347874041
        } else {
            1.281551565545
        }
    }
}

struct MinMaxStat {
    current: BestStat,
    duration_ms: u64,
    next: BestStat,
    ordering: CmpOrdering,
}

impl Default for MinMaxStat {
    fn default() -> Self {
        Self {
            current: BestStat::default(),
            duration_ms: 0,
            next: BestStat::default(),
            ordering: CmpOrdering::Less,
        }
    }
}

impl MinMaxStat {
    fn with_params(duration_ms: u64, ordering: CmpOrdering) -> Self {
        Self { duration_ms, ordering, ..Default::default() }
    }

    fn on_value(&self, value: u32, mut timestamp_micros: u64) -> u32 {
        self.update(&mut timestamp_micros);
        loop {
            let (current, current_timestamp_micros) = self.current.get();
            let current = current as u32;
            if (current == 0) || (value.cmp(&current) == self.ordering) {
                if !self.current.update(value, current_timestamp_micros) {
                    continue;
                }
            }
            let (next, next_timestamp_micros) = self.next.get();
            let next = next as u32;
            if (next == 0) || (value.cmp(&next) == self.ordering) {
                if !self.next.update(value, next_timestamp_micros) {
                    continue;
                }
            }
            break;
        }
        let (current, _) = self.current.get();
        current
    }

    fn update(&self, timestamp_micros: &mut u64) {
        loop {
            let (next, next_timestamp_micros) = self.next.get();
            if *timestamp_micros < next_timestamp_micros {
                *timestamp_micros = next_timestamp_micros;
            }
            if self.duration_ms == 0 {
                break;
            }
            if next_timestamp_micros + self.duration_ms * 2000 < *timestamp_micros {
                if !self.current.update(0, *timestamp_micros)
                    || !self.next.update(0, *timestamp_micros)
                {
                    continue;
                }
            } else if next_timestamp_micros + self.duration_ms * 1000 < *timestamp_micros {
                if !self.current.update(next, next_timestamp_micros)
                    || !self.next.update(0, *timestamp_micros)
                {
                    continue;
                }
            }
            break;
        }
    }
}

/*
 public:
  TimedStat() : TimedStat(0, 0) {
  }
  template <class EventT>
  const StatT &get_stat(double now) {
    update(now);
    return current_;
  }
  std::pair<StatT, double> stat_duration(double now) {
    update(now);
    return std::make_pair(current_, now - current_timestamp_);
  }
  void clear_events() {
    current_.clear();
    next_.clear();
  }

};
*/

pub(crate) struct PacketBandwidthInfo {
    delivered_count: u32,
    delivered_at_micros: u64,
    first_sent_at_micros: u64,
    is_paused: bool,
}

#[derive(Default)]
struct RoundtripStats {
    last_micros: AtomicU32,
    smoothed_micros: AtomicFixedPoint,
    variance_micros: AtomicFixedPoint,
    windowed_min_micros: AtomicU32,
    windowed_min_micros_stat: MinMaxStat,
    round: AtomicU32,
    round_at_micros: AtomicU64,
}

impl RoundtripStats {
    const LOCK_BIT: u32 = 0x80000000;
    const MAX_ROUNDTRIP_MICROS: u32 = 10000000;
    const MIN_ROUNDTRIP_MICROS: u32 = 10000;
    const STAT_DURATION_MS: u64 = 5000;

    fn new(initial_roundtrip_micros: u32) -> Result<Self> {
        let ret = Self {
            windowed_min_micros: AtomicU32::new(initial_roundtrip_micros),
            windowed_min_micros_stat: MinMaxStat::with_params(
                Self::STAT_DURATION_MS,
                std::cmp::Ordering::Less,
            ),
            smoothed_micros: AtomicFixedPoint::new(f64::NAN)?,
            ..Default::default()
        };
        Ok(ret)
    }

    fn calc_loss_delay(&self, config: &StatsConfigV2) -> u64 {
        loop {
            let last_micros = self.last_micros.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (last_micros & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            let ret = if last_micros > 0 {
                let (smoothed_micros, _) = self.smoothed_micros.load();
                last_micros.max(smoothed_micros as u32)
            } else {
                config.initial_roundtrip_micros
            };
            let ret = ((ret as f64) * 8.0 / 7.0) as u64;
            self.last_micros
                .compare_exchange(
                    last_micros | Self::LOCK_BIT,
                    last_micros,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .ok();
            break ret;
        }
    }

    fn calc_probe_delay(&self, probe_k: u32, config: &StatsConfigV2) -> u64 {
        loop {
            let last_micros = self.last_micros.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (last_micros & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            let ret = if last_micros > 0 {
                let (smoothed_micros, _) = self.smoothed_micros.load();
                let (variance_micros, _) = self.variance_micros.load();
                let roundtrip_micros =
                    smoothed_micros + variance_micros * 4.0 + config.max_ack_delay_micros as f64;
                roundtrip_micros as u64 * probe_k as u64
            } else {
                config.initial_roundtrip_micros as u64 * 2
            };
            self.last_micros
                .compare_exchange(
                    last_micros | Self::LOCK_BIT,
                    last_micros,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .ok();
            break ret;
        }
    }

    fn get_min_and_round(&self) -> (u32, u32) {
        loop {
            let last_micros = self.last_micros.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (last_micros & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            let min_micros = self.windowed_min_micros.load(Ordering::Relaxed);
            let round = self.round.load(Ordering::Relaxed);
            self.last_micros
                .compare_exchange(
                    last_micros | Self::LOCK_BIT,
                    last_micros,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .ok();
            break (min_micros, round);
        }
    }

    fn update(&self, mut roundtrip_micros: u32, timestamp_micros: u64) -> Result<()> {
        if (roundtrip_micros == 0) || (roundtrip_micros > Self::MAX_ROUNDTRIP_MICROS) {
            fail!("Suspicious roundtrip sample {}", roundtrip_micros)
        }
        roundtrip_micros = roundtrip_micros.max(Self::MIN_ROUNDTRIP_MICROS);
        let windowed_min_micros =
            self.windowed_min_micros_stat.on_value(roundtrip_micros, timestamp_micros);
        loop {
            let last_micros = self.last_micros.fetch_or(Self::LOCK_BIT, Ordering::Acquire);
            if (last_micros & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            loop {
                let (org_smoothed_micros, old_smoothed_micros) = self.smoothed_micros.load();
                let mut smoothed_micros = org_smoothed_micros;
                let (mut variance_micros, old_variance_micros) = self.variance_micros.load();
                if smoothed_micros.is_nan() {
                    smoothed_micros = roundtrip_micros as f64;
                    variance_micros = roundtrip_micros as f64 / 2.0;
                } else {
                    smoothed_micros += (roundtrip_micros as f64 - smoothed_micros) / 8.0;
                    let var_micros = (smoothed_micros - roundtrip_micros as f64).abs();
                    variance_micros += (var_micros - variance_micros) / 4.0;
                }
                if !self.smoothed_micros.store(smoothed_micros, Some(old_smoothed_micros))? {
                    continue;
                }
                if !self.variance_micros.store(variance_micros, Some(old_variance_micros))? {
                    self.smoothed_micros.store(org_smoothed_micros, Some(old_smoothed_micros))?;
                    continue;
                }
                let round_at_micros = self.round_at_micros.load(Ordering::Relaxed);
                if round_at_micros + (smoothed_micros as u64) < timestamp_micros {
                    self.round_at_micros.store(timestamp_micros, Ordering::Relaxed);
                    self.round.fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
            if windowed_min_micros > 0 {
                self.windowed_min_micros.store(windowed_min_micros, Ordering::Relaxed)
            }
            self.last_micros
                .compare_exchange(
                    last_micros | Self::LOCK_BIT,
                    roundtrip_micros,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .ok();
            break;
        }
        /*
          last_rtt = rtt_sample;

          windowed_min_rtt_stat.add_event(rtt_sample, now.at());
          auto windowed_min_rtt_sample = windowed_min_rtt_stat.get_stat(now.at()).get_stat();
          if (windowed_min_rtt_sample) {
            windowed_min_rtt = windowed_min_rtt_sample.value();
          }

          if (smoothed_rtt < 0) {
            // ignore ack_delay just because
            min_rtt = last_rtt;
            smoothed_rtt = last_rtt;
            rtt_var = last_rtt / 2;
          } else {
            if (rtt_sample < min_rtt) {
              min_rtt = rtt_sample;
            }

            double adjusted_rtt = rtt_sample;
            if (adjusted_rtt - ack_delay > min_rtt) {
              adjusted_rtt -= ack_delay;
            }

            smoothed_rtt += (adjusted_rtt - smoothed_rtt) / 8;
            double var = fabs(smoothed_rtt - adjusted_rtt);
            rtt_var += (var - rtt_var) / 4;
          }

          if (td::Timestamp::in(smoothed_rtt, rtt_round_at).is_in_past(now)) {
            rtt_round_at = now;
            rtt_round++;
          }
        */
        Ok(())
    }
}

pub(crate) struct StatsV1 {
    roundtrip: AverageTotal,
    start: Instant,
    timeout: AtomicU64,
    updated_at_ms: AtomicU64,
}

impl StatsV1 {
    pub(crate) fn new(min_timeout_ms: u64) -> Self {
        let start = Instant::now();
        Self {
            updated_at_ms: AtomicU64::new(0),
            roundtrip: AverageTotal::new(),
            start,
            timeout: AtomicU64::new(Self::calc_timeout(0, min_timeout_ms)),
        }
    }

    pub(crate) fn on_connect(&self) {
        self.updated_at_ms.store(self.timestamp_ms(), Ordering::Relaxed)
    }

    pub(crate) fn roundtrip(&self) -> u64 {
        self.roundtrip.get()
    }

    pub(crate) fn timeout(&self) -> u64 {
        self.timeout.load(Ordering::Acquire)
    }

    pub(crate) fn timestamp_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub(crate) fn try_timeout(&self, start_ms: u64) -> bool {
        // This function is called if there is no updates in transfer
        // Timeout calculation logic:
        // 0. Take current timeout
        // 1. Calculate time since last update
        // 2. If time since last update is not less than current timeout, then we failed
        // 3. Calculate overall transfer time.
        // 4. If overall transfer time is less then current timeout then we are ok
        // 5. If overall transfer time is more than (K * current timeout) then we are failed
        // 6  If time since last update is more than (current timeout / K) then we are failed
        // K = 10
        let timeout = self.timeout();
        let updated_ms = self.timestamp_ms() - self.updated_at_ms.load(Ordering::Relaxed);
        let ret = if updated_ms == 0 {
            false
        } else if updated_ms >= timeout {
            true
        } else {
            let elapsed_ms = self.timestamp_ms() - start_ms;
            if elapsed_ms < timeout {
                false
            } else {
                (elapsed_ms / timeout > 10) || (timeout / updated_ms < 10)
            }
        };
        if ret {
            self.roundtrip.update(RldpNode::TIMEOUT_MAX_MS.min(self.roundtrip() * 2));
        }
        ret
    }

    pub(crate) fn update(&self, min_timeout_ms: u64) {
        let timestamp_ms = self.timestamp_ms();
        let elapsed_ms = timestamp_ms - self.updated_at_ms.load(Ordering::Relaxed);
        self.roundtrip.update(elapsed_ms);
        self.timeout.store(Self::calc_timeout(self.roundtrip(), min_timeout_ms), Ordering::Release);
        self.updated_at_ms.store(timestamp_ms, Ordering::Relaxed)
    }

    fn calc_timeout(roundtrip: u64, min_timeout_ms: u64) -> u64 {
        let roundtrip = roundtrip.min(RldpNode::TIMEOUT_MAX_MS);
        roundtrip.max(min_timeout_ms)
    }
}

pub(crate) struct StatsV2 {
    pub(crate) v1: StatsV1,
    bandwidth: BandwidthStats,
    bbr: Bbr,
    capacity: AtomicFixedPoint,
    capacity_at_micros: AtomicU64,
    config: StatsConfigV2,
    in_flight_symbols_window: AtomicU64, // window (32) | in-flight symbols (32)
    loss: LossStats,
    roundtrip: RoundtripStats,
    speed: AtomicFixedPoint,
    wakeup_at_micros: AtomicU64,
}

impl StatsV2 {
    const INITIAL_CAPACITY: f64 = 20.0;
    const INITIAL_SPEED: f64 = 10.0;
    const MAX_CAPACITY: f64 = 40.0;
    const SIZE: f64 = 1.0;
    const TIME_GRANULARITY: f64 = 0.001;

    pub(crate) fn new(config: StatsConfigV2, min_timeout_ms: u64) -> Result<Self> {
        let ret = Self {
            bandwidth: BandwidthStats::new(config.initial_bandwidth),
            roundtrip: RoundtripStats::new(config.initial_roundtrip_micros)?,
            bbr: Bbr::new(),
            capacity: AtomicFixedPoint::new(Self::INITIAL_CAPACITY)?,
            capacity_at_micros: AtomicU64::new(0),
            config,
            in_flight_symbols_window: AtomicU64::new(0),
            loss: LossStats::new()?,
            speed: AtomicFixedPoint::new(Self::INITIAL_SPEED)?,
            v1: StatsV1::new(min_timeout_ms),
            wakeup_at_micros: AtomicU64::new(0),
        };
        Ok(ret)
    }

    pub(crate) fn bbr_step(&self) -> Result<()> {
        let timestamp_micros = self.timestamp_micros();
        let in_flight_symbols_window = self.in_flight_symbols_window.load(Ordering::Acquire);
        let in_flight_symbols = in_flight_symbols_window as u32;
        let window = (in_flight_symbols_window >> 32) as u32;
        if (in_flight_symbols > window) || (in_flight_symbols == 0) {
            self.bandwidth.paused_at_micros.store(timestamp_micros, Ordering::Relaxed)
        }
        let (new_speed, new_window) =
            self.bbr.step(&self.roundtrip, &self.bandwidth, in_flight_symbols, timestamp_micros);
        self.speed.store(new_speed, None)?;
        if window < new_window {
            self.in_flight_symbols_window
                .fetch_add(((new_window - window) as u64) << 32, Ordering::Release);
        } else if window > new_window {
            self.in_flight_symbols_window
                .fetch_sub(((window - new_window) as u64) << 32, Ordering::Release);
        }
        /*
          if (in_flight_count_ > congestion_window_) {
            bdw_stats_.on_pause(now);
          }
          if (in_flight_count_ == 0) {
            bdw_stats_.on_pause(now);
          }
        */
        /*
          bbr_.step(rtt_stats_, bdw_stats_, in_flight_count_, td::Timestamp::now());
          //LOG(ERROR) << td::format::as_time(rtt_stats_.windowed_min_rtt) << " "
          //<< td::format::as_size((td::int64)bdw_stats_.windowed_max_bdw * 768) << " " << rtt_stats_.rtt_round;
          double speed = bbr_.get_rate();
          td::uint32 congestion_window = bbr_.get_window_size();
          pacer_.set_speed(speed);
          congestion_window_ = congestion_window;
        */
        Ok(())
    }

    pub(crate) fn calc_extra_symbols(
        &self,
        new_ack: u32,
        new_loss: u32,
        left_symbols: u32,
    ) -> Result<u32> {
        self.loss.calc_extra_symbols(new_ack, new_loss, left_symbols)
    }

    pub(crate) fn calc_loss_delay(&self) -> u64 {
        self.timestamp_micros().saturating_sub(self.roundtrip.calc_loss_delay(&self.config))
    }

    pub(crate) fn calc_probe_delay(&self, probe_k: u32) -> u64 {
        self.timestamp_micros() + self.roundtrip.calc_probe_delay(probe_k, &self.config)
    }

    pub(crate) fn config(&self) -> &StatsConfigV2 {
        &self.config
    }

    pub(crate) fn is_only_probe(&self) -> bool {
        let in_flight_symbols_window = self.in_flight_symbols_window.load(Ordering::Acquire);
        let in_flight_symbols = in_flight_symbols_window as u32;
        let window = (in_flight_symbols_window >> 32) as u32;
        //println!("probe {} {}", in_flight_symbols, window);
        (in_flight_symbols > window)
            || (self.wakeup_at_micros.load(Ordering::Relaxed) > self.timestamp_micros())
    }

    pub(crate) fn on_ack(
        &self,
        max_packet: SendPacketV2,
        new_received: u32,
        min_timeout_ms: u64,
    ) -> Result<()> {
        let timestamp_micros = self.timestamp_micros();
        self.roundtrip
            .update((timestamp_micros - max_packet.sent_at_micros) as u32, timestamp_micros)?;
        self.bandwidth.update(max_packet, new_received, timestamp_micros)?;
        self.v1.update(min_timeout_ms);
        Ok(())
    }

    pub(crate) fn on_drop(&self, new_dropped: u32) {
        self.in_flight_symbols_window.fetch_sub(new_dropped as u64, Ordering::Release);
    }

    pub(crate) fn on_send(
        &self,
        is_probe: bool,
        first_sent_at_micros: u64,
    ) -> Result<PacketBandwidthInfo> {
        if !is_probe {
            loop {
                let timestamp_micros = self.timestamp_micros();
                let mut capacity_at_micros = self.capacity_at_micros.load(Ordering::Relaxed);
                let (mut capacity, old_capacity) = self.capacity.load();
                let (speed, _) = self.speed.load();
                if (capacity_at_micros > 0) && (capacity_at_micros < timestamp_micros) {
                    capacity += (timestamp_micros - capacity_at_micros) as f64 * speed * 1000.0;
                    capacity = capacity.min(Self::MAX_CAPACITY.max(speed * Self::TIME_GRANULARITY));
                }
                let update_wakeup = if Self::SIZE < capacity {
                    capacity -= Self::SIZE;
                    capacity_at_micros = timestamp_micros;
                    false
                } else {
                    let size = Self::SIZE - capacity;
                    capacity = 0.0;
                    capacity_at_micros = timestamp_micros + (size / speed) as u64;
                    true
                };
                if self.capacity.store(capacity, Some(old_capacity))? {
                    self.capacity_at_micros.store(capacity_at_micros, Ordering::Relaxed);
                    if update_wakeup {
                        self.wakeup_at_micros.store(capacity_at_micros, Ordering::Relaxed)
                    }
                    break;
                }
            }
        }
        self.in_flight_symbols_window.fetch_add(1, Ordering::Release);
        Ok(self.bandwidth.on_send(first_sent_at_micros))
    }

    pub(crate) fn set_roundtrip(&self, roundtrip: u64) -> Result<()> {
        if roundtrip != self.v1.roundtrip() {
            let roundtrip_micros = roundtrip * 1000;
            if roundtrip_micros >= u32::MAX as u64 {
                fail!("Roundtrip {} micros is too big", roundtrip_micros)
            }
            self.v1.roundtrip.update(roundtrip);
            self.roundtrip.update(roundtrip_micros as u32, self.timestamp_micros())
        } else {
            Ok(())
        }
    }

    pub(crate) fn timestamp_micros(&self) -> u64 {
        self.v1.start.elapsed().as_micros() as u64
    }
}

pub(crate) struct StatsConfigV2 {
    initial_bandwidth: u32,
    initial_roundtrip_micros: u32,
    max_ack_delay_micros: u32,
    pub(crate) packet_threshold: u32,
}

impl Default for StatsConfigV2 {
    fn default() -> Self {
        Self {
            initial_bandwidth: Self::INITIAL_BANDWIDTH,
            initial_roundtrip_micros: Self::INITIAL_ROUNDTRIP_MICROS,
            max_ack_delay_micros: Self::MAX_ACK_DELAY_MICROS,
            packet_threshold: Self::PACKET_TRESHOLD,
        }
    }
}

impl StatsConfigV2 {
    const MAX_ACK_DELAY_MICROS: u32 = 10000;
    const INITIAL_BANDWIDTH: u32 = 100;
    const INITIAL_ROUNDTRIP_MICROS: u32 = 500000;
    const PACKET_TRESHOLD: u32 = 3;
}
