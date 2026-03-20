import { toNano } from '@ton/core';
import { JettonMinter } from '../wrappers/JettonMinter';
import { compile, NetworkProvider } from '@ton/blueprint';
import { AppGlobals } from './globals';

export async function run(_provider: NetworkProvider) {
    const tonClient = (await AppGlobals.S()).nextTonCenterClient();
    let faucetWallet = (await AppGlobals.S()).getFaucetWallet();

    console.log(`Sender address: ${faucetWallet.getAddress()}`);

    const jettonWalletCode = await compile('JettonWallet');
    console.log(`Jetton admin address: ${faucetWallet.getAddress()}`);

    const jettonMetadataUri = "https://domain"

    const minter = tonClient.open
        (
            JettonMinter.createFromConfig
                (
                    {
                        admin: faucetWallet.getAddress(),
                        wallet_code: jettonWalletCode,
                        jetton_content: { uri: jettonMetadataUri }
                    },
                    await compile('JettonMinter')
                )
        );

    await minter.sendDeploy(
        await faucetWallet.getSender(),
        toNano("1.5")
    );

    console.log(`Jetton address: ${minter.address}`);
}
