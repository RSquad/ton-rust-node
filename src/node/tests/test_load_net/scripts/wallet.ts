import {
    Address, beginCell, Cell, internal, Message, MessageRelaxed, OpenedContract, Sender, SendMode, StateInit,
    storeMessage, storeMessageRelaxed, toNano, TupleReader, WalletContractV3R2, WalletContractV4, WalletContractV5R1
} from "@ton/ton";
import { KeyPair } from "@ton/crypto";
import { WalletContractFactory } from "./walletContractFactory";
import { AppGlobals } from "./globals";
import { sleep } from "@ton/blueprint";

export enum WalletVersion {
    V3R2 = "V3R2",
    V4R2 = "V4R2",
    V5R1 = "V5R1"
}

export class WalletVersionUtils {
    public static fromString(value: string): WalletVersion {
        switch (value.trim().toUpperCase()) {
            case "V3R2":
                return WalletVersion.V3R2;
            case "V4":
            case "V4R2":
                return WalletVersion.V4R2;
            case "V5":
            case "V5R1":
                return WalletVersion.V5R1;
            default:
                throw new Error(`Invalid wallet version string: ${value}`);
        }
    }
}

export type ShardInfo = {
    workchain: number;
    shardId: bigint;
    shardIdInt: bigint;
    depth: number
}

type AnyCreateTransfer = (args: {
    seqno: number;
    secretKey: Uint8Array;
    messages: MessageRelaxed[];
    sendMode?: SendMode;
}) => Promise<Cell> | Cell;

export class Wallet {
    private readonly version: WalletVersion;
    private readonly contract: WalletContractV3R2 | WalletContractV4 | WalletContractV5R1;
    private readonly keypair: KeyPair;
    private readonly workchain: number;
    private readonly walletId: number;

    private seqno: number = 0;
    private openedContract?: OpenedContract<WalletContractV3R2 | WalletContractV4 | WalletContractV5R1>;
    private jettonMintersAddr: Map<string, Address> = new Map();
    private jettonWalletsAddr: Map<string, Address> = new Map();
    private jettonBalances: Map<string, bigint> = new Map();
    private balance: bigint = 0n;

    public static async fromMnemonic(
        mnemonic: string,
        version: WalletVersion,
        workchain: number | undefined,
        walletId: number | undefined,
    ): Promise<Wallet> {
        let { contract, keypair } = await WalletContractFactory.createFromMnemonic(
            mnemonic,
            version,
            workchain ?? Number.parseInt(process.env.WORKCHAIN!),
            walletId ?? Number.parseInt(process.env.WALLET_ID!),
        );

        return new Wallet(
            version,
            contract,
            keypair,
            workchain ?? Number.parseInt(process.env.WORKCHAIN!),
            walletId ?? Number.parseInt(process.env.WALLET_ID!)
        );
    }

    public static fromSecretKeyHex(
        secretHex: string,
        version: WalletVersion,
        workchain: number | undefined,
        walletId: number | undefined,
    ): Wallet {
        let { contract, keypair } = WalletContractFactory.createFromSecretKeyHex(
            secretHex,
            version,
            workchain ?? Number.parseInt(process.env.WORKCHAIN!),
            walletId ?? Number.parseInt(process.env.WALLET_ID!),
        );

        return new Wallet(
            version,
            contract,
            keypair,
            workchain ?? Number.parseInt(process.env.WORKCHAIN!),
            walletId ?? Number.parseInt(process.env.WALLET_ID!)
        );
    }

    constructor(
        version: WalletVersion,
        contract: WalletContractV3R2 | WalletContractV4 | WalletContractV5R1,
        keypair: KeyPair,
        workchain: number | undefined,
        walletId: number | undefined,
    ) {
        this.version = version;
        this.contract = contract;
        this.keypair = keypair;
        this.workchain = workchain ?? Number.parseInt(process.env.WORKCHAIN!);
        this.walletId = walletId ?? Number.parseInt(process.env.WALLET_ID!);
    }

