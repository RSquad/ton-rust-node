import { AppGlobals } from "./globals";
import { fromNano, toNano } from "@ton/core";
import { checkEnvs, input } from "./utils";
import * as fs from "fs";
import { createWalletContract, NodectlConfig } from "./nodectlConfig";

async function run() {
  const master = (await AppGlobals.S()).getMasterWallet();
  await master.updateSeqno();
  console.log(`Master wallet address: ${master.getAddress()}`);

  const envPath = process.env.NODECTL_CONFIG_PATH;
  const configPath = envPath ?? await input("Enter nodectl config file path: ");
  const config = JSON.parse(fs.readFileSync(configPath, "utf8")) as NodectlConfig;
  const walletsConfig = config.wallets ?? {};
  console.log(`Total wallets: ${Object.keys(walletsConfig).length}`);
  const envAmount = process.env.NODECTL_INITIAL_BALANCE;
  const initialBalance = envAmount ? toNano(envAmount) : toNano(await input("Initial wallet balance (TON): "));
  console.log(`Initial balance: ${fromNano(initialBalance)} TON`);
  
  const wallets: Record<string, string> = {};
  for (let nodeName of Object.keys(walletsConfig)) {
    const walletConfig = walletsConfig[nodeName];
    const wallet = await createWalletContract(walletConfig);
    console.log(`Deploying wallet '${wallet.address.toRawString()}' for node '${nodeName}' (${walletConfig.version})`);
    let seqno = master.getSeqno();
    await master.sendTon(wallet.address, initialBalance, wallet.init);
    console.log(`Wait for transfer...`);
    await master.waitForSeqNoChange(seqno, 100);
    wallets[nodeName] = wallet.address.toRawString();
  }

  console.log(`Done. Wallets:\n${JSON.stringify(wallets, null, 2)}`);
}

(async () => {
  checkEnvs();
  await run();
})();
