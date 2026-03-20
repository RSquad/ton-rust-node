/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::collections::HashMap;
use ton_block::{base64_decode, error, read_single_root_boc, Cell, Result, SliceData};

type Extractor = fn(&Cell) -> WalletResult;
type WalletResult = Result<serde_json::Map<String, serde_json::Value>>;

#[derive(Clone, Copy)]
pub(crate) struct WalletInfo {
    wallet_type: &'static str,
    extractor: Extractor,
}

impl WalletInfo {
    const fn new(wallet_type: &'static str, extractor: Extractor) -> Self {
        Self { wallet_type, extractor }
    }
    pub(crate) fn wallet_type(&self) -> &'static str {
        self.wallet_type
    }
    pub(crate) fn extract(&self, data: &Cell) -> WalletResult {
        (self.extractor)(data)
    }
}

pub struct WalletLibrary {
    wallets: HashMap<[u8; 32], WalletInfo>,
}

impl WalletLibrary {
    pub fn new() -> Result<Self> {
        let mut wallets = HashMap::with_capacity(WALLET_CODES.len());
        for descriptor in WALLET_CODES {
            let bytes = base64_decode(descriptor.code_boc)
                .map_err(|e| error!("invalid wallet code {}: {e}", descriptor.wallet_type))?;
            let cell = read_single_root_boc(&bytes)
                .map_err(|e| error!("invalid wallet code boc {}: {e}", descriptor.wallet_type))?;
            let hash = *cell.repr_hash().as_array();
            wallets.insert(hash, WalletInfo::new(descriptor.wallet_type, descriptor.extractor));
        }
        Ok(Self { wallets })
    }

    fn find_by_hash(&self, hash: &[u8; 32]) -> Option<&WalletInfo> {
        self.wallets.get(hash)
    }

    pub(crate) fn find_by_code(&self, code: &Cell) -> Result<Option<&WalletInfo>> {
        let hash = Self::calc_hash(code)?;
        Ok(self.find_by_hash(&hash))
    }

    fn calc_hash(code: &Cell) -> Result<[u8; 32]> {
        Ok(*code.repr_hash().as_array())
    }
}

struct WalletCodeDescriptor {
    code_boc: &'static str,
    wallet_type: &'static str,
    extractor: Extractor,
}

const fn descriptor(
    code_boc: &'static str,
    wallet_type: &'static str,
    extractor: Extractor,
) -> WalletCodeDescriptor {
    WalletCodeDescriptor { code_boc, wallet_type, extractor }
}

fn empty_extractor(_: &Cell) -> WalletResult {
    Ok(serde_json::Map::new())
}

fn seqno_extractor(data: &Cell) -> WalletResult {
    let mut slice = SliceData::load_cell(data.clone())?;
    let seqno = slice.get_next_u32()?;
    let mut result = serde_json::Map::new();
    result.insert("seqno".into(), serde_json::Value::from(seqno));
    Ok(result)
}

fn v3_extractor(data: &Cell) -> WalletResult {
    let mut slice = SliceData::load_cell(data.clone())?;
    let seqno = slice.get_next_u32()?;
    let wallet_id = slice.get_next_u32()?;
    let mut result = serde_json::Map::new();
    result.insert("seqno".into(), serde_json::Value::from(seqno));
    result.insert("wallet_id".into(), serde_json::Value::from(wallet_id));
    Ok(result)
}

fn v5_extractor(data: &Cell) -> WalletResult {
    let mut slice = SliceData::load_cell(data.clone())?;
    let is_signature_allowed = slice.get_next_bit()?;
    let seqno = slice.get_next_u32()?;
    let wallet_id = slice.get_next_u32()?;
    let mut result = serde_json::Map::new();
    result.insert("is_signature_allowed".into(), serde_json::Value::Bool(is_signature_allowed));
    result.insert("seqno".into(), serde_json::Value::from(seqno));
    result.insert("wallet_id".into(), serde_json::Value::from(wallet_id));
    Ok(result)
}

const WALLET_V1_R1: &str =
    "te6cckEBAQEARAAAhP8AIN2k8mCBAgDXGCDXCx/tRNDTH9P/0VESuvKhIvkBVBBE+RDyovgAAd\
    MfMSDXSpbTB9QC+wDe0aTIyx/L/8ntVEH98Ik=";
const WALLET_V1_R2: &str =
    "te6cckEBAQEAUwAAov8AIN0gggFMl7qXMO1E0NcLH+Ck8mCBAgDXGCDXCx/tRNDTH9P/0VESuv\
    KhIvkBVBBE+RDyovgAAdMfMSDXSpbTB9QC+wDe0aTIyx/L/8ntVNDieG8=";
const WALLET_V1_R3: &str =
    "te6cckEBAQEAXwAAuv8AIN0gggFMl7ohggEznLqxnHGw7UTQ0x/XC//jBOCk8mCBAgDXGCDXCx\
    /tRNDTH9P/0VESuvKhIvkBVBBE+RDyovgAAdMfMSDXSpbTB9QC+wDe0aTIyx/L/8ntVLW4bkI=";
const WALLET_V2_R1: &str =
    "te6cckEBAQEAVwAAqv8AIN0gggFMl7qXMO1E0NcLH+Ck8mCDCNcYINMf0x8B+CO78mPtRNDTH9\
    P/0VExuvKhA/kBVBBC+RDyovgAApMg10qW0wfUAvsA6NGkyMsfy//J7VShNwu2";
