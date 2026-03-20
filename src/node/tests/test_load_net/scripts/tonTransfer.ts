import { toNano } from "@ton/core";
import { AppGlobals } from "./globals";
import { fromTons } from "./utils";
import { Wallet, WalletVersion } from "./wallet";

export async function run() {
    const tonAmount = 110000.0;

    const keys: string[] = [
        "c5a8880c862431048e11ebdbbd1c11e3fd91138b712b6f7ea3fa92665a63c06b8e07457fdd130e6ce6208b645779a406b17191a833f03119e2ad194cf6f9fe52",
        "08af1f0966d57477c10fb39c65234df97d56c47b86e7a21943bd0f11a4c38534a0f45b6764f6a8b3ad2303d93496840fe1410182e654c14f713cfdbf8506608b",
        "8165ae88d850a6ba1a35f20968bdc78af889c9cb08a60444dec5a86a25f8c41954ae1704a798e26142d441982126a56994725816988c2bb567995343da67fa32",
        "41702550d9e4dea6c141a70dac0879057dc541937725cb9d8dfb4923526eaa9953b5978ccc1cbaae27ee0390b3ce5c978665c1f6bb9d26449335630f76962630",
        "2c405b30682cb755a0236bd096140dfa88b0ea04595a67bdaa4dee393fcfe2ff3acaae0e8ebaf8e688e5ec1e9964912d3ab24f235a4fb882e480ae6ee38b68e2",
        "cdd166ea9e3a75af2295be335e9b9713e9be9853902fa045ddf926dc50357bbfdf40435db66aa5e8f2d568cdae1e55bbdf532bc0b432b201c8bd0153e2101b1f",
        "bd831e33b1f779307e50f7ec9b8c3a166b5b24d8f9f741183f726aa3785c3a2fbb96af26ccd3b92491fca5adc41544248e28d9bb699f70a48ac15e1bfc1ff9b9",
        "2b2eec98d6e1e9d2124052dde3fb5f7f721c5a7af4f1222a02e31d8d963f5886e4d138c53ba4059f34b02cf9ac154e3d2970e18d0fa89ba50c0e438cb9f42dc1",
        "a16976c351e11e4bae1185103c0ba2e61910d6f4cbff4f4d8dfe33ae3bab3ff1c8cb8742813abf582a45f0fb3428bbb274d7790c2c12a777cdf4266101d1fadb",
        "807a77e71e6ae5bb7ca5190cfd5babb78cecfcafc9ea53a94b93e5eb89567521a03297e8e5d25747eda5e049d89e3867e3b144409d893892f7131a3469fab221",
    ];

    for (let key of keys) {
        let masterWallet = (await AppGlobals.S()).getMasterWallet();
        let faucetWallet = Wallet.fromSecretKeyHex(key, WalletVersion.V3R2, -1, 42);

        console.log(`masterWallet address: ${masterWallet.getAddress()}`);
        console.log(`faucetWallet address: ${faucetWallet.getAddress()}`);

        console.log(`updateSeqno...`);
        await masterWallet.updateSeqno();
        await faucetWallet.updateSeqno();
        console.log(`masterWallet seqno: ${masterWallet.getSeqno()}`);
        console.log(`faucetWallet seqno: ${faucetWallet.getSeqno()}`);

        console.log(`updateBalance...`);
        await masterWallet.updateBalance();
        await faucetWallet.updateBalance();
        console.log(`masterWallet balance: ${fromTons(masterWallet.getBalance())} TON`);
        console.log(`faucetWallet balance: ${fromTons(faucetWallet.getBalance())} TON`);

        console.log(`Transfer ${tonAmount} TON from '${masterWallet.getAddress()}' to '${faucetWallet.getAddress()}'...`);

        let seqno = masterWallet.getSeqno();
        await masterWallet.sendTon(
            faucetWallet.getAddress(),
            toNano(tonAmount),
            faucetWallet.getStateInit(),
        );

        console.log(`Wait for transfer...`);
        await masterWallet.waitForSeqNoChange(seqno);

        console.log(`updateBalance...`);
        await masterWallet.waitForBalanceChange();
        await faucetWallet.waitForBalanceChange();
        console.log(`masterWallet balance: ${fromTons(masterWallet.getBalance())} TON`);
        console.log(`faucetWallet balance: ${fromTons(faucetWallet.getBalance())} TON`);

        console.log(`Done`);
    }
}

run();