/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{ContractProvider, TonWallet, smart_contract::SmartContract};
use common::{TonWalletVersion, signer::Signer, time_format};
use std::sync::Arc;
use ton_block::{
    BuilderData, Cell, CurrencyCollection, ExternalInboundMessageHeader, IBitstring,
    InternalMessageHeader, Message, MsgAddressExt, MsgAddressInt, OutAction, OutActions,
    Serializable, SliceData, StateInit, base64_decode, read_single_root_boc,
};

pub const V1R3_CODE: &str = "b5ee9c7241010101005f0000baff0020dd2082014c97ba218201339cbab19c71b0ed44d0d31fd70bffe304e0a4f260810200d71820d70b1fed44d0d31fd3ffd15112baf2a122f901541044f910f2a2f80001d31f3120d74a96d307d402fb00ded1a4c8cb1fcbffc9ed54b5b86e42";
pub const V3R2_CODE: &str = "b5ee9c724101010100710000deff0020dd2082014c97ba218201339cbab19f71b0ed44d0d31fd31f31d70bffe304e0a4f2608308d71820d31fd31fd31ff82313bbf263ed44d0d31fd31fd3ffd15132baf2a15144baf2a204f901541055f910f2a3f8009320d74a96d307d402fb00e8d101a4c8cb1fcb1fcbffc9ed5410bd6dad";
pub const V4R2_CODE_B64: &str = "te6cckECFAEAAtQAART/APSkE/S88sgLAQIBIAIDAgFIBAUE+PKDCNcYINMf0x/THwL4I7vyZO\
    1E0NMf0x/T//QE0VFDuvKhUVG68qIF+QFUEGT5EPKj+AAkpMjLH1JAyx9SMMv/UhD0AMntVPgPA\
    dMHIcAAn2xRkyDXSpbTB9QC+wDoMOAhwAHjACHAAuMAAcADkTDjDQOkyMsfEssfy/8QERITAubQ\
    AdDTAyFxsJJfBOAi10nBIJJfBOAC0x8hghBwbHVnvSKCEGRzdHK9sJJfBeAD+kAwIPpEAcjKB8v\
    /ydDtRNCBAUDXIfQEMFyBAQj0Cm+hMbOSXwfgBdM/yCWCEHBsdWe6kjgw4w0DghBkc3RyupJfBu\
    MNBgcCASAICQB4AfoA9AQw+CdvIjBQCqEhvvLgUIIQcGx1Z4MesXCAGFAEywUmzxZY+gIZ9ADLa\
    RfLH1Jgyz8gyYBA+wAGAIpQBIEBCPRZMO1E0IEBQNcgyAHPFvQAye1UAXKwjiOCEGRzdHKDHrFw\
    gBhQBcsFUAPPFiP6AhPLassfyz/JgED7AJJfA+ICASAKCwBZvSQrb2omhAgKBrkPoCGEcNQICEe\
    kk30pkQzmkD6f+YN4EoAbeBAUiYcVnzGEAgFYDA0AEbjJftRNDXCx+AA9sp37UTQgQFA1yH0BDA\
    CyMoHy//J0AGBAQj0Cm+hMYAIBIA4PABmtznaiaEAga5Drhf/AABmvHfaiaEAQa5DrhY/AAG7SB\
    /oA1NQi+QAFyMoHFcv/ydB3dIAYyMsFywIizxZQBfoCFMtrEszMyXP7AMhAFIEBCPRR8qcCAHCB\
    AQjXGPoA0z/IVCBHgQEI9FHyp4IQbm90ZXB0gBjIywXLAlAGzxZQBPoCFMtqEssfyz/Jc/sAAgB\
    sgQEI1xj6ANM/MFIkgQEI9Fnyp4IQZHN0cnB0gBjIywXLAlAFzxZQA/oCE8tqyx8Syz/Jc/sAAAr0AMntVGliJeU=";
