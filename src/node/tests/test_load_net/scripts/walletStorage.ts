import { promises as fs } from "node:fs";
import { dirname } from "node:path";
import { EOL } from "node:os";
import { Wallet, WalletVersionUtils } from "./wallet";
import { Address } from "@ton/core";

export type Options = {
    fsyncEveryN: number;            // fsync after N appends (default: 100; 0 = never)
};

export class WalletStorage {
    private readonly filePath: string;
    private readonly fsyncEvery: number;
    private fh?: fs.FileHandle;
    private tail: Promise<void> = Promise.resolve(); // serialize writes
    private sinceSync = 0;

    constructor(filePath: string, opts: Options = { fsyncEveryN: 100 }) {
        this.filePath = filePath;
        this.fsyncEvery = opts.fsyncEveryN ?? 0;
    }

    public async init(): Promise<void> {
        await fs.mkdir(dirname(this.filePath), { recursive: true });
        this.fh = await fs.open(this.filePath, "a");
    }

    public async append(wallets: Wallet[]): Promise<void> {
        const fh = this.fh;
        if (!fh) {
            throw new Error("init() not called or file already closed.");
        }

        if (wallets.length === 0) {
            return this.tail;
        }

        const lines = wallets.map(
            wallet => {
                const addr = wallet.getAddressStr();
                const keyhex = wallet.getKeypair().secretKey.toString("hex");
                const version = wallet.getVersion().toString();

                return `${addr}|${keyhex}|${version}`;
            }
        );
        const payload = lines.join(EOL) + EOL;

        this.tail = this.tail.then(async () => {
            await fh.appendFile(payload, "utf8");

            if (this.fsyncEvery > 0) {
                this.sinceSync += lines.length; // count items, not calls
                if (this.sinceSync >= this.fsyncEvery) {
                    await fh.sync();
                    this.sinceSync = 0;
                }
            }
        });

        return this.tail;
    }

    public async readAll(): Promise<Wallet[]> {
        await this.tail;
        if (this.fh && this.fsyncEvery === 0) {
            await this.fh.datasync?.().catch(() => { });
        }

        const content = await fs.readFile(this.filePath, "utf8").catch((e: any) => {
            if (e?.code === "ENOENT") {
                return "";
            }
            throw e;
        });

        const lines = content.split(/\r?\n/);
        if (lines.at(-1) === "") {
            lines.pop();
        }

        return WalletStorage.parseWallets(lines);
    }

    public async flush(): Promise<void> {
        await this.tail;
        if (this.fh) {
            await this.fh.sync();
        }
    }

    public async close(): Promise<void> {
        await this.flush();
        await this.fh?.close();
        this.fh = undefined;
    }

    private static async parseWallets(lines: string[]): Promise<Wallet[]> {
        let wallets: Wallet[] = [];

        for (const line of lines) {
            const parts = line.split("|");
            const address = Address.parse(parts[0]);
            const keyhex = parts[1];
            const walletVersion = WalletVersionUtils.fromString(parts[2]);

            let wallet = await Wallet.fromSecretKeyHex(
                keyhex,
                walletVersion,
                address.workChain,
                undefined,
            );

            wallets.push(wallet);
        }

        return wallets;
    }
}
