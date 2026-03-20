import { toNano } from '@ton/core';
import { PingPong } from '../wrappers/PingPong';
import { compile, NetworkProvider } from '@ton/blueprint';

export async function run(provider: NetworkProvider) {
    const pingPong = provider.open(PingPong.createFromConfig({}, await compile('PingPong')));

    await pingPong.sendDeploy(provider.sender(), toNano('0.05'));

    await provider.waitForDeploy(pingPong.address);

    // run methods on `pingPong`
}
