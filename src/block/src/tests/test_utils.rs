/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
struct AccountTestOptions {
    balance: Option<CurrencyCollection>,
    state_init: StateInitTestOptions,
}

impl AccountTestOptions {
    const SAMPLE: [u8; 8] = [
        0b00111111, 0b11111111, 0b00011111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ];
    fn with_default_setup(is_library_public: bool) -> Self {
        let mut state_init = StateInitTestOptions::with_default_setup(is_library_public);
        let sample = SliceData::new(Self::SAMPLE.to_vec()).into_cell().unwrap();
        state_init.data = Some(sample);
        Self { balance: None, state_init }
    }
}

struct StateInitTestOptions {
    code: Option<Cell>,
    data: Option<Cell>,
    library: Option<Cell>,
    is_library_public: bool,
    is_set_tick_tock: bool,
    fixed_prefix_length: u32,
}

impl StateInitTestOptions {
    const SAMPLE: [u8; 8] = [
        0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ];
    fn with_default_setup(is_library_public: bool) -> Self {
        let sample = SliceData::new(Self::SAMPLE.to_vec()).into_cell().unwrap();
        Self {
            code: Some(sample.clone()),
            data: Some(sample.clone()),
            library: Some(sample),
            is_library_public,
            is_set_tick_tock: true,
            fixed_prefix_length: 23,
        }
    }
}

fn generate_test_stateinit(options: StateInitTestOptions) -> StateInit {
    let mut stinit = StateInit::default();
    stinit.set_fixed_prefix_length(options.fixed_prefix_length.try_into().unwrap());
    if options.is_set_tick_tock {
        stinit.set_special(TickTock::with_values(false, true))
    }
    if let Some(code) = options.code {
        stinit.set_code(code)
    }
    if let Some(data) = options.data {
        stinit.set_data(data)
    }
    if let Some(library) = options.library {
        stinit.set_library_code(library, options.is_library_public).unwrap()
    }
    stinit
}

#[allow(dead_code)]
fn generate_test_message(big: bool, options: StateInitTestOptions) -> Message {
    let mut stinit = generate_test_stateinit(options);

    if big {
        let mut code0 = SliceData::load_cell_ref(stinit.code().unwrap()).unwrap();
        let mut code1 = SliceData::new(vec![
            0xad, 0xc9, 0xba, 0xfc, 0x56, 0x94, 0x11, 0x56, 0x58, 0xfa, 0x2b, 0xdf, 0xe4, 0x65,
            0x15, 0x1a, 0x32, 0x03, 0x69, 0x4a, 0xff, 0xcd, 0x00, 0x8f, 0x36, 0x8b, 0xd2, 0xcc,
            0x8c, 0xc8, 0x10, 0xfb, 0x6b, 0x5b, 0x51,
        ]);
        let mut code2 = SliceData::new(vec![
            0xad, 0xc9, 0xba, 0xfc, 0x56, 0x94, 0x11, 0x56, 0x58, 0xfa, 0x2b, 0xdf, 0xe4, 0x65,
            0x15, 0x1a, 0x32, 0x03, 0x69, 0x4a, 0xff, 0xcd, 0x00, 0x8f, 0x36, 0x8b, 0xd2, 0xcc,
            0x8c, 0xc8, 0x10, 0xfb, 0x6b, 0x5b, 0x51,
        ]);
        let code3 = SliceData::new(vec![
            0xad, 0xc9, 0xba, 0xfc, 0x56, 0x94, 0x11, 0x57, 0x58, 0xfa, 0x2b, 0xdf, 0xe4, 0x65,
            0x15, 0x1a, 0x32, 0x03, 0x69, 0x4a, 0xff, 0xcd, 0x00, 0x8f, 0x36, 0x8b, 0xd2, 0xcc,
            0x8c, 0xc8, 0x10, 0xfb, 0x6b, 0x5b, 0x51,
        ]);
        code2.append_reference(code3).unwrap();
        code1.append_reference(code2).unwrap();
        code0.append_reference(code1).unwrap();
        stinit.set_code(code0.into_cell().unwrap());
    }

    let mut body0 = SliceData::new(vec![
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x80,
    ])
    .into_builder()
    .unwrap();
    if big {
        let mut body1 = SliceData::new(vec![
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE,
            0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0x80,
        ])
        .into_builder()
        .unwrap();
        let body2 = SliceData::new(vec![
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6,
            0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0x80,
        ])
        .into_builder()
        .unwrap();
        body1.checked_append_reference(body2.into_cell().unwrap()).unwrap();
        body0.checked_append_reference(body1.into_cell().unwrap()).unwrap();
    }

    let mut msg = Message::with_int_header(Default::default());
    msg.set_state_init(stinit);
    msg.set_body(SliceData::load_builder(body0).unwrap());
    msg
}

fn generate_test_account(big: bool, options: AccountTestOptions) -> Account {
    let acc_id = AccountId::from(&[
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
        0x1E, 0x1F,
    ]);

    let g = Some(111.into());
    let st_info = StorageInfo::with_values(123456789, g);
    let mut stinit = generate_test_stateinit(options.state_init);

    if big {
        let mut code = SliceData::load_cell_ref(stinit.code().unwrap()).unwrap();
        let mut subcode1 = SliceData::new(vec![
            0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
            0b11110100,
        ]);
        let mut subcode2 = SliceData::new(vec![
            0b00111111, 0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
            0b11110100,
        ]);
        let mut subcode3 = SliceData::new(vec![
            0b00001111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
            0b11110100,
        ]);
        let subcode4 = SliceData::new(vec![
            0b00111111, 0b11111111, 0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
            0b11110100,
        ]);
        subcode3.append_reference(subcode4).unwrap();
        subcode2.append_reference(subcode3).unwrap();
        subcode1.append_reference(subcode2).unwrap();
        code.append_reference(subcode1).unwrap();
        stinit.set_code(code.into_cell().unwrap());
    }

    let balance = options.balance.unwrap_or_else(|| {
        let mut balance = CurrencyCollection::with_grams(100000000000);
        balance.set_other(1, 100).unwrap();
        balance.set_other(2, 200).unwrap();
        balance.set_other(3, 300).unwrap();
        balance.set_other(4, 400).unwrap();
        balance.set_other(5, 500).unwrap();
        balance.set_other(6, 600).unwrap();
        balance.set_other(7, 10000100).unwrap();
        balance
    });

    let acc_st = AccountStorage::active(0, balance, stinit);
    let addr = MsgAddressInt::with_standart(None, 0, acc_id).unwrap();
    let mut account = Account::with_storage(&addr, &st_info, &acc_st);
    // TODO: use proper DICT_HASH_MIN_CELLS constant when `include` macros are removed -
    // `crate::DICT_HASH_MIN_CELLS` vs `block::DICT_HASH_MIN_CELLS` collision now
    account.update_storage_stat(26).unwrap();
    account
}

#[allow(dead_code)]
fn write_read_and_assert<T>(s: T) -> T
where
    T: Serializable + Deserializable + std::fmt::Debug + Default + PartialEq,
{
    let cell = s.serialize().unwrap();
    let mut slice = SliceData::load_cell_ref(&cell).unwrap();
    println!("slice: {}", slice);
    let s2: T = T::construct_from(&mut slice).unwrap();
    let cell2 = s2.serialize().unwrap();
    pretty_assertions::assert_eq!(s, s2);
    if cell != cell2 {
        panic!(
            "write_read_and_assert: cells are not equal\nleft: {:#.5}\nright: {:#.5}",
            cell, cell2
        )
    }
    s2
}
