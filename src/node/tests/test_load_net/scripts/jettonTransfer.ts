import { Address, toNano } from "@ton/core";
import { AppGlobals } from "./globals";
import { fromJettons, fromTons, toJettons } from "./utils";
import { WalletUtils } from "./walletUtils";
import { WalletVersion } from "./wallet";

const JETTON_MINTER = Address.parse(process.env.JETTON_MINTER!);
const JETTON_DECIMALS = 9;
const FORWARD_TON = 0.01;
const FORWARD_COMMENT = '';

export async function run() {
    const tonAmount = 1.5;
    const jettonAmount = 0.1;

    let faucetWallet = (await AppGlobals.S()).getFaucetWallet();
    {
        console.log(`Update...`);
        await faucetWallet.updateSeqno();
        console.log(`Update...`);
        await faucetWallet.updateBalance();
        console.log(`Update...`);
        await faucetWallet.setJettonMinter("Test", JETTON_MINTER);
        console.log(`Update...`);
        await faucetWallet.updateJettonBalance("Test");

        console.log(`faucetWallet address(owner):  ${faucetWallet.getAddress()}`);
        console.log(`faucetWallet address(jetton): ${faucetWallet.getJettonWalletAddr("Test")}`);
        console.log(`faucetWallet balance:         ${fromTons(faucetWallet.getBalance())} TON`);
        console.log(`faucetWallet balance:         ${fromJettons(faucetWallet.getJettonBalance("Test"), JETTON_DECIMALS)} J`);
        console.log(`faucetWallet seqno:           ${faucetWallet.getSeqno()}`);
    }

    // New wallet
    let wallets = await WalletUtils.generate(1, WalletVersion.V3R2, 0, undefined);
    let wallet = wallets[0];
    {
        console.log(`Update...`);
        await wallet.updateSeqno();
        console.log(`Update...`);
        await wallet.updateBalance();
        console.log(`Update...`);
        await wallet.setJettonMinter("Test", JETTON_MINTER);
        console.log(`Update...`);
        await wallet.updateJettonBalance("Test");

        console.log(`wallet address(owner):        ${wallet.getAddress()}`);
        console.log(`wallet address(jetton):       ${wallet.getJettonWalletAddr("Test")}`);
        console.log(`wallet balance:               ${fromTons(wallet.getBalance())} TON`);
        console.log(`wallet balance:               ${fromJettons(wallet.getJettonBalance("Test"), JETTON_DECIMALS)} J`);
        console.log(`wallet seqno:                 ${wallet.getSeqno()}`);
    }

    // Transfer tons (faucet -> wallet)
    {
        console.log(`Transfer ${tonAmount} TON from '${faucetWallet.getAddress()}' to '${wallet.getAddress()}'...`);

        await faucetWallet.sendTon(
            wallet.getAddress(),
            toNano(tonAmount),
            wallet.getStateInit(),
        );

        console.log(`Wait for TON transfer...`);
        await faucetWallet.waitForBalanceChange();
        await wallet.waitForBalanceChange();
        console.log(`faucetWallet '${faucetWallet.getAddress()}' balance: ${fromTons(faucetWallet.getBalance())} TON`);
        console.log(`wallet       '${wallet.getAddress()}' balance: ${fromTons(wallet.getBalance())} TON`);
    }

    // Transfer tons (wallet -> faucet)
    {
        console.log(`Transfer ${tonAmount / 4.0} TON from '${wallet.getAddress()}' to '${faucetWallet.getAddress()}'...`);

        await wallet.updateSeqno();
        await wallet.sendTon(
            faucetWallet.getAddress(),
            toNano(tonAmount / 4.0),
            undefined
        );

        console.log(`Wait for TON transfer...`);
        await faucetWallet.waitForBalanceChange();
        await wallet.waitForBalanceChange();
        console.log(`faucetWallet '${faucetWallet.getAddress()}' balance: ${fromTons(faucetWallet.getBalance())} TON`);
        console.log(`wallet       '${wallet.getAddress()}' balance: ${fromTons(wallet.getBalance())} TON`);
    }

    // Transfer jettons (faucet -> wallet)
    {
        console.log(`Transfer ${jettonAmount} J from '${faucetWallet.getJettonWalletAddr("Test")}' to '${wallet.getAddress()}'...`);

        await faucetWallet.updateSeqno();
        await wallet.updateJettonBalance("Test");
        let seqno = faucetWallet.getSeqno();
        await faucetWallet.sendJetton(
            "Test",
            wallet.getAddress(),
            toJettons(jettonAmount, JETTON_DECIMALS),
            toNano(FORWARD_TON),
            toNano(0.5), // Fee
            FORWARD_COMMENT
        );

        console.log(`Wait for JETTON transfer...`);
        await faucetWallet.waitForSeqNoChange(seqno);
        await wallet.waitForJettonBalanceChange("Test");
        console.log(`New jetton wallet '${wallet.getJettonWalletAddr("Test")}' jetton balance: ${fromJettons(wallet.getJettonBalance("Test"), JETTON_DECIMALS)} J`);
    }

    // Transfer jettons (wallet -> faucet)
    {
        console.log(`Transfer ${jettonAmount / 4.0} J from '${wallet.getJettonWalletAddr("Test")}' to '${faucetWallet.getAddress()}'...`);

        await wallet.updateSeqno();
        await faucetWallet.updateJettonBalance("Test");
        let seqno = wallet.getSeqno();
        await wallet.sendJetton(
            "Test",
            faucetWallet.getAddress(),
            toJettons(jettonAmount / 4.0, JETTON_DECIMALS),
            toNano(FORWARD_TON / 4.0),
            toNano(0.2), // Fee
            FORWARD_COMMENT
        );

        console.log(`Wait for JETTON transfer...`);
        await wallet.waitForSeqNoChange(seqno);
        await faucetWallet.waitForJettonBalanceChange("Test");
    }

    // Faucet
    {
        console.log(`Update...`);
        await faucetWallet.updateSeqno();
        console.log(`Update...`);
        await faucetWallet.updateBalance();
        console.log(`Update...`);
        await faucetWallet.setJettonMinter("Test", JETTON_MINTER);
        console.log(`Update...`);
        await faucetWallet.updateJettonBalance("Test");

        console.log(`faucetWallet address(owner):  ${faucetWallet.getAddress()}`);
        console.log(`faucetWallet address(jetton): ${faucetWallet.getJettonWalletAddr("Test")}`);
        console.log(`faucetWallet balance:         ${fromTons(faucetWallet.getBalance())} TON`);
        console.log(`faucetWallet balance:         ${fromJettons(faucetWallet.getJettonBalance("Test"), JETTON_DECIMALS)} J`);
        console.log(`faucetWallet seqno:           ${faucetWallet.getSeqno()}`);
    }

    // New wallet
    {
        console.log(`Update...`);
        await wallet.updateSeqno();
        console.log(`Update...`);
        await wallet.updateBalance();
        console.log(`Update...`);
        await wallet.setJettonMinter("Test", JETTON_MINTER);
        console.log(`Update...`);
        await wallet.updateJettonBalance("Test");

        console.log(`wallet address(owner):        ${wallet.getAddress()}`);
        console.log(`wallet address(jetton):       ${wallet.getJettonWalletAddr("Test")}`);
        console.log(`wallet balance:               ${fromTons(wallet.getBalance())} TON`);
        console.log(`wallet balance:               ${fromJettons(wallet.getJettonBalance("Test"), JETTON_DECIMALS)} J`);
        console.log(`wallet seqno:                 ${wallet.getSeqno()}`);
    }

    console.log(`Done`);
}

run();