pub const V5R1_CODE_B64: &str = "te6cckECFAEAAoEAART/APSkE/S88sgLAQIBIAIDAgFIBAUBAvIOAtzQINdJwSCRW49jINcLHy\
    CCEGV4dG69IYIQc2ludL2wkl8D4IIQZXh0brqOtIAg1yEB0HTXIfpAMPpE+Cj6RDBYvZFb4O1E0\
    IEBQdch9AWDB/QOb6ExkTDhgEDXIXB/2zzgMSDXSYECgLmRMOBw4hAPAgEgBgcCASAICQAZvl8P\
    aiaECAoOuQ+gLAIBbgoLAgFIDA0AGa3OdqJoQCDrkOuF/8AAGa8d9qJoQBDrkOuFj8AAF7Ml+1E\
    0HHXIdcLH4AARsmL7UTQ1woAgAR4g1wsfghBzaWduuvLgin8PAeaO8O2i7fshgwjXIgKDCNcjII\
    Ag1yHTH9Mf0x/tRNDSANMfINMf0//XCgAK+QFAzPkQmiiUXwrbMeHywIffArNQB7Dy0IRRJbry4\
    IVQNrry4Ib4I7vy0IgikvgA3gGkf8jKAMsfAc8Wye1UIJL4D95w2zzYEAP27aLt+wL0BCFukmwh\
    jkwCIdc5MHCUIccAs44tAdcoIHYeQ2wg10nACPLgkyDXSsAC8uCTINcdBscSwgBSMLDy0InXTNc\
    5MAGk6GwShAe78uCT10rAAPLgk+1V4tIAAcAAkVvg69csCBQgkXCWAdcsCBwS4lIQseMPINdKER\
    ITAJYB+kAB+kT4KPpEMFi68uCR7UTQgQFB1xj0BQSdf8jKAEAEgwf0U/Lgi44UA4MH9Fvy4Iwi1\
    woAIW4Bs7Dy0JDiyFADzxYS9ADJ7VQAcjDXLAgkji0h8uCS0gDtRNDSAFETuvLQj1RQMJExnAGB\
    AUDXIdcKAPLgjuLIygBYzxbJ7VST8sCN4gAQk1vbMeHXTNCon9ZI";

const SEND_MODE: u8 = 3;
const V4_OP_SIMPLE_SEND: u8 = 0;

pub struct WalletContract {
    signer: Box<dyn Signer>,
    subwallet_id: u32,
    provider: Arc<dyn ContractProvider>,
    address: MsgAddressInt,
    version: TonWalletVersion,
}

impl WalletContract {
    const LIFETIME: u32 = 120; // 2 minutes
    const V5_PREFIX_SIGNED_EXTERNAL: u32 = 0x7369_676e;

    pub async fn new(
        signer: Box<dyn Signer>,
        version: TonWalletVersion,
        subwallet_id: u32,
        workchain_id: i32,
        provider: Arc<dyn ContractProvider>,
    ) -> anyhow::Result<Self> {
        let address = WalletContract::calculate_address(
            version,
            workchain_id,
            subwallet_id,
            &signer.public_key().await?,
        )?;
        Ok(Self { signer, address, subwallet_id, provider, version })
    }

    pub fn calculate_address(
        version: TonWalletVersion,
        wc: i32,
        subwallet_id: u32,
        public_key: &[u8],
    ) -> anyhow::Result<MsgAddressInt> {
        let wallet_id = subwallet_id;
        match version {
            TonWalletVersion::V1R3 => {
                let v1r3_code = read_single_root_boc(
                    hex::decode(V1R3_CODE).expect("V1R3 code hex is invalid"),
                )?;
                let mut b = BuilderData::new();
                b.append_u32(0)?.append_raw(public_key, 256)?;
                let state = StateInit::with_code_and_data(v1r3_code, b.into_cell()?);
                let state_hash = state.write_to_new_cell()?.into_cell()?.hash(0);
                Ok(MsgAddressInt::with_params(wc, state_hash.as_slice())?)
            }
            TonWalletVersion::V3R2 => {
                let v3r2_code = read_single_root_boc(
                    hex::decode(V3R2_CODE).expect("V3R2 code hex is invalid"),
                )?;
                let mut b = BuilderData::new();
                b.append_u32(0)?.append_u32(wallet_id)?.append_raw(public_key, 256)?;
                let state = StateInit::with_code_and_data(v3r2_code, b.into_cell()?);
                let state_hash = state.write_to_new_cell()?.into_cell()?.hash(0);
                Ok(MsgAddressInt::with_params(wc, state_hash.as_slice())?)
            }
            TonWalletVersion::V4R2 => {
                let v4r2_code = read_single_root_boc(base64_decode(V4R2_CODE_B64)?)?;
                let mut b = BuilderData::new();
                b.append_u32(0)?.append_u32(wallet_id)?.append_raw(public_key, 256)?;
                b.append_bit_zero()?;
                let state = StateInit::with_code_and_data(v4r2_code, b.into_cell()?);
                let state_hash = state.write_to_new_cell()?.into_cell()?.hash(0);
                Ok(MsgAddressInt::with_params(wc, state_hash.as_slice())?)
            }
            TonWalletVersion::V5R1 => {
                let v5r1_code = read_single_root_boc(base64_decode(V5R1_CODE_B64)?)?;
                let mut b = BuilderData::new();
                b.append_bit_one()?;
                b.append_u32(0)?.append_u32(wallet_id)?.append_raw(public_key, 256)?;
                b.append_bit_zero()?;
                let state = StateInit::with_code_and_data(v5r1_code, b.into_cell()?);
                let state_hash = state.write_to_new_cell()?.into_cell()?.hash(0);
                Ok(MsgAddressInt::with_params(wc, state_hash.as_slice())?)
            }
        }
    }

