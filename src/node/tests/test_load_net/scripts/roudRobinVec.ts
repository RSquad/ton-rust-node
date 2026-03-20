export class RoundRobinVec<T> {
    private readonly pool: T[];
    private nextId = 0;

    constructor(items: Iterable<T>) {
        this.pool = Array.from(items);
        if (this.pool.length === 0) {
            throw new Error("Pool must contain at least one element");
        }
    }

    next(): T {
        const item = this.pool[this.nextId];
        this.nextId = (this.nextId + 1) % this.pool.length;
        return item;
    }

    size(): number {
        return this.pool.length;
    }

    reset(startIndex = 0): void {
        if (startIndex < 0 || startIndex >= this.pool.length) {
            throw new Error("startIndex out of range");
        }
        this.nextId = startIndex;
    }
}
