import { KeyPair, keyPairFromSecretKey, mnemonicToWalletKey } from "@ton/crypto";
import { WalletContractV3R2, WalletContractV4, WalletContractV5R1 } from "@ton/ton";
import * as bip39 from "bip39";
import { WalletVersion } from "./wallet";

export class WalletContractFactory {
    private static async getKeyPairFromMnemonic(mnemonic?: string): Promise<KeyPair> {
        if (!mnemonic) {
            throw new Error("Mnemonic is empty");
        }
        return await mnemonicToWalletKey(mnemonic.split(" "));
    }

    private static getKeyPairFromSecretKeyHex(secretHex: string): KeyPair {
        return keyPairFromSecretKey(Buffer.from(secretHex, "hex"));
    }

    public static generateMnemonic(): string {
        return bip39.generateMnemonic(256);
    }

    public static createFromSecretKey(
        keyPair: KeyPair,
        version: WalletVersion,
        workchain: number,
        walletId: number,
    ): {
        contract: WalletContractV3R2 | WalletContractV4 | WalletContractV5R1,
        keypair: KeyPair
    } {
        switch (version) {
            case WalletVersion.V3R2:
                return WalletContractFactory.createWalletV3R2(keyPair, workchain, walletId);
            case WalletVersion.V4R2:
                return WalletContractFactory.createWalletV4(keyPair, workchain, walletId);
            case WalletVersion.V5R1:
                return WalletContractFactory.createWalletV5R1(keyPair, workchain, walletId);
            default:
                throw new Error(`Unsupported version: ${version}`);
        }
    }

    public static createFromSecretKeyHex(
        secretHex: string,
        version: WalletVersion,
        workchain: number,
        walletId: number,
    ): {
        contract: WalletContractV3R2 | WalletContractV4 | WalletContractV5R1,
        keypair: KeyPair
    } {
        return WalletContractFactory.createFromSecretKey(
            WalletContractFactory.getKeyPairFromSecretKeyHex(secretHex),
            version,
            workchain,
            walletId
        );
    }

    public static async createFromMnemonic(
        mnemonic: string,
        version: WalletVersion,
        workchain: number,
        walletId: number,
    ): Promise<{
        contract: WalletContractV3R2 | WalletContractV4 | WalletContractV5R1,
        keypair: KeyPair
    }> {
        return WalletContractFactory.createFromSecretKey(
            await WalletContractFactory.getKeyPairFromMnemonic(mnemonic),
            version,
            workchain,
            walletId
        );
    }

    public static createWalletV3R2(
        keyPair: KeyPair,
        workchain: number,
        walletId: number,
    ): {
        contract: WalletContractV3R2,
        keypair: KeyPair
    } {
        return {
            contract: WalletContractV3R2.create({
                workchain: workchain,
                publicKey: keyPair.publicKey,
                walletId: walletId
            }),
            keypair: keyPair
        };
    }

    public static createWalletV4(
        keyPair: KeyPair,
        workchain: number,
        walletId: number,
    ): {
        contract: WalletContractV4,
        keypair: KeyPair
    } {
        return {
            contract: WalletContractV4.create({
                workchain: workchain,
                publicKey: keyPair.publicKey,
                walletId: walletId,
            }),
            keypair: keyPair
        };
    }

    public static createWalletV5R1(
        keyPair: KeyPair,
        workchain: number,
        walletId: number,
    ): {
        contract: WalletContractV5R1,
        keypair: KeyPair
    } {
        return {
            contract: WalletContractV5R1.create({
                workchain: workchain,
                publicKey: keyPair.publicKey,
                walletId: { networkGlobalId: 0, context: walletId }
            }),
            keypair: keyPair
        };
    }
}
