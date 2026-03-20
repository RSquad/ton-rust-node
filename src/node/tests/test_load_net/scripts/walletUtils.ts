import * as bip39 from "bip39";
import { Wallet, WalletVersion } from "./wallet";

export class WalletUtils {
    public static async generate(
        count: number,
        version: WalletVersion,
        workchain: number | undefined,
        walletId: number | undefined,
    ): Promise<Wallet[]> {
        let wallets: Wallet[] = [];

        for (let i = 0; i < count; i++) {
            let mnemonic = bip39.generateMnemonic(256);
            let wallet = await Wallet.fromMnemonic(
                mnemonic,
                version,
                workchain,
                walletId
            );

            wallets.push(wallet);
        }

        return wallets;
    }
}
