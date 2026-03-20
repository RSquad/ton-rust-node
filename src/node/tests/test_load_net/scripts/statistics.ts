import {
    mean as ssMean,
    median as ssMedian,
    standardDeviation as ssStd,
    quantileSorted as ssQuantileSorted,
} from "simple-statistics";

export type StatEvent = {
    beginTs: number;
    endTs: number;
    deltaError: number;
};

export type StatDurationStats = {
    mean: string;
    median: string;
    stdev: string;
    min: string;
    max: string;
    p50: string;
    p90: string;
    p95: string;
    p99: string;
};

const nonnegDuration = (e: StatEvent) => Math.max(0, e.endTs - e.beginTs);
const sorted = (xs: number[]) => [...xs].sort((a, b) => a - b);

export class Statistics {
    private events: StatEvent[] = [];

    public addEvent(
        beginTs: number,
        endTs: number,
        deltaError: number,
    ) {
        this.events.push({ beginTs, endTs, deltaError });
    }

    public summarize(): { count: number, stat: StatDurationStats[] } {
        return Statistics.summarizeImpl(this.events);
    }

    private static durationSummary(durations: number[]): StatDurationStats {
        const n = durations.length;
        if (n === 0) {
            return { mean: "NaN", median: "NaN", stdev: "NaN", min: "NaN", max: "NaN", p50: "NaN", p90: "NaN", p95: "NaN", p99: "NaN" };
        }
        const s = sorted(durations);
        return {
            mean: ssMean(durations).toFixed(3),
            median: ssMedian(s).toFixed(3),
            stdev: ssStd(durations).toFixed(3),
            min: s[0].toFixed(3),
            max: s[n - 1].toFixed(3),
            p50: ssQuantileSorted(s, 0.50).toFixed(3),
            p90: ssQuantileSorted(s, 0.90).toFixed(3),
            p95: ssQuantileSorted(s, 0.95).toFixed(3),
            p99: ssQuantileSorted(s, 0.99).toFixed(3),
        };
    }

    private static summarizeImpl(events: StatEvent[]): { count: number, stat: StatDurationStats[] } {
        const durations = events.map(nonnegDuration).filter(Number.isFinite);
        const stat = Statistics.durationSummary(durations);

        /*
        const deltas = events
            .map(e => e.deltaError)
            .filter((x): x is number => Number.isFinite(x as number));

        const deltaStats = deltas.length
            ? {
                mean: ssMean(deltas).toFixed(3),
                median: ssMedian(sorted(deltas)).toFixed(3),
                stdev: ssStd(deltas).toFixed(3),
                min: Math.min(...deltas).toFixed(3),
                max: Math.max(...deltas).toFixed(3),
            }
            : undefined;
        */

        return {
            count: events.length,
            stat: [stat],
        };
    }
}