const WALLET_V2_R2: &str =
    "te6cckEBAQEAYwAAwv8AIN0gggFMl7ohggEznLqxnHGw7UTQ0x/XC//jBOCk8mCDCNcYINMf0x\
    8B+CO78mPtRNDTH9P/0VExuvKhA/kBVBBC+RDyovgAApMg10qW0wfUAvsA6NGkyMsfy//J7VQETNeh";
const WALLET_V3_R1: &str =
    "te6cckEBAQEAYgAAwP8AIN0gggFMl7qXMO1E0NcLH+Ck8mCDCNcYINMf0x/TH/gjE7vyY+1E0N\
    Mf0x/T/9FRMrryoVFEuvKiBPkBVBBV+RDyo/gAkyDXSpbTB9QC+wDo0QGkyMsfyx/L/8ntVD++buA=";
const WALLET_V3_R2: &str =
    "te6cckEBAQEAcQAA3v8AIN0gggFMl7ohggEznLqxn3Gw7UTQ0x/THzHXC//jBOCk8mCDCNcYIN\
    Mf0x/TH/gjE7vyY+1E0NMf0x/T/9FRMrryoVFEuvKiBPkBVBBV+RDyo/gAkyDXSpbTB9QC+wDo0\
    QGkyMsfyx/L/8ntVBC9ba0=";
const WALLET_V4_R1: &str =
    "te6cckECFQEAAvUAART/APSkE/S88sgLAQIBIAIDAgFIBAUE+PKDCNcYINMf0x/THwL4I7vyY+\
    1E0NMf0x/T//QE0VFDuvKhUVG68qIF+QFUEGT5EPKj+AAkpMjLH1JAyx9SMMv/UhD0AMntVPgPA\
    dMHIcAAn2xRkyDXSpbTB9QC+wDoMOAhwAHjACHAAuMAAcADkTDjDQOkyMsfEssfy/8REhMUA+7Q\
    AdDTAwFxsJFb4CHXScEgkVvgAdMfIYIQcGx1Z70ighBibG5jvbAighBkc3RyvbCSXwPgAvpAMCD\
    6RAHIygfL/8nQ7UTQgQFA1yH0BDBcgQEI9ApvoTGzkl8F4ATTP8glghBwbHVnupEx4w0kghBibG\
    5juuMABAYHCAIBIAkKAFAB+gD0BDCCEHBsdWeDHrFwgBhQBcsFJ88WUAP6AvQAEstpyx9SEMs/A\
    FL4J28ighBibG5jgx6xcIAYUAXLBSfPFiT6AhTLahPLH1Iwyz8B+gL0AACSghBkc3Ryuo41BIEB\
    CPRZMO1E0IEBQNcgyAHPFvQAye1UghBkc3Rygx6xcIAYUATLBVjPFiL6AhLLassfyz+UEDRfBOL\
    JgED7AAIBIAsMAFm9JCtvaiaECAoGuQ+gIYRw1AgIR6STfSmRDOaQPp/5g3gSgBt4EBSJhxWfMY\
    QCAVgNDgARuMl+1E0NcLH4AD2ynftRNCBAUDXIfQEMALIygfL/8nQAYEBCPQKb6ExgAgEgDxAAG\
    a3OdqJoQCBrkOuF/8AAGa8d9qJoQBBrkOuFj8AAbtIH+gDU1CL5AAXIygcVy//J0Hd0gBjIywXL\
    AiLPFlAF+gIUy2sSzMzJcfsAyEAUgQEI9FHypwIAbIEBCNcYyFQgJYEBCPRR8qeCEG5vdGVwdIA\
    YyMsFywJQBM8WghAF9eEA+gITy2oSyx/JcfsAAgBygQEI1xgwUgKBAQj0WfKn+CWCEGRzdHJwdI\
    AYyMsFywJQBc8WghAF9eEA+gIUy2oTyx8Syz/Jc/sAAAr0AMntVEap808=";
const WALLET_V4_R2: &str =
    "te6cckECFAEAAtQAART/APSkE/S88sgLAQIBIAIDAgFIBAUE+PKDCNcYINMf0x/THwL4I7vyZO\
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
const WALLET_V5_R1: &str =
    "te6cckECFAEAAoEAART/APSkE/S88sgLAQIBIAIDAgFIBAUBAvIOAtzQINdJwSCRW49jINcLHy\
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
const NOMINATOR_POOL_V1: &str =
    "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";

const WALLET_CODES: &[WalletCodeDescriptor] = &[
    descriptor(WALLET_V1_R1, "wallet v1 r1", seqno_extractor),
    descriptor(WALLET_V1_R2, "wallet v1 r2", seqno_extractor),
    descriptor(WALLET_V1_R3, "wallet v1 r3", seqno_extractor),
    descriptor(WALLET_V2_R1, "wallet v2 r1", seqno_extractor),
    descriptor(WALLET_V2_R2, "wallet v2 r2", seqno_extractor),
    descriptor(WALLET_V3_R1, "wallet v3 r1", v3_extractor),
    descriptor(WALLET_V3_R2, "wallet v3 r2", v3_extractor),
    descriptor(WALLET_V4_R1, "wallet v4 r1", v3_extractor),
    descriptor(WALLET_V4_R2, "wallet v4 r2", v3_extractor),
    descriptor(WALLET_V5_R1, "wallet v5 r1", v5_extractor),
    descriptor(NOMINATOR_POOL_V1, "nominator pool v1", empty_extractor),
];
