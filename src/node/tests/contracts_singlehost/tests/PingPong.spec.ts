import { Blockchain, SandboxContract, TreasuryContract } from '@ton/sandbox';
import { Cell, toNano } from '@ton/core';
import { PingPong } from '../wrappers/PingPong';
import '@ton/test-utils';
import { compile } from '@ton/blueprint';
import { error } from 'console';

describe('PingPong', () => {
    let code: Cell;

    beforeAll(async () => {
        code = await compile('PingPong');
    });

    let blockchain: Blockchain;
    let deployer: SandboxContract<TreasuryContract>;
    let pingPong1: SandboxContract<PingPong>;
    let pingPong2: SandboxContract<PingPong>;

    beforeEach(async () => {
        blockchain = await Blockchain.create();

        pingPong1 = blockchain.openContract(PingPong.createFromConfig(
            { accumulator: BigInt(0), last_call: 0, salt: 0, error: 0 }, code));

        pingPong2 = blockchain.openContract(PingPong.createFromConfig(
            { accumulator: BigInt(0), last_call: 0, salt: 1, error: 0 }, code));

        deployer = await blockchain.treasury('deployer');

        const deployResult1 = await pingPong1.sendDeploy(
            deployer.getSender(), toNano('10'));

        expect(deployResult1.transactions).toHaveTransaction({
            from: deployer.address,
            to: pingPong1.address,
            deploy: true,
            success: true,
        });

        const deployResult2 = await pingPong2.sendDeploy(
            deployer.getSender(), toNano('10'));

        expect(deployResult2.transactions).toHaveTransaction({
            from: deployer.address,
            to: pingPong2.address,
            deploy: true,
            success: true,
        });
    });

    it('should deploy', async () => {
        // the check is done inside beforeEach
        // blockchain and pingPong are ready to use
    });

    it('ping pong', async () => {

        var accumulator1 = await pingPong1.getAccumulator();
        var accumulator2 = await pingPong2.getAccumulator();
        console.log('Accumulator before start:', accumulator1, accumulator2);

        const starter = await blockchain.treasury('starter');

        const startResult = await pingPong1.sendStart(
            starter.getSender(), pingPong2.address);

        expect(startResult.transactions).toHaveTransaction({
            from: starter.address,
            to: pingPong1.address,
            success: true,
        });

        const error1 = await pingPong1.getError();
        const error2 = await pingPong2.getError();
        console.log('Error bits:', error1, error2);

        accumulator1 = await pingPong1.getAccumulator();
        accumulator2 = await pingPong2.getAccumulator();
        console.log('Accumulator after start:', accumulator1.toString(2), accumulator2.toString(2));

        expect(error1).toBe(0);
        expect(error2).toBe(0);
    });
});
