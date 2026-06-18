type Producer<T> = () => Promise<T> | undefined;

interface RunWithLimitResult<T> {
    results: T[];
    errors: unknown[];
}

export async function runWithLimit<T>(
    maxInFlight: number,
    producer: Producer<T>
): Promise<RunWithLimitResult<T>> {
    const results: T[] = [];
    const errors: unknown[] = [];
    const inFlight: Promise<void>[] = [];

    const launch = (p: Promise<T>) => {
        const wrapped = p
            .then((res) => {
                results.push(res);
            })
            .catch((err) => {
                console.error(`runWithLimit error!`, err);
                errors.push(err);
            })
            .finally(() => {
                const idx = inFlight.indexOf(wrapped);
                if (idx !== -1) inFlight.splice(idx, 1);
            });

        inFlight.push(wrapped);
    };

    for (let i = 0; i < maxInFlight; i++) {
        const next = producer();
        if (next == null) {
            break;
        }
        launch(next);
    }

    while (inFlight.length > 0) {
        await Promise.race(inFlight);

        while (inFlight.length < maxInFlight) {
            const next = producer();
            if (next == null) {
                break;
            }
            launch(next);
        }
    }

    return { results, errors };
}
