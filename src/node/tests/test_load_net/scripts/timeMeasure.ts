type Clock = () => number; // milliseconds

function defaultClock(): number {
    // Prefer a monotonic clock for duration accuracy.
    if (typeof performance !== "undefined" && typeof performance.now === "function") {
        return performance.now();
    }
    // Node.js high-resolution monotonic time
    // Convert ns -> ms
    // @ts-ignore
    if (typeof process !== "undefined" && typeof process.hrtime === "function" && typeof process.hrtime.bigint === "function") {
        // @ts-ignore
        return Number(process.hrtime.bigint()) / 1_000_000;
    }
    // Fallback (Date.now is wall-clock, can jump)
    return Date.now();
}

let _idSeq = 1;

export interface EventHandle {
    id: number;
    name: string;
    startMs: number;
    endMs?: number;
}

export interface StatsSnapshot {
    name: string;
    count: number;
    avg: string;
    min: string;
    max: string;
    stdDev: string;
    p50?: string;
    p90?: string;
    p95?: string;
    p99?: string;
}

class RunningStats {
    private _count = 0;
    private _mean = 0;
    private _m2 = 0;
    min = Number.POSITIVE_INFINITY;
    max = Number.NEGATIVE_INFINITY;

    get count() { return this._count; }
    get mean() { return this._mean; }
    get variance() { return this._count > 1 ? this._m2 / (this._count - 1) : 0; }
    get stdDev() { return Math.sqrt(this.variance); }

    push(x: number) {
        this._count += 1;
        if (x < this.min) this.min = x;
        if (x > this.max) this.max = x;

        // Welford
        const delta = x - this._mean;
        this._mean += delta / this._count;
        const delta2 = x - this._mean;
        this._m2 += delta * delta2;
    }
}

export interface TimeMeasureOptions {
    keepSamples: boolean;
    maxSamplesPerName: number;
}

export class TimeMeasure {
    private clock: Clock;
    private keepSamples: boolean;
    private maxSamplesPerName: number;

    private openById = new Map<number, EventHandle>();
    private openStacksByName = new Map<string, number[]>(); // name -> stack of ids (for stopLatest)
    private statsByName = new Map<string, RunningStats>();
    private samplesByName = new Map<string, number[]>(); // durations (ms), capped

    constructor(opts: TimeMeasureOptions) {
        this.clock = defaultClock;
        this.keepSamples = opts.keepSamples;
        this.maxSamplesPerName = opts.maxSamplesPerName;
    }

    /**
     * Start an event under a given name. Returns an EventHandle id.
     */
    start(name: string): number {
        const id = _idSeq++;
        const now = this.clock();
        const handle: EventHandle = { id, name, startMs: now };
        this.openById.set(id, handle);

        let stack = this.openStacksByName.get(name);
        if (!stack) {
            stack = [];
            this.openStacksByName.set(name, stack);
        }
        stack.push(id);

        return id;
    }

    /**
     * Stop an event by id. Returns duration in ms.
     */
    stop(id: number): number {
        const handle = this.openById.get(id);
        if (!handle) {
            throw new Error(`TimeMeasure.stop: invalid or already-stopped id ${id}`);
        }
        handle.endMs = this.clock();
        this.openById.delete(id);

        // pop from its name stack (may not be top if stop(id) is used out-of-order)
        const stack = this.openStacksByName.get(handle.name);
        if (stack) {
            const idx = stack.lastIndexOf(id);
            if (idx >= 0) stack.splice(idx, 1);
            if (stack.length === 0) this.openStacksByName.delete(handle.name);
        }

        const duration = handle.endMs - handle.startMs;
        this._record(handle.name, duration);
        return duration;
    }

