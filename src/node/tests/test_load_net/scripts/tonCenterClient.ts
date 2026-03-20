import { Address, Contract, OpenedContract, TonClient, Transaction, TupleItem, TupleReader } from "@ton/ton"
import { AppGlobals } from "./globals";
import { logAxiosError } from "./logAxiosError";

export type TonCenterClientParameters = {
    endpoint: string;
    timeout?: number;
    apiKey?: string;
}

export interface TonBlockIdExt {
    "@type": "ton.blockIdExt";
    workchain: number;
    shard: string;
    seqno: number;
    root_hash: string;
    file_hash: string;
    "@extra"?: string;
}

export interface LookupBlockResponseOk {
    ok: true;
    result: TonBlockIdExt;
}

export interface LookupBlockParams {
    workchain: number;
    shard: string | number | bigint;
    lt: string | number | bigint;
    timeoutMs?: number;
}

export interface GetShardBlockProofParams {
    workchain: number;
    shard: string | number | bigint;
    seqno: number | bigint;
    from_seqno?: number | bigint;
    timeoutMs?: number;
}

export interface ShardBlockProof {
    "@type": "blocks.shardBlockProof";
    from?: TonBlockIdExt;
    mc_id?: TonBlockIdExt;
    [k: string]: unknown;
}

export interface GetShardBlockProofResponseOk {
    ok: true;
    result: ShardBlockProof;
}

export interface GetBlockHeaderParams {
    workchain: number;
    shard: string | number | bigint;
    seqno: number | bigint;
    root_hash?: string;
    file_hash?: string;
    timeoutMs?: number;
}

export interface BlockHeader {
    id: TonBlockIdExt;
    global_id?: number;
    version?: number;
    after_merge?: boolean;
    before_split?: boolean;
    after_split?: boolean;
    want_merge?: boolean;
    want_split?: boolean;
    validator_list_hash_short?: number;
    catchain_seqno?: number;
    min_ref_mc_seqno?: number;
    is_key_block?: boolean;
    prev_key_block_seqno?: number;
    start_lt?: string | number;
    end_lt?: string | number;
    gen_utime?: number;
    vert_seqno?: number;
    [k: string]: unknown;
}

export interface GetBlockHeaderResponseOk {
    ok: true;
    result: BlockHeader;
}

export class TonCenterClient {
    readonly parameters: TonCenterClientParameters;
    private tonClient: TonClient;

    constructor(parameters: TonCenterClientParameters) {
        this.parameters = parameters;
        this.tonClient = new TonClient({
            endpoint: parameters.endpoint + "jsonRPC",
            timeout: parameters.timeout,
            apiKey: parameters.apiKey,
        });
    }

    public async sendBoc(
        bocB64: string,
        timeoutMs = 15_000,
    ): Promise<void> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("sendBoc");