    public getVersion(): WalletVersion {
        return this.version;
    }

    public getKeypair(): KeyPair {
        return this.keypair;
    }

    public getKeypairAsHex(): string {
        return this.keypair.secretKey.toString("hex");
    }

    public getWorkchain(): number {
        return this.workchain;
    }

    public getWalletId(): number | undefined {
        return this.walletId;
    }

    public getAddressStr(args?: {
        urlSafe?: boolean;
        bounceable?: boolean;
        testOnly?: boolean;
    }): string {
        return this.contract.address.toString(args);
    }

    public getAddress(): Address {
        return this.contract.address;
    }

    public getSeqno(): number {
        return this.seqno;
    }

    public setSeqno(seqno: number) {
        return this.seqno = seqno;
    }

    public incSeqno() {
        this.seqno++;
    }

    public async updateSeqno(): Promise<number> {
        let tonClient = (await AppGlobals.S()).nextTonCenterClient();
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getSeqno");

        return tonClient.runGetMethod(this.getAddress(), "seqno", [])
            .then(v => {
                if (typeof v === "number") {
                    if (v == -13) {
                        this.seqno = 0;
                    } else {
                        throw Error(`Exit code: ${v}`);
                    }
                } else if (v instanceof TupleReader) {
                    this.seqno = v.readNumber();
                } else {
                    throw Error(`Unknown type: ${v}`);
                }

                tm.stop(tmId);
                return this.seqno;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                throw err;
            });
    }

    public async waitForSeqNoChange(seqno: number, sleepPeriod: number = 250): Promise<void> {
        while (seqno === this.getSeqno()) {
            await sleep(sleepPeriod);
            await this.updateSeqno();
        }
    }

    public getBalance(): bigint {
        return this.balance;
    }

    public async updateBalance(): Promise<bigint> {
        let tonClient = (await AppGlobals.S()).nextTonCenterClient();
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("getBalance");

        return tonClient.getBalance(this.getAddress())
            .then(v => {
                tm.stop(tmId);
                this.balance = v;
                return v;
            })
            .catch(err => {
                tm.stopErr(tmId);
                throw err;
            });
    }

    public async waitForBalanceChange(sleepPeriod: number = 250): Promise<void> {
        const currentBalance = this.getBalance();
        while (currentBalance === this.getBalance()) {
            await sleep(sleepPeriod);
            await this.updateBalance();
        }
    }

    public async getOpenedContract(): Promise<OpenedContract<WalletContractV3R2 | WalletContractV4 | WalletContractV5R1>> {
        if (this.openedContract === undefined) {
            let tonClient = (await AppGlobals.S()).nextTonCenterClient();
            this.openedContract = tonClient.open(this.contract);
        }

        return this.openedContract!;
    }

    public async getSender(): Promise<Sender> {
        let openedContract = await this.getOpenedContract();
        return openedContract.sender(this.keypair.secretKey);
    }

    public getStateInit(): StateInit {
        const { data, code } = this.contract.init;
        let stateInit: StateInit = {
            code: code,
            data: data,
        };

        return stateInit;
    }

