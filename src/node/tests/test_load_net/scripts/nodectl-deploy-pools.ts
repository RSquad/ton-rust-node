import { AppGlobals } from "./globals";
import { NOMINATOR_CODE_BOC, SingleNominator } from "../wrappers/SingleNominator";
import { Cell, toNano } from "@ton/core";
import { checkEnvs, input } from "./utils";
import * as fs from "fs";
import { createWalletContract, NodectlConfig } from "./nodectlConfig";


export async function run() {
  const master = (await AppGlobals.S()).getMasterWallet();
  await master.updateSeqno();
  console.log(`Master wallet address: ${master.getAddress()}`);

  const envPath = process.env.NODECTL_CONFIG_PATH;
  const configPath = envPath ?? await input("Enter nodectl config file path: ");
  const config = JSON.parse(fs.readFileSync(configPath, "utf8")) as NodectlConfig;
  const walletsConfig = config.wallets ?? {};
  console.log(`Total wallets: ${Object.keys(walletsConfig).length}`);

  const pools: Record<string, string> = {};
  for (const nodeName of Object.keys(walletsConfig)) {
    const walletConfig = walletsConfig[nodeName];
    const wallet = await createWalletContract(walletConfig);
    const nominator = SingleNominator.createFromConfig(
      {
        owner: master.getAddress(),
        validator: wallet.address,
      },
      Cell.fromHex(NOMINATOR_CODE_BOC),
      -1,
    );
    console.log(`Deploying pool '${nominator.address.toRawString()}' for ${nodeName} ${wallet.address}'`);
    let seqno = master.getSeqno();
    await master.sendTon(nominator.address, toNano("20"), nominator.init!);
    console.log(`Wait for transfer...`);
    await master.waitForSeqNoChange(seqno, 100);
    pools[nodeName] = nominator.address.toRawString();
  }

  console.log(`Done. Pools:\n${JSON.stringify(pools, null, 2)}`);
}

(async () => {
  checkEnvs();
  await run();
})();
