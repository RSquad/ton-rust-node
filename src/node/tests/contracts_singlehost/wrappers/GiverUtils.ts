import { beginCell, Cell, internal } from "@ton/ton";
import { Address, storeMessageRelaxed, external } from "@ton/core";
import { toNano } from "@ton/core";
import { KeyPair, sign } from "@ton/crypto";
import { getAccountState, sendMessage } from "./ConsoleUtils";
import { log } from "console";

export async function askMoney(recipient: string, amount_tons: number) {

  const giver = "-1:77eebfb4e01e67d2ea8e255e1d6e05e371028af5ebdbfde5fd44607fd8c87017";

  // Ask seqno
  var seqno = 0;
  const giverState = await getAccountState(giver);
  if (giverState != undefined && giverState.storage.state.type === 'active' && giverState.storage.state.state.data != null) {
    seqno = giverState.storage.state.state.data.beginParse().loadUint(32);
    log(`Giver seqno: ${seqno}`);
  } else {
    throw new Error("Giver account is not active");
  }

  // Build message
  const body = buildWalletV3R2ExternalMessageBodyCell(
    60, // expire time - seconds
    seqno,
    0x2a, // walletId (my id)
    {
      publicKey: Buffer.from("44b27a27ffa31ab80971202adfcdf64aa5ec26dfb74e29f75cf643c5e40e05e4", "hex"),
      secretKey: Buffer.from("2124a0b9abb01caa9eb661ff0d66aefdba12612051996387588ae2873d069db144b27a27ffa31ab80971202adfcdf64aa5ec26dfb74e29f75cf643c5e40e05e4", "hex"),
    },
    Address.parse(recipient),
    toNano(amount_tons),
  );
  const msg = external({ to: Address.parse(giver), body });

  // Send message
  await sendMessage(msg);
}

function buildWalletV3R2ExternalMessageBodyCell(
  msgTimeoutSecs: number,
  seqno: number,
  walletId: number,
  keyPair: KeyPair,
  receiver: Address,
  value: bigint,
): Cell {
  // create simple transfer message
  const simpleTransfer = internal({
    to: receiver,
    value,
    bounce: false,
  });

  // serialize internal message
  const b = beginCell();
  storeMessageRelaxed(simpleTransfer)(b);
  const internalMessage = b.endCell();

  // external body for our wallet
  const toSign = beginCell()
    .storeUint(walletId, 32)
    .storeUint(Math.floor(Date.now() / 1000) + msgTimeoutSecs, 32) // message lifetime
    .storeUint(seqno, 32)
    .storeUint(3, 8) // sendMode: 1 (pay fees from balance) + 2 (ignore errors)
    .storeRef(internalMessage);

  // sign the body
  const signature = sign(toSign.endCell().hash(), keyPair.secretKey);
  // attach the signature to the external body
  const body = beginCell().storeBuffer(signature).storeBuilder(toSign).endCell();

  return body;
}