    public async send(
        messages: MessageRelaxed[],
        sendMode: SendMode,
    ): Promise<Message> {
        // externalInMsg
        let externalInMsg: Message;
        {
            const validUntil = Math.floor(Date.now() / 1000) + 600; // +10 min

            const args = {
                seqno: this.getSeqno(),
                secretKey: this.keypair.secretKey,
                messages: messages,
                sendMode: sendMode,
                validUntil: validUntil,
            };
            let openedContract = await this.getOpenedContract();
            const create = (openedContract.createTransfer as unknown as AnyCreateTransfer).bind(openedContract);
            const extMsgBody = await Promise.resolve(create(args));

            externalInMsg = {
                info: {
                    type: 'external-in',
                    dest: this.getAddress(),
                    importFee: 0n,//toNano(0.01)
                },
                init: undefined,
                body: extMsgBody,
            };
        }

        // Send
        let tonClient = (await AppGlobals.S()).nextTonCenterClient();
        return tonClient.sendBoc(Wallet.extInMessageToBase64(externalInMsg)).then(v => {
            return externalInMsg;
        });

        /*
        const openedContract = await this.getOpenedContract();

        const validUntil = Math.floor(Date.now() / 1000) + 600; // +10 min

        const args = {
            seqno: this.getSeqno(),
            secretKey: this.keypair.secretKey,
            messages: messages,
            sendMode: sendMode,
            validUntil: validUntil,
        };

        const create = (openedContract.createTransfer as unknown as AnyCreateTransfer).bind(openedContract);
        const extMsgBody = await Promise.resolve(create(args));
        let extInMsg: Message = {
            info: {
                type: 'external-in',
                dest: this.getAddress(),
                importFee: 0n,
            },
            init: undefined,
            body: extMsgBody,
        };

        // Send
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("send");

        const trySend = (): Promise<Message> => openedContract.send(extMsgBody)
            .then(() => {
                tm.stop(tmId);
                return extInMsg;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                logAxiosError(err, "send");
                await new Promise(r => setTimeout(r, 50));
                tmId = tm.start("send");
                return trySend();
            });

        return trySend();
        */
    }

    public async sendTon(
        toAddress: Address,
        amountNano: bigint,
        stateInit: StateInit | undefined,
    ): Promise<Message> {
        let messages = [internal({
            to: toAddress,
            value: amountNano,
            bounce: false,
            init: stateInit
        })];

        return this.send(
            messages,
            SendMode.PAY_GAS_SEPARATELY,
        );
    }

    // ---------------------------------------- JETTONS ---------------------------------------------
    public getJettonMinterAddr(jettonName: string): Address | undefined {
        return this.jettonMintersAddr.get(jettonName);
    }

    public getJettonWalletAddr(jettonName: string): Address | undefined {
        return this.jettonWalletsAddr.get(jettonName);
    }

    public async setJettonMinter(jettonName: string, minterAddr: Address): Promise<void> {
        const existing = [...this.jettonMintersAddr.entries()].find(([, a]) => a.equals(minterAddr));

        if (existing) {
            const [key] = existing;
            if (key === jettonName) {
                return;
            }
            throw new Error(`Address is already used by jetton "${key}"`);
        }

        let tonClient = (await AppGlobals.S()).nextTonCenterClient();
        const res = await tonClient.runGetMethod(minterAddr, "get_wallet_address", [{ type: "slice", cell: beginCell().storeAddress(this.getAddress()).endCell() }]);

        if (typeof res === "number") {
            throw Error(`Exit code: ${res}`);
        }

        const cell = (res as TupleReader).readCell();
        const jettonWalletAddr = cell.beginParse().loadAddress();

        this.jettonMintersAddr.set(jettonName, minterAddr);
        this.jettonWalletsAddr.set(jettonName, jettonWalletAddr);
    }

    public async sendJetton(
        jettonName: string,
        toOwnerAddress: Address, // jetton owner address
        amountNanoJettons: bigint,
        amountNanoForwardTon: bigint,
        amountNanoForFees: bigint,
        forwardComment: string,
    ): Promise<Message> {
        const JETTON_TRANSFER_OP = 0x0f8a7ea5 as const;

        // Jetton transfer payload (TEP-74)
        const forwardPayload = (amountNanoForwardTon > 0n)
            ? beginCell().storeUint(0, 32).storeStringTail(forwardComment).endCell()
            : null;

        const body = beginCell()
            .storeUint(JETTON_TRANSFER_OP, 32)
            .storeUint(0, 64)
            .storeCoins(amountNanoJettons)
            .storeAddress(toOwnerAddress)
            .storeAddress(this.getAddress())    // response_destination
            .storeMaybeRef(null)                // custom_payload
            .storeCoins(amountNanoForwardTon)
            .storeMaybeRef(forwardPayload)
            .endCell();

        let messages = [internal({
            to: this.getJettonWalletAddr(jettonName)!,
            value: amountNanoForFees,
            bounce: true,
            body,
        })];

        return this.send(
            messages,
            SendMode.PAY_GAS_SEPARATELY,
        );
    }

