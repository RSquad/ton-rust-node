import { Cell, beginCell, Address, Contract, toNano, contractAddress, ContractProvider, Sender, SendMode, MessageRelaxed, storeMessageRelaxed } from "@ton/core";
import { KeyPair, sign } from "@ton/crypto";

const OP = {
  WITHDRAW: 0x1000,
  CHANGE_VALIDATOR_ADDRESS: 0x1001,
  SEND_RAW_MSG: 0x7702,
  UPGRADE: 0x9903,
  NEW_STAKE: 0x4e73744b,
  NEW_STAKE_SIGNED: 0x654c5074,
  NEW_STAKE_OK: 0xf374484c,
  NEW_STAKE_FAILED: 0xee6f454c,
  RECOVER_STAKE: 0x47657424,
  RECOVER_STAKE_OK: 0xf96f7324,
  RECOVER_STAKE_FAILED: 0xfffffffe,
};

export const NOMINATOR_CODE_BOC = "b5ee9c7241020d010001f0000114ff00f4a413f4bcf2c80b01020162020a02bcd0ed44d0fa40fa40d122c700925f06e003d0d3030171b0925f06e0fa403002d31f7022c000228b1778c705b022d74ac000b08e136c21830bc85376a182103b9aca00a1fa02c9d09430d33f12e25343c7059133e30d5235c705925f06e30d030401c421830bba8ea0fa005387a182103b9aca00a112b60881200421c200f2f452406d80188040db3cde21811001ba9efa405044c858cf1601cf16c9ed549133e220817702ba9802d307d402fb0002de2082009903ba9d02d4812002226ef2f201fb0402de0904f22382104e73744bba8fe102fa4430f828fa443081200302c0ff12f2f4830c01c0fff2f481200122f2f481200524821047868c00bef2f4fa0020db3c300581200405a182103b9aca00a15210bb14f2f4db3c82104e73744bc8cb1f5220cb3f5005cf16c9443080188040db3c9410356c41e201821047657424ba05070906001cd3ff31d31fd31f31d3ff31d431d102368f16821047657424c8cb1fcb3fc9db3c705880188040db3c9130e20709011671f833d0d70bff7f01db3c08001674c8cb0212ca07cbffc9d00048226eb32091719170e203c8cb055006cf165004fa02cb6a039358cc019130e201c901fb000201200b0c0027bdf8cb938b82a38002a380036b6aa39152988b6c0015bfe5076a2687d207d2068c5fb766c2";

const signData = (data: Cell, priv: Buffer) => {
  // I know buffer aggregation is supposed to be recursive, but it's good enough for elector cases
  return beginCell()
    .storeBuffer(sign(Buffer.from(data.bits.toString(), "hex"), priv))
    .endCell();
};

export type SingleNominatorConfig = {
  owner: Address;
  validator: Address;
};

type NewStakeOpts = {
  max_factor: number;
  adnl_address: bigint;
  query_id: bigint | number;
  value: bigint;
};

export const defaultNewStake: NewStakeOpts = {
  max_factor: 1 << 16,
  adnl_address: BigInt(0),
  query_id: 1,
  value: toNano("1.2"),
};

export function PoolConfigToCell(config: SingleNominatorConfig) {
  return beginCell().storeAddress(config.owner).storeAddress(config.validator).endCell();
}
export class SingleNominator implements Contract {
  constructor(
    readonly address: Address,
    readonly init?: { code: Cell; data: Cell },
    workchain = -1,
  ) {}

  static createFromAddress(address: Address) {
    return new SingleNominator(address);
  }
  static createFromConfig(opts: SingleNominatorConfig, code?: Cell, workchain: 0 | -1 = -1) {
    if (!code) {
      code = Cell.fromHex(NOMINATOR_CODE_BOC);
    }
    const data = PoolConfigToCell(opts);
    const init = { code, data };
    return new SingleNominator(contractAddress(workchain, init), init);
  }

  async sendDeploy(provider: ContractProvider, via: Sender, value: bigint, query_id: number | bigint = 0) {
    await provider.internal(via, {
      value,
      sendMode: SendMode.PAY_GAS_SEPARATELY,
      body: beginCell().storeUint(0, 32).storeUint(query_id, 64).endCell(),
    });
  }

  static withdrawMessage(amount: bigint, query_id: bigint | number = 0) {
    return beginCell().storeUint(OP.WITHDRAW, 32).storeUint(query_id, 64).storeCoins(amount).endCell();
  }