    /**
     * Stop an event by id (catch error). Returns duration in ms.
     */
    stopErr(id: number): number {
        const handle = this.openById.get(id);
        if (!handle) {
            throw new Error(`TimeMeasure.stop: invalid or already-stopped id ${id}`);
        }
        handle.endMs = this.clock();
        this.openById.delete(id);

        // pop from its name stack (may not be top if stop(id) is used out-of-order)
        const stack = this.openStacksByName.get(handle.name);
        if (stack) {
            const idx = stack.lastIndexOf(id);
            if (idx >= 0) stack.splice(idx, 1);
            if (stack.length === 0) this.openStacksByName.delete(handle.name);
        }

        const duration = handle.endMs - handle.startMs;
        this._record(handle.name + "_ERR", duration);
        return duration;
    }

    /**
     * Time a synchronous function and record under name. Returns fn() result.
     */
    withTiming<T>(name: string, fn: () => T): T {
        const id = this.start(name);
        try {
            return fn();
        } finally {
            this.stop(id);
        }
    }

    /**
     * Time an async function/promise and record under name. Returns awaited result.
     */
    async withTimingAsync<T>(name: string, fn: () => Promise<T>): Promise<T> {
        const id = this.start(name);
        try {
            return await fn();
        } finally {
            this.stop(id);
        }
    }

    /**
     * Snapshot stats for all names or a single name.
     * If keepSamples=true, includes basic percentiles (50/90/95/99).
     */
    snapshot(name?: string): StatsSnapshot[] | StatsSnapshot | undefined {
        if (name) {
            const s = this.statsByName.get(name);
            if (!s) return undefined;
            return this._toSnapshot(name, s);
        }
        const out: StatsSnapshot[] = [];
        for (const [n, s] of this.statsByName.entries()) {
            out.push(this._toSnapshot(n, s));
        }
        return out;
    }

    reset(name?: string) {
        if (name) {
            this.statsByName.delete(name);
            this.samplesByName.delete(name);

            return;
        }
        this.statsByName.clear();
        this.samplesByName.clear();
    }

    openCount(name?: string): number {
        if (!name) return this.openById.size;
        const stack = this.openStacksByName.get(name);
        return stack ? stack.length : 0;
    }

    private _record(name: string, durationMs: number) {
        let rs = this.statsByName.get(name);
        if (!rs) {
            rs = new RunningStats();
            this.statsByName.set(name, rs);
        }
        rs.push(durationMs);

        if (this.keepSamples) {
            let arr = this.samplesByName.get(name);
            if (!arr) {
                arr = [];
                this.samplesByName.set(name, arr);
            }
            arr.push(durationMs);

            if (arr.length > this.maxSamplesPerName) {
                const over = arr.length - this.maxSamplesPerName;
                arr.splice(0, over);
            }
        }
    }

    private _toSnapshot(name: string, s: RunningStats): StatsSnapshot {
        const snap: StatsSnapshot = {
            name,
            count: s.count,
            avg: ((s.mean || 0) / 1000.0).toFixed(3),
            min: ((Number.isFinite(s.min) ? s.min : 0) / 1000.0).toFixed(3),
            max: ((Number.isFinite(s.max) ? s.max : 0) / 1000.0).toFixed(3),
            stdDev: ((s.stdDev || 0) / 1000.0).toFixed(3),
        };

        if (this.keepSamples) {
            const arr = this.samplesByName.get(name);
            if (arr && arr.length) {
                const sorted = [...arr].sort((a, b) => a - b);
                const pick = (p: number) => {
                    if (sorted.length === 1) return sorted[0];
                    const i = Math.min(sorted.length - 1, Math.max(0, Math.round((p / 100) * (sorted.length - 1))));
                    return sorted[i];
                };
                snap.p50 = (pick(50) / 1000.0).toFixed(3);
                snap.p90 = (pick(90) / 1000.0).toFixed(3);
                snap.p95 = (pick(95) / 1000.0).toFixed(3);
                snap.p99 = (pick(99) / 1000.0).toFixed(3);
            }
        }
        return snap;
    }
}
