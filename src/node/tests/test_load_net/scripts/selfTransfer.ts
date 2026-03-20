import { toNano } from "@ton/core";
import { AppGlobals } from "./globals";
import { fromTons } from "./utils";
import { Wallet, WalletVersionUtils } from "./wallet";

const DEFAULT_TON_AMOUNT = "0.05";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing ${name} in environment`);
  }
  return value;
}

function setDefaultEnv(name: string, value: string): void {
  if (!process.env[name]) {
    process.env[name] = value;
  }
}

export async function run() {
  const mnemonic = requireEnv("WALLET_MNEMONIC");
  const walletVersionStr = requireEnv("WALLET_VERSION");
  requireEnv("API_ENDPOINTS");
  requireEnv("WORKCHAIN");
  requireEnv("WALLET_ID");

  setDefaultEnv("API_BATCH_SIZE", "1");
  setDefaultEnv("NETWORK", "self");
  setDefaultEnv("MASTER_WALLET_VERSION", walletVersionStr);
  setDefaultEnv("FAUCET_WALLET_VERSION", walletVersionStr);
  setDefaultEnv("FAUCET_WALLET_MNEMONIC", mnemonic);

  const walletVersion = WalletVersionUtils.fromString(walletVersionStr);
  const wallet = await Wallet.fromMnemonic(mnemonic, walletVersion, undefined, undefined);

  setDefaultEnv("MASTER_WALLET_KEY", wallet.getKeypairAsHex());

  await AppGlobals.S();

  const tonAmountStr = process.env.TON_AMOUNT ?? DEFAULT_TON_AMOUNT;
  const tonAmount = Number.parseFloat(tonAmountStr);
  if (!Number.isFinite(tonAmount) || tonAmount <= 0) {
    throw new Error(`Invalid TON_AMOUNT: ${tonAmountStr}`);
  }

  console.log(`wallet address: ${wallet.getAddress()}`);

  console.log(`updateSeqno...`);
  await wallet.updateSeqno();
  console.log(`seqno: ${wallet.getSeqno()}`);

  console.log(`updateBalance...`);
  await wallet.updateBalance();
  console.log(`balance: ${fromTons(wallet.getBalance())} TON`);

  console.log(`send ${tonAmount} TON to self...`);
  const seqnoBeforeSend = wallet.getSeqno();
  const msg = await wallet.sendTon(wallet.getAddress(), toNano(tonAmountStr), undefined);
  console.log(`external msg id: ${Wallet.messageIdToHex(msg)}`);

  console.log(`wait for seqno change...`);
  await wallet.waitForSeqNoChange(seqnoBeforeSend);

  console.log(`updateBalance...`);
  await wallet.waitForBalanceChange();
  console.log(`seqno: ${wallet.getSeqno()}`);
  console.log(`balance: ${fromTons(wallet.getBalance())} TON`);

  console.log(`Done`);
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