    fn signing_body(
        &self,
        seqno: u32,
        dest: MsgAddressInt,
        value: u64,
        payload: Cell,
        bounce: bool,
        state_init: Option<StateInit>,
    ) -> anyhow::Result<Cell> {
        match self.version {
            TonWalletVersion::V1R3 | TonWalletVersion::V3R2 | TonWalletVersion::V4R2 => {
                let header = InternalMessageHeader {
                    bounce,
                    dst: dest,
                    value: CurrencyCollection::with_coins(value),
                    ..Default::default()
                };
                let mut internal_message =
                    Message::with_int_header_and_body(header, SliceData::load_cell(payload)?);

                if let Some(state) = state_init {
                    internal_message.set_state_init(state);
                }

                let message_cell = internal_message.serialize_as_is()?.0.into_cell()?;
                const SEND_MODE: u8 = 1 + 2; // pay fees separately + ignore errors
                let expire = time_format::now() as u32 + Self::LIFETIME;
                let mut builder = BuilderData::new();
                match self.version {
                    TonWalletVersion::V1R3 => {
                        // V1R3 body layout: seqno(32) + mode(8) + [ref: msg]
                        builder
                            .append_u32(seqno)
                            .and_then(|b| b.append_u8(SEND_MODE))
                            .and_then(|b| b.checked_append_reference(message_cell))?;
                    }
                    TonWalletVersion::V3R2 => {
                        // V3R2 body layout: subwallet_id(32) + expire(32) + seqno(32) + [ref: msg] + mode(8)
                        builder
                            .append_u32(self.subwallet_id)
                            .and_then(|b| b.append_u32(expire))
                            .and_then(|b| b.append_u32(seqno))
                            .and_then(|b| b.checked_append_reference(message_cell))
                            .and_then(|b| b.append_u8(SEND_MODE))?;
                    }
                    TonWalletVersion::V4R2 => {
                        builder
                            .append_u32(self.subwallet_id)
                            .and_then(|b| b.append_u32(expire))
                            .and_then(|b| b.append_u32(seqno))
                            .and_then(|b| b.append_u8(V4_OP_SIMPLE_SEND)) // wallet-v4 simple transfer opcode
                            .and_then(|b| b.checked_append_reference(message_cell))
                            .and_then(|b| b.append_u8(SEND_MODE))?;
                    }
                    _ => anyhow::bail!("unreachable wallet version"),
                };
                builder.into_cell()
            }
            TonWalletVersion::V5R1 => {
                let actions =
                    Self::build_v5_single_send_actions(dest, value, payload, bounce, state_init)?;
                self.signing_body_v5(seqno, actions)
            }
        }
    }

    fn signing_body_v5(&self, seqno: u32, actions: Cell) -> anyhow::Result<Cell> {
        let wallet_id = self.subwallet_id;
        let mut builder = BuilderData::new();
        builder
            .append_u32(Self::V5_PREFIX_SIGNED_EXTERNAL)?
            .append_u32(wallet_id)?
            .append_u32(time_format::now() as u32 + Self::LIFETIME)?
            .append_u32(seqno)?
            .append_bit_one()? // has_actions = true
            .checked_append_reference(actions)?
            .append_bit_zero()?; // has_other_actions = false
        builder.into_cell()
    }

    async fn sign(&self, message: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.signer.sign(message).await
    }