    public getJettonBalance(jettonName: string): bigint {
        const balance = this.jettonBalances.get(jettonName);
        return balance ?? 0n;
    }

    public async updateJettonBalance(jettonName: string): Promise<bigint> {
        let jettonWalletAddr = this.getJettonWalletAddr(jettonName);
        if (jettonWalletAddr == null) {
            throw Error(`No jetton '${jettonName}' wallet address found for '${this.getAddress()}'`);
        }

        let tonClient = (await AppGlobals.S()).nextTonCenterClient();
        let tm = (await AppGlobals.S()).getTimeMeasure();
        let tmId = tm.start("get_wallet_data");

        return tonClient.runGetMethod(jettonWalletAddr, "get_wallet_data", [])
            .then(v => {
                if (typeof v === "number") {
                    if (v == -13) {
                        this.jettonBalances.set(jettonName, 0n);
                    } else {
                        throw Error(`Exit code: ${v}`);
                    }
                } else if (v instanceof TupleReader) {
                    this.jettonBalances.set(jettonName, v.readBigNumber());
                } else {
                    throw Error(`Unknown type: ${v}`);
                }

                tm.stop(tmId);
                return this.jettonBalances.get(jettonName)!;
            })
            .catch(async (err) => {
                tm.stopErr(tmId);
                throw err;
            });
    }

    public async waitForJettonBalanceChange(jettonName: string, sleepPeriod: number = 250): Promise<void> {
        const currentBalance = this.getJettonBalance(jettonName);
        while (currentBalance === this.getJettonBalance(jettonName)) {
            await sleep(sleepPeriod);
            await this.updateJettonBalance(jettonName);
        }
    }

    // ---------------------------------------- Messages ---------------------------------------------
    public static messageIdToHex(msg: Message): string {
        const cell = beginCell().store(storeMessage(msg)).endCell();
        const h = cell.hash();
        return h.toString('hex');
    }

    public static messageRelaxedIdToHex(msg: MessageRelaxed): string {
        const cell = beginCell().store(storeMessageRelaxed(msg)).endCell();
        const h = cell.hash();
        return h.toString('hex');
    }

    public static extInMessageToBase64(extInMsg: Message): string {
        const extInMsgCell = beginCell().store(storeMessage(extInMsg)).endCell();
        const bocBase64 = Buffer.from(extInMsgCell.toBoc({ idx: false, crc32: true })).toString('base64');
        return bocBase64;
    }

    public static shardInfoForAddr(
        addr: Address,
        shardsCount: number
    ): ShardInfo {
        if (shardsCount <= 0 || (shardsCount & (shardsCount - 1)) !== 0) {
            throw new Error('shardsCount must be a positive power of two');
        }

        let depth = BigInt(Math.log2(shardsCount));
        if (!Number.isInteger(Number(depth))) {
            throw new Error('shardsCount must be a power of two');
        }

        if (depth < 0n || depth > 60n) {
            throw new Error('depth must be between 0 and 60 for TON shards');
        }

        const accountHash = addr.hash;
        if (!accountHash || accountHash.length !== 32) {
            throw new Error('Invalid address: missing 32-byte account hash');
        }

        // Get first 8 bytes
        let acc = 0n;
        for (let i = 0; i < 8; i++) {
            acc = (acc << 8n) | BigInt(accountHash[i]);
        }

        const mask = ((1n << depth) - 1n) << (64n - depth);
        let shardId = acc & mask | (1n << (64n - depth - 1n));

        // Convert to signed 64-bit integer (two's complement)
        let shardIdInt = shardId;
        if (shardIdInt >= (1n << 63n)) {
            shardIdInt = shardIdInt - (1n << 64n);
        }

        return {
            workchain: addr.workChain,
            shardId: shardId,
            shardIdInt: shardIdInt,
            depth: Number(depth),
        };
    }
}
