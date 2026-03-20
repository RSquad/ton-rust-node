import { Blockchain, SandboxContract, TreasuryContract } from '@ton/sandbox';
import { Cell, toNano } from '@ton/core';
import { SimplePingPong } from '../wrappers/SimplePingPong';
import '@ton/test-utils';
import { compile } from '@ton/blueprint';
import { error } from 'console';

describe('SimplePingPong', () => {
    let code: Cell;

    beforeAll(async () => {
        code = await compile('SimplePingPong');
    });

    let blockchain: Blockchain;
    let deployer: SandboxContract<TreasuryContract>;
    let simplePingPong1: SandboxContract<SimplePingPong>;
    let simplePingPong2: SandboxContract<SimplePingPong>;

    beforeEach(async () => {
        blockchain = await Blockchain.create();

        simplePingPong1 = blockchain.openContract(SimplePingPong.createFromConfig(
            { accumulator: 0, last_call: 0, salt: 0 }, code));

        simplePingPong2 = blockchain.openContract(SimplePingPong.createFromConfig(
            { accumulator: 0, last_call: 0, salt: 1 }, code));

        deployer = await blockchain.treasury('deployer');

        const deployResult1 = await simplePingPong1.sendDeploy(
            deployer.getSender(), toNano('0.1'));

        expect(deployResult1.transactions).toHaveTransaction({
            from: deployer.address,
            to: simplePingPong1.address,
            deploy: true,
            success: true,
        });

        const deployResult2 = await simplePingPong2.sendDeploy(
            deployer.getSender(), toNano('0.1'));

        expect(deployResult2.transactions).toHaveTransaction({
            from: deployer.address,
            to: simplePingPong2.address,
            deploy: true,
            success: true,
        });
    });

    it('should deploy', async () => {
        // the check is done inside beforeEach
        // blockchain and simplePingPong are ready to use
    });

    it('simple ping pong 10', async () => {

        var accumulator1 = await simplePingPong1.getAccumulator();
        var accumulator2 = await simplePingPong2.getAccumulator();
        console.log('Accumulator before start:', accumulator1, accumulator2);

        const starter = await blockchain.treasury('starter');

        const startResult = await simplePingPong1.sendStart(
            starter.getSender(), simplePingPong2.address, 50);

        expect(startResult.transactions).toHaveTransaction({
            from: starter.address,
            to: simplePingPong1.address,
            success: true,
        });

        accumulator1 = await simplePingPong1.getAccumulator();
        accumulator2 = await simplePingPong2.getAccumulator();
        console.log('Accumulator after start:', accumulator1.toString(), accumulator2.toString());

        expect(accumulator1).toBeGreaterThan(10);
        expect(accumulator2).toBeGreaterThan(10);
    });
});
