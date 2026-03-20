import * as readline from "readline";

export function toHexPrefixed(n: bigint): string {
    const neg = n < 0n;
    const absHex = (neg ? -n : n).toString(16);
    return (neg ? "-0x" : "0x") + absHex;
}

export function base64toHexStr(b64: string): string {
    const bytes = Buffer.from(b64, "base64");
    return bytes.toString("hex");
}

export function hexToBase64Str(hex: string): string {
    const bytes = Buffer.from(hex, "hex");
    return bytes.toString("base64");
}

export function toJettons(value: number, decimals: number): bigint {
    return BigInt(Math.floor(value * 10 ** decimals));
}

export function fromTons(value: bigint): string {
    return fromJettons(value, 9);
}

export function fromJettons(value: bigint, decimals: number): string {
    return (Number(value) / Number((10n ** BigInt(decimals)))).toFixed(5);
}

export function shuffle<T>(arr: T[], rng: () => number = Math.random): T[] {
    for (let i = arr.length - 1; i > 0; i--) {
        const j = Math.floor(rng() * (i + 1));
        [arr[i], arr[j]] = [arr[j], arr[i]];
    }
    return arr;
}

export const checkEnvs = (names?: string[]) => {
    const envs = names ?? [
        "NETWORK",
        "API_BATCH_SIZE",
        "WORKCHAIN",
        "WALLET_ID",
        "MASTER_WALLET_VERSION",
        "MASTER_WALLET_KEY",
        "FAUCET_WALLET_VERSION",
        "FAUCET_WALLET_MNEMONIC",
        "API_ENDPOINTS",
    ];
    let throwError = false;
    for (const env of envs) {
        if (!process.env[env]) {
            throwError = true;
            console.error(`${env} is not set`);
        }
    }
    if (throwError) {
        throw new Error("Some environment variables are not set");
    }
    console.log("All environment variables are set");
}

export function input(prompt: string): Promise<string> {
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });
    return new Promise((resolve) => {
      rl.question(prompt, (answer) => {
        rl.close();
        resolve(answer);
      });
    });
  }