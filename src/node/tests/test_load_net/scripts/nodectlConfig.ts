import { keyPairFromSecretKey, keyPairFromSeed } from "@ton/crypto";
import { WalletContractV3R2, WalletContractV4, WalletContractV5R1 } from "@ton/ton";
import { exportPublicKeyFromVault } from "./vault";

type SupportedWalletVersion = "V3R2" | "V4R2" | "V5R1";
type WalletContractAny = WalletContractV3R2 | WalletContractV4 | WalletContractV5R1;

type JsonMap = Record<string, unknown>;

export type NodectlWalletConfig = {
    key: unknown;
    version: string;
    subwallet_id?: number;
    workchain?: number;
};

export type NodectlConfig = {
    wallets?: Record<string, NodectlWalletConfig>;
    pools?: JsonMap;
    bindings?: JsonMap;
    elections?: JsonMap;
};

function normalizeVersion(version: string): SupportedWalletVersion {
    const normalized = version.trim().toUpperCase();
    switch (normalized) {
        case "V3R2":
            return "V3R2";
        case "V4":
        case "V4R2":
            return "V4R2";
        case "V5":
        case "V5R1":
            return "V5R1";
        default:
            throw new Error(`Unsupported wallet version: ${version}`);
    }
}

function getPublicKeyFromHex(hexValue: string): Buffer {
    const normalized = hexValue.startsWith("0x") ? hexValue.slice(2) : hexValue;
    const keyBytes = Buffer.from(normalized, "hex");
    if (keyBytes.length === 64) {
        return keyBytes.subarray(32);
    }
    if (keyBytes.length === 32) {
        return keyBytes;
    }

    throw new Error(`Invalid key length in hex format: ${keyBytes.length} bytes`);
}

function getPublicKeyFromPrivateBase64(privateKeyBase64: string): Buffer {
    const privateKey = Buffer.from(privateKeyBase64, "base64");

    if (privateKey.length === 64) {
        return keyPairFromSecretKey(privateKey).publicKey;
    }
    if (privateKey.length === 32) {
        return keyPairFromSeed(privateKey).publicKey;
    }

    throw new Error(`Invalid private key length: ${privateKey.length} bytes`);
}

async function resolveWalletPublicKey(walletConfig: NodectlWalletConfig): Promise<Buffer> {
    const key = walletConfig.key;
    if (typeof key === "string") {
        return getPublicKeyFromHex(key);
    }

    if (typeof key !== "object" || key === null) {
        throw new Error(`Unsupported key config format: ${String(key)}`);
    }

    const keyObj = key as JsonMap;
    const vaultName = keyObj.name ?? keyObj.secret_id;
    if (typeof vaultName === "string") {
        console.log(`Exporting wallet public key from Vault secret ${vaultName}...`);
        return exportPublicKeyFromVault(vaultName);
    }

    if (typeof keyObj.pub_key === "string") {
        const publicKey = Buffer.from(keyObj.pub_key, "base64");
        if (publicKey.length !== 32) {
            throw new Error(`Invalid public key length: ${publicKey.length} bytes`);
        }
        return publicKey;
    }

    if (typeof keyObj.pvt_key === "string") {
        return getPublicKeyFromPrivateBase64(keyObj.pvt_key);
    }

    throw new Error(`Unsupported wallet key object format: ${JSON.stringify(keyObj)}`);
}

export async function createWalletContract(
    walletConfig: NodectlWalletConfig,
): Promise<WalletContractAny> {
    const publicKey = await resolveWalletPublicKey(walletConfig);
    const version = normalizeVersion(walletConfig.version);
    const workchain = walletConfig.workchain ?? -1;
    const subwalletId = walletConfig.subwallet_id ?? 42;

    switch (version) {
        case "V3R2":
            return WalletContractV3R2.create({
                workchain,
                publicKey,
                walletId: subwalletId,
            });
        case "V4R2":
            return WalletContractV4.create({
                workchain,
                publicKey,
                walletId: subwalletId,
            });
        case "V5R1":
            return WalletContractV5R1.create({
                workchain,
                publicKey,
                walletId: { networkGlobalId: 0, context: subwalletId },
            });
        default:
            throw new Error(`Unsupported wallet version: ${walletConfig.version}`);
    }
}

function readPoolAddress(value: unknown): string | null {
    if (typeof value === "string") {
        return value;
    }

    if (typeof value !== "object" || value === null) {
        return null;
    }

    const obj = value as JsonMap;

    if (typeof obj.address === "string") {
        return obj.address;
    }

    if (Array.isArray(obj.addresses)) {
        const first = obj.addresses.at(0);
        if (typeof first === "string") {
            return first;
        }
    }

    return null;
}

export type PoolTarget = {
    name: string;
    address: string;
};

export function collectPoolTargets(config: NodectlConfig): PoolTarget[] {
    const targets: PoolTarget[] = [];
    const seenAddresses = new Set<string>();
    const addTarget = (name: string, address: string | null) => {
        if (!address) {
            return;
        }
        if (seenAddresses.has(address)) {
            return;
        }
        seenAddresses.add(address);
        targets.push({ name, address });
    };

    const elections = config.elections;
    if (typeof elections === "object" && elections !== null) {
        const legacyPools = (elections as JsonMap).pools;
        if (typeof legacyPools === "object" && legacyPools !== null) {
            for (const [nodeName, value] of Object.entries(legacyPools as JsonMap)) {
                addTarget(nodeName, readPoolAddress(value));
            }
        }
    }

    const pools = config.pools;
    const bindings = config.bindings;
    if (typeof pools === "object" && pools !== null) {
        if (typeof bindings === "object" && bindings !== null) {
            for (const [nodeName, binding] of Object.entries(bindings as JsonMap)) {
                if (typeof binding !== "object" || binding === null) {
                    continue;
                }
                const poolName = (binding as JsonMap).pool;
                if (typeof poolName !== "string") {
                    continue;
                }
                const poolConfig = (pools as JsonMap)[poolName];
                addTarget(nodeName, readPoolAddress(poolConfig));
            }
        }

        for (const [poolName, poolConfig] of Object.entries(pools as JsonMap)) {
            addTarget(poolName, readPoolAddress(poolConfig));
        }
    }

    return targets;
}
