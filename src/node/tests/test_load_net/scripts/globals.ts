import { RoundRobinVec } from "./roudRobinVec";
import { Wallet, WalletVersionUtils } from "./wallet";
import { TonCenterClient } from "./tonCenterClient";
import { WalletStorage } from "./walletStorage";
import { TimeMeasure } from "./timeMeasure";

export class AppGlobals {
    private masterWallet: Wallet;
    private faucetWallet: Wallet;
    private tonClients: RoundRobinVec<TonCenterClient>;
    private walletStorage: WalletStorage;
    private timeMeasure: TimeMeasure;
    private apiBatchSize: number;

    constructor(
        masterWallet: Wallet,
        faucetWallet: Wallet,
    ) {
        this.masterWallet = masterWallet;
        this.faucetWallet = faucetWallet;
        this.tonClients = new RoundRobinVec(AppGlobals.makeTonCenterClients(process.env.API_ENDPOINTS!));
        this.walletStorage = new WalletStorage(`./${process.env.NETWORK}_wallets.csv`);
        this.timeMeasure = new TimeMeasure({ keepSamples: true, maxSamplesPerName: 10000 });
        this.apiBatchSize = Number.parseInt(process.env.API_BATCH_SIZE!);
    }

    public getMasterWallet(): Wallet {
        return this.masterWallet;
    }

    public getFaucetWallet(): Wallet {
        return this.faucetWallet;
    }

    public nextTonCenterClient(): TonCenterClient {
        return this.tonClients.next();
    }

    public getWalletStorage(): WalletStorage {
        return this.walletStorage;
    }

    public getTimeMeasure(): TimeMeasure {
        return this.timeMeasure;
    }

    public getApiBatchSize(): number {
        return this.apiBatchSize;
    }

    private static makeTonCenterClients(tonApiEndpoints: string): TonCenterClient[] {
        return tonApiEndpoints.split("|").map(endpoint => new TonCenterClient({
            endpoint: endpoint,
            apiKey: process.env.API_KEY,
        }));
    }

    private static INSTANCE?: AppGlobals;

    public static async S(): Promise<AppGlobals> {
        if (AppGlobals.INSTANCE != null) {
            return AppGlobals.INSTANCE;
        }

        let masterWallet = Wallet.fromSecretKeyHex(
            process.env.MASTER_WALLET_KEY!,
            WalletVersionUtils.fromString(process.env.MASTER_WALLET_VERSION!),
            -1,
            undefined
        );

        let faucetWallet = await Wallet.fromMnemonic(
            process.env.FAUCET_WALLET_MNEMONIC!,
            WalletVersionUtils.fromString(process.env.FAUCET_WALLET_VERSION!),
            0,
            undefined
        );

        AppGlobals.INSTANCE = new AppGlobals(
            masterWallet,
            faucetWallet
        );

        AppGlobals.INSTANCE.getWalletStorage().init();

        return AppGlobals.INSTANCE;
    }
};