  async sendWithdraw(provider: ContractProvider, via: Sender, amount: bigint, value: bigint = toNano("0.1"), query_id: bigint | number = 0) {
    await provider.internal(via, {
      body: SingleNominator.withdrawMessage(amount),
      value,
      sendMode: SendMode.PAY_GAS_SEPARATELY,
    });
  }

  static changeValidatorMessage(validator: Address, query_id: bigint | number = 0) {
    return beginCell().storeUint(OP.CHANGE_VALIDATOR_ADDRESS, 32).storeUint(query_id, 64).storeAddress(validator).endCell();
  }
  async sendChangeValidator(provider: ContractProvider, via: Sender, validator: Address, value: bigint = toNano("0.1"), query_id: bigint | number = 0) {
    await provider.internal(via, {
      value,
      body: SingleNominator.changeValidatorMessage(validator, query_id),
      sendMode: SendMode.PAY_GAS_SEPARATELY,
    });
  }

  static rawMessage(msg: MessageRelaxed | Cell, mode: number, query_id: number | bigint = 0) {
    let msgCell: Cell;
    if (msg instanceof Cell) {
      msgCell = msg;
    } else {
      msgCell = beginCell().store(storeMessageRelaxed(msg)).endCell();
    }
    return beginCell().storeUint(OP.SEND_RAW_MSG, 32).storeUint(query_id, 64).storeRef(msgCell).storeUint(mode, 8).endCell();
  }

  async sendRawMessage(provider: ContractProvider, via: Sender, msg: MessageRelaxed | Cell, mode: number, value: bigint = toNano("0.1"), query_id: bigint | number = 0) {
    await provider.internal(via, {
      value,
      body: SingleNominator.rawMessage(msg, mode, query_id),
      sendMode: SendMode.PAY_GAS_SEPARATELY,
    });
  }

  static upgradeMessage(code: Cell, query_id: bigint | number = 0) {
    return beginCell().storeUint(OP.UPGRADE, 32).storeUint(query_id, 64).storeRef(code).endCell();
  }

  async sendUpgradeMessage(provider: ContractProvider, via: Sender, code: Cell, value: bigint = toNano("0.1"), query_id: bigint | number = 0) {
    await provider.internal(via, {
      value,
      body: SingleNominator.upgradeMessage(code, query_id),
      sendMode: SendMode.PAY_GAS_SEPARATELY,
    });
  }

  static newStakeMessage(stake_val: bigint, src: Address, keys: KeyPair, stake_at: number | bigint, opts: NewStakeOpts = defaultNewStake) {
    const signCell = beginCell()
      .storeUint(OP.NEW_STAKE_SIGNED, 32)
      .storeUint(stake_at, 32)
      .storeUint(opts.max_factor, 32)
      .storeBuffer(src.hash, 32)
      .storeUint(opts.adnl_address, 256)
      .endCell();

    const signature = signData(signCell, keys.secretKey);

    return beginCell()
      .storeUint(OP.NEW_STAKE, 32)
      .storeUint(opts.query_id, 64)
      .storeCoins(stake_val)
      .storeBuffer(keys.publicKey, 32)
      .storeUint(stake_at, 32)
      .storeUint(opts.max_factor, 32)
      .storeUint(opts.adnl_address, 256)
      .storeRef(signature)
      .endCell();
  }

  async sendNewStake(provider: ContractProvider, via: Sender, stake_val: bigint, keys: KeyPair, stake_at: number | bigint, opts?: Partial<NewStakeOpts>) {
    let curOpts: NewStakeOpts;
    if (opts) {
      curOpts = {
        ...defaultNewStake,
        ...opts,
      };
    } else {
      curOpts = { ...defaultNewStake };
    }

    await provider.internal(via, {
      value: curOpts.value,
      body: SingleNominator.newStakeMessage(stake_val, this.address, keys, stake_at, curOpts),
      sendMode: SendMode.PAY_GAS_SEPARATELY,
    });
  }
  static recoverStakeMessage(query_id: bigint | number = 0) {
    return beginCell().storeUint(OP.RECOVER_STAKE, 32).storeUint(query_id, 64).endCell();
  }

  async sendRecoverStake(provider: ContractProvider, via: Sender, value: bigint = toNano("1"), query_id: bigint | number = 0) {
    await provider.internal(via, {
      body: SingleNominator.recoverStakeMessage(query_id),
      sendMode: SendMode.PAY_GAS_SEPARATELY,
      value,
    });
  }

  async getRoles(provider: ContractProvider) {
    const { stack } = await provider.get("get_roles", []);

    return {
      owner: stack.readAddress(),
      validator: stack.readAddress(),
    };
  }
}