    fn build_v5_single_send_actions(
        dest: MsgAddressInt,
        value: u64,
        payload: Cell,
        bounce: bool,
        state_init: Option<StateInit>,
    ) -> anyhow::Result<Cell> {
        let header = InternalMessageHeader {
            bounce,
            dst: dest,
            value: CurrencyCollection::with_coins(value),
            ..Default::default()
        };
        let mut internal_message =
            Message::with_int_header_and_body(header, SliceData::load_cell(payload)?);
        if let Some(state) = state_init {
            internal_message.set_state_init(state);
        }

        let mut actions = OutActions::new();
        actions.push_back(OutAction::new_send(SEND_MODE, internal_message));
        let mut actions_builder = BuilderData::new();
        actions.write_to(&mut actions_builder)?;
        actions_builder.into_cell()
    }

    async fn build_state_init(&self) -> anyhow::Result<StateInit> {
        let pub_key = self.signer.public_key().await?;
        let wallet_id = self.subwallet_id;

        match self.version {
            TonWalletVersion::V1R3 => {
                let mut builder = BuilderData::new();
                builder.append_u32(0)?; // 32 bits: seqno = 0
                builder.append_raw(&pub_key, 256)?;
                let initial_data = builder.into_cell()?;
                let v1r3_code = read_single_root_boc(
                    hex::decode(V1R3_CODE).expect("V1R3 code hex is invalid"),
                )?;
                Ok(StateInit::with_code_and_data(v1r3_code, initial_data))
            }
            TonWalletVersion::V3R2 => {
                let mut builder = BuilderData::new();
                builder.append_u32(0)?; // seqno = 0
                builder.append_u32(wallet_id)?;
                builder.append_raw(&pub_key, 256)?;
                let initial_data = builder.into_cell()?;

                let v3r2_code = read_single_root_boc(
                    hex::decode(V3R2_CODE).expect("V3R2 code hex is invalid"),
                )?;

                Ok(StateInit::with_code_and_data(v3r2_code, initial_data))
            }
            TonWalletVersion::V4R2 => {
                let bytes = base64_decode(V4R2_CODE_B64)?;
                let v4r2_code = read_single_root_boc(bytes)?;
                let mut b = BuilderData::new();
                b.append_u32(0)?.append_u32(wallet_id)?.append_raw(&pub_key, 256)?;
                b.append_bit_zero()?; // empty plugins dict
                Ok(StateInit::with_code_and_data(v4r2_code, b.into_cell()?))
            }
            TonWalletVersion::V5R1 => {
                let bytes = base64_decode(V5R1_CODE_B64)?;
                let v5r1_code = read_single_root_boc(bytes)?;
                let mut b = BuilderData::new();
                b.append_bit_one()?; // is_signature_allowed = true
                b.append_u32(0)?.append_u32(wallet_id)?.append_raw(&pub_key, 256)?;
                b.append_bit_zero()?; // empty extensions dict
                Ok(StateInit::with_code_and_data(v5r1_code, b.into_cell()?))
            }
        }
    }
}

impl WalletContract {
    pub async fn seqno(&self) -> anyhow::Result<u32> {
        let stack = self.provider.get_method(self.address.to_string(), "seqno", vec![]).await?;
        stack.i64(0).map(|s| s as u32).map_err(|e| anyhow::anyhow!("seqno error: {}", e))
    }
}

#[async_trait::async_trait]
impl TonWallet for WalletContract {
    async fn message(
        &self,
        dest: MsgAddressInt,
        value: u64,
        payload: Cell,
    ) -> anyhow::Result<Cell> {
        self.build_message(dest, value, payload, true, None, None, None).await
    }

    async fn deploy_message(&self, value: u64, payload: Cell) -> anyhow::Result<Cell> {
        let state_init = self.build_state_init().await?;
        self.build_message(
            self.address().await?,
            value,
            payload,
            false,
            Some(0),
            Some(state_init),
            None,
        )
        .await
    }

    async fn state_init(&self) -> anyhow::Result<StateInit> {
        self.build_state_init().await
    }