        const trySendBocImpl = (): Promise<void> => this.sendBocImpl(bocB64, timeoutMs)
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "sendBoc");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("sendBoc");
                return trySendBocImpl();
            });

        return trySendBocImpl();
    }

    private async sendBocImpl(
        bocB64: string,
        timeoutMs = 15_000,
    ): Promise<void> {
        const tonCenterUrl = this.parameters.endpoint;
        const url = new URL("sendBoc", tonCenterUrl);

        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), timeoutMs);

        try {
            const headers: Record<string, string> = {
                "accept": "application/json",
                "content-type": "application/json",
            };

            if (this.parameters.apiKey) {
                headers["X-API-Key"] = this.parameters.apiKey;
            }

            const res = await fetch(url, {
                method: "POST",
                headers,
                body: JSON.stringify({ boc: bocB64 }),
                signal: controller.signal,
            });

            if (!res.ok) {
                const text = await res.text().catch(() => "");
                throw new Error(
                    `HTTP ${res.status} ${res.statusText}${text ? `: ${text}` : ""}`
                );
            }
        } catch (err: unknown) {
            if (err instanceof Error && err.name === "AbortError") {
                throw new Error(`Request timed out after ${timeoutMs} ms`);
            }
            throw err;
        } finally {
            clearTimeout(timeout);
        }
    }

    public async runGetMethod(address: Address, name: string, stack?: TupleItem[]): Promise<TupleReader | Number> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("runGetMethod");

        const tryRunGetMethod = (): Promise<TupleReader | Number> => this.tonClient.runMethod(address, name, stack)
            .then(v => {
                tm.stop(tmId);
                return v.stack;
            })
            .catch(async (err) => {
                // Workaround (Got exit_code: ...)
                {
                    const extractCode = (input: string): Number | null => {
                        const m = /Got exit_code:\s*(-?\d+)/.exec(input);
                        if (!m) {
                            return null;
                        }

                        const n = Number(m[1]);
                        return Number.isInteger(n) ? n : null;
                    }

                    const exitCode = extractCode(err.toString());

                    if (exitCode != null) {
                        tm.stop(tmId);
                        return exitCode;
                    }
                }

                tm.stopErr(tmId);
                logAxiosError(err, "runGetMethod");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("runGetMethod");
                return tryRunGetMethod();
            });

        return tryRunGetMethod();
    }

    public open<T extends Contract>(contract: T): OpenedContract<T> {
        return this.tonClient.open(contract);
    }

    public async getBalance(address: Address): Promise<bigint> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getBalance");

        const tryGetBalance = (): Promise<bigint> => this.tonClient.getBalance(address)
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "getBalance");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("getBalance");
                return tryGetBalance();
            });

        return tryGetBalance();
    }

    public async getTransactions(address: Address, opts: {
        limit: number;
        lt?: string;
        hash?: string;
        to_lt?: string;
        inclusive?: boolean;
        archival?: boolean;
    }): Promise<Transaction[]> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getTransactions");

        const tryGetTransactions = (): Promise<Transaction[]> => this.tonClient.getTransactions(address, opts)
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "getTransactions");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("getTransactions");
                return tryGetTransactions();
            });

        return tryGetTransactions();
    }

    public async getTransaction(address: Address, lt: string, hash: string): Promise<Transaction | null> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getTransaction");

        const tryGetTransaction = (): Promise<Transaction | null> => this.tonClient.getTransaction(address, lt, hash)
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "getTransaction");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("getTransaction");
                return tryGetTransaction();
            });

        return tryGetTransaction();
    }

    public async lookupBlock({
        workchain,
        shard,
        lt,
        timeoutMs = 15_000,
    }: LookupBlockParams
    ): Promise<LookupBlockResponseOk> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("lookupBlock");

        const tryLookupBlockImpl = (): Promise<LookupBlockResponseOk> => this.lookupBlockImpl({ workchain, shard, lt, timeoutMs })
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "lookupBlock");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("lookupBlock");
                return tryLookupBlockImpl();
            });

        return tryLookupBlockImpl();
    }

    private async lookupBlockImpl({
        workchain,
        shard,
        lt,
        timeoutMs = 15_000,
    }: LookupBlockParams): Promise<LookupBlockResponseOk> {
        let tonCenterUrl = this.parameters.endpoint;
        const url = new URL("/lookupBlock", tonCenterUrl);
        url.searchParams.set("workchain", String(workchain));
        url.searchParams.set("shard", String(shard));
        url.searchParams.set("lt", String(lt));

        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), timeoutMs);

        try {
            const headers: Record<string, string> = {
                "accept": "application/json"
            };

            if (this.parameters.apiKey) {
                headers["X-API-Key"] = this.parameters.apiKey;
            }

            const res = await fetch(url, {
                method: "GET",
                headers: headers,
                signal: controller.signal,
            });


            if (!res.ok) {
                const text = await res.text().catch(() => "");
                throw new Error(`HTTP ${res.status} ${res.statusText}${text ? `: ${text}` : ""}`);
            }

            const data = (await res.json()) as LookupBlockResponseOk;

            if (data?.ok && data.result && typeof data.result.shard !== "string") {
                data.result.shard = String(data.result.shard);
            }

            return data;
        } finally {
            clearTimeout(timeout);
        }
    }

    public async getShardBlockProof({
        workchain,
        shard,
        seqno,
        from_seqno,
        timeoutMs = 15_000,
    }: GetShardBlockProofParams
    ): Promise<GetShardBlockProofResponseOk> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getShardBlockProof");

        const tryGetShardBlockProofImpl = (): Promise<GetShardBlockProofResponseOk> => this.getShardBlockProofImpl({ workchain, shard, seqno, from_seqno, timeoutMs })
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "getShardBlockProof");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("getShardBlockProof");
                return tryGetShardBlockProofImpl();
            });

        return tryGetShardBlockProofImpl();
    }

    private async getShardBlockProofImpl({
        workchain,
        shard,
        seqno,
        from_seqno,
        timeoutMs = 15_000,
    }: GetShardBlockProofParams): Promise<GetShardBlockProofResponseOk> {
        const tonCenterUrl = this.parameters.endpoint;
        const url = new URL("/getShardBlockProof", tonCenterUrl);
        url.searchParams.set("workchain", String(workchain));
        url.searchParams.set("shard", String(shard));
        url.searchParams.set("seqno", String(seqno));
        if (from_seqno !== undefined) url.searchParams.set("from_seqno", String(from_seqno));

        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), timeoutMs);

        try {
            const headers: Record<string, string> = {
                "accept": "application/json"
            };

            if (this.parameters.apiKey) {
                headers["X-API-Key"] = this.parameters.apiKey;
            }

            const res = await fetch(url, {
                method: "GET",
                headers: headers,
                signal: controller.signal,
            });

            if (!res.ok) {
                const text = await res.text().catch(() => "");
                throw new Error(`HTTP ${res.status} ${res.statusText}${text ? `: ${text}` : ""}`);
            }

            const data = (await res.json()) as GetShardBlockProofResponseOk;

            if (data?.ok && data.result) {
                const r = data.result as ShardBlockProof;
                TonCenterClient.normalizeBlockIdShard(r.from as TonBlockIdExt | undefined);
                TonCenterClient.normalizeBlockIdShard(r.mc_id as TonBlockIdExt | undefined);

                const links = (r as any).links as any[] | undefined;
                if (Array.isArray(links)) {
                    for (const link of links) {
                        TonCenterClient.normalizeBlockIdShard(link?.to as TonBlockIdExt | undefined);
                        TonCenterClient.normalizeBlockIdShard(link?.from as TonBlockIdExt | undefined);
                    }
                }
            }

            return data;
        } finally {
            clearTimeout(timeout);
        }
    }

    public async getBlockHeader({
        workchain,
        shard,
        seqno,
        root_hash,
        file_hash,
        timeoutMs = 15_000,
    }: GetBlockHeaderParams): Promise<GetBlockHeaderResponseOk> {
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getBlockHeader");

        const tryGetBlockHeaderImpl = (): Promise<GetBlockHeaderResponseOk> => this.getBlockHeaderImpl({
            workchain,
            shard,
            seqno,
            root_hash,
            file_hash,
            timeoutMs
        })
            .then(v => {
                tm.stop(tmId);
                return v;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "getBlockHeader");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("getBlockHeader");
                return tryGetBlockHeaderImpl();
            });

        return tryGetBlockHeaderImpl();
    }

    private async getBlockHeaderImpl({
        workchain,
        shard,
        seqno,
        root_hash,
        file_hash,
        timeoutMs = 15_000,
    }: GetBlockHeaderParams): Promise<GetBlockHeaderResponseOk> {
        const tonCenterUrl = this.parameters.endpoint;
        const url = new URL("/getBlockHeader", tonCenterUrl);
        url.searchParams.set("workchain", String(workchain));
        url.searchParams.set("shard", String(shard));
        url.searchParams.set("seqno", String(seqno));
        if (root_hash) url.searchParams.set("root_hash", root_hash);
        if (file_hash) url.searchParams.set("file_hash", file_hash);

        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), timeoutMs);

        try {
            const headers: Record<string, string> = {
                "accept": "application/json"
            };

            if (this.parameters.apiKey) {
                headers["X-API-Key"] = this.parameters.apiKey;
            }

            const res = await fetch(url, {
                method: "GET",
                headers: headers,
                signal: controller.signal,
            });

            if (!res.ok) {
                const text = await res.text().catch(() => "");
                throw new Error(`HTTP ${res.status} ${res.statusText}${text ? `: ${text}` : ""}`);
            }

            const data = (await res.json()) as GetBlockHeaderResponseOk;

            if (data?.ok && data.result?.id) {
                TonCenterClient.normalizeBlockIdShard(data.result.id);
            }

            return data;
        } finally {
            clearTimeout(timeout);
        }
    }

    private static normalizeBlockIdShard(b?: Partial<TonBlockIdExt>) {
        if (b && typeof b.shard !== "string") {
            (b as TonBlockIdExt).shard = String(b.shard);
        }
    }
}

// curl --parallel --parallel-max 30 -sS -X POST 'http://127.0.0.1:8081/jsonRPC' -H 'Content-Type: application/json' -d '{"id":"1","jsonrpc":"2.0","method":"runGetMethod","params":{"address":"EQDKES-w_JJD6_NlPLmByWuognx5kTOyHX6RPAzrFPCeXn5K","method":"seqno","stack":[]}}' --config <(awk '{print "url = " $0}' urls.txt)