    async fn build_message(
        &self,
        dest: MsgAddressInt,
        value: u64,
        payload: Cell,
        bounce: bool,
        seqno: Option<u32>,
        state_init_external: Option<StateInit>,
        state_init_internal: Option<StateInit>,
    ) -> anyhow::Result<Cell> {
        let seqno = match seqno {
            Some(seqno) => seqno,
            None => self.seqno().await.map_err(|e| anyhow::anyhow!("get seqno error: {}", e))?,
        };

        let body_slice = match self.version {
            TonWalletVersion::V5R1 => {
                // V5: signature at end, uses OutActions
                let actions_cell = Self::build_v5_single_send_actions(
                    dest,
                    value,
                    payload,
                    bounce,
                    state_init_internal,
                )?;

                let signing_cell = self.signing_body_v5(seqno, actions_cell)?;
                let signature = self.sign(signing_cell.hash(0).as_slice()).await?;

                // V5: body first, then signature
                let mut builder = BuilderData::from_cell(&signing_cell)?;
                builder.append_raw(&signature, 512)?;
                SliceData::load_builder(builder)?
            }
            TonWalletVersion::V1R3 | TonWalletVersion::V3R2 | TonWalletVersion::V4R2 => {
                // signature first, then body
                let signing_cell =
                    self.signing_body(seqno, dest, value, payload, bounce, state_init_internal)?;
                let signature = self.sign(signing_cell.hash(0).as_slice()).await?;

                let mut builder = BuilderData::new();
                builder.append_raw(&signature, 512)?;
                builder.append_builder(&BuilderData::from_cell(&signing_cell)?)?;
                SliceData::load_builder(builder)?
            }
        };

        let mut message = Message::with_ext_in_header_and_body(
            ExternalInboundMessageHeader::new(MsgAddressExt::AddrNone, self.address().await?),
            body_slice,
        );

        if let Some(state) = state_init_external {
            message.set_state_init(state);
        }

        let (builder, _, _) = message
            .serialize_as_is()
            .map_err(|e| anyhow::anyhow!("external message serialization error: {:?}", e))?;

        builder.into_cell()
    }
}

#[async_trait::async_trait]
impl SmartContract for WalletContract {
    async fn address(&self) -> anyhow::Result<MsgAddressInt> {
        Ok(self.address.clone())
    }
    async fn balance(&self) -> anyhow::Result<u64> {
        self.provider.balance(&self.address).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Verify wallet code hashes match official TON contracts
    /// https://github.com/ton-blockchain/wallet-contract
    #[test]
    fn test_wallet_code_hashes() {
        let v3r2 = read_single_root_boc(hex::decode(V3R2_CODE).unwrap()).unwrap();
        let v4r2 = read_single_root_boc(base64_decode(V4R2_CODE_B64).unwrap()).unwrap();
        let v5r1 = read_single_root_boc(base64_decode(V5R1_CODE_B64).unwrap()).unwrap();

        assert_eq!(
            format!("{:x}", v3r2.repr_hash()),
            "84dafa449f98a6987789ba232358072bc0f76dc4524002a5d0918b9a75d2d599"
        );
        assert_eq!(
            format!("{:x}", v4r2.repr_hash()),
            "feb5ff6820e2ff0d9483e7e0d62c817d846789fb4ae580c878866d959dabd5c0"
        );
        assert_eq!(
            format!("{:x}", v5r1.repr_hash()),
            "20834b7b72b112147e1b2fb457b84e74d1a30f04f737d4f62a668e9552d2b72f"
        );
    }

    /// Verify address calculation against real mainnet wallet
    #[test]
    fn test_address_calculation() {
        let public_key =
            hex::decode("72c9ed6b62a6e2eba14a93b90462e7a367777beb8a38fb15b9f33844d22ce2ff")
                .unwrap();

        // real wallet EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N
        let v3r2 =
            WalletContract::calculate_address(TonWalletVersion::V3R2, 0, 698983191, &public_key)
                .unwrap();
        assert_eq!(
            v3r2,
            MsgAddressInt::from_str(
                "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8"
            )
            .unwrap()
        );

        let v4r2 =
            WalletContract::calculate_address(TonWalletVersion::V4R2, 0, 698983191, &public_key)
                .unwrap();
        assert_eq!(
            v4r2,
            MsgAddressInt::from_str(
                "0:1195e0aa861dee81216752efbcf2425ebbd7e846987b5fab01e9bc5303fdd1b0"
            )
            .unwrap()
        );

        let v5r1 =
            WalletContract::calculate_address(TonWalletVersion::V5R1, 0, 698983191, &public_key)
                .unwrap();
        assert_eq!(
            v5r1,
            MsgAddressInt::from_str(
                "0:b3f77c0448a5b1c15112a7fcda3007cc6ba473565a7b6c4968aa0c6de23c525b"
            )
            .unwrap()
        );
    }
}
