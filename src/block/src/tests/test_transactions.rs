/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{
    generate_test_message, write_read_and_assert, Message, MsgAddressExt, MsgAddressInt,
    StateInitTestOptions,
};
use std::sync::Arc;

pub struct TestTransactionSet {
    pub account_id: AccountId,
    pub in_msg: Message,
    pub out_msgs: Vec<Message>,
    pub lt: u64,
    pub orig_status: AccountStatus,
}

pub fn create_test_transaction_set() -> TestTransactionSet {
    let account_id = AccountId::from([1; 32]);

    // Create inbound message
    let in_msg = Message::with_int_header(crate::InternalMessageHeader::with_addresses_and_bounce(
        MsgAddressInt::with_standart(None, 0, [0x55; 32].into()).unwrap(),
        MsgAddressInt::with_standart(None, 0, [0x66; 32].into()).unwrap(),
        CurrencyCollection::from_grams(5_000_000_000.into()),
        true,
    ));

    // Create out internal msg
    let int_addr1 = MsgAddressInt::with_standart(None, 0, [0x11; 32].into()).unwrap();
    let int_addr2 = MsgAddressInt::with_standart(None, 0, [0x22; 32].into()).unwrap();
    let hdr = crate::InternalMessageHeader::with_addresses_and_bounce(
        int_addr1,
        int_addr2,
        CurrencyCollection::from_grams(1_000_000_000.into()),
        true,
    );
    let int_msg = Message::with_int_header(hdr);

    // Create out std external out msg
    let int_addr3 = MsgAddressInt::with_standart(None, 0, [0x33; 32].into()).unwrap();
    let ext_addr = MsgAddressExt::with_extern([0x99; 32].into()).unwrap();
    let hdr = crate::ExtOutMessageHeader::with_addresses(int_addr3, ext_addr);
    let ext_msg = Message::with_ext_out_header(hdr);
    let options = StateInitTestOptions::with_default_setup(true);
    TestTransactionSet {
        account_id,
        lt: 12345,
        orig_status: AccountStatus::AccStateActive,
        in_msg,
        out_msgs: vec![int_msg, ext_msg, generate_test_message(true, options)],
    }
}

#[test]
fn test_transaction_serde() {
    let address = AccountId::from([1; 32]);
    let tr = generate_tranzaction(address);
    write_read_and_assert(tr);
}

#[test]
fn test_account_block_serde() {
    let address = AccountId::from([1; 32]);
    let acc_block = generate_account_block(address, 32).unwrap();

    // let mut cell = BuilderData::new();
    // acc_block.transactions.write_hashmap_data(&mut cell).unwrap();
    // let mut hmp = HashmapAugE::<Grams>::with_data(64, &mut SliceData::from(cell));

    // assert_eq!(acc_block.transactions, hmp);

    // let mut cell = BuilderData::new();
    // acc_block.transactions.write_hashmap_root(&mut cell).unwrap();

    // let mut hmp = HashmapAugE::<Grams>::with_bit_len(64);
    // hmp.read_hashmap_root::<InRefValue<Transaction>>(&mut cell.into());

    // assert_eq!(acc_block.transactions.get_data(), hmp.get_data());
    // assert_eq!(acc_block.transactions, hmp);

    write_read_and_assert(acc_block);
}

#[test]
fn test_account_block_serde_max() {
    std::env::set_var("RUST_BACKTRACE", "full");

    let address = AccountId::from([1; 32]);
    let mut acc_block = AccountBlock::with_address(address.clone());
    let s_status_update = HashUpdate::default();
    acc_block.write_state_update(&s_status_update).unwrap();

    let mut transaction = generate_tranzaction(address);
    let mut fees = CurrencyCollection::with_grams(u64::MAX);
    fees.set_other(1, 111).unwrap();
    transaction.set_total_fees(fees);
    acc_block.add_transaction(&transaction).unwrap();

    transaction.set_logical_time(transaction.logical_time() + 1);
    acc_block.add_transaction(&transaction).unwrap();

    write_read_and_assert(acc_block);
}

#[test]
fn test_shard_account_blocks() {
    let shard_block = generate_test_shard_account_block();
    write_read_and_assert(shard_block);
}

#[test]
fn test_acc_status_change() {
    write_read_and_assert(AccStatusChange::Unchanged);
    write_read_and_assert(AccStatusChange::Frozen);
    write_read_and_assert(AccStatusChange::Deleted);
}

#[test]
fn test_compute_skin_reason() {
    write_read_and_assert(ComputeSkipReason::NoState);
    write_read_and_assert(ComputeSkipReason::BadState);
    write_read_and_assert(ComputeSkipReason::NoGas);
    write_read_and_assert(ComputeSkipReason::Suspended);
}

#[test]
fn test_compute_phase() {
    write_read_and_assert(TrComputePhase::Skipped(TrComputePhaseSkipped {
        reason: ComputeSkipReason::BadState,
    }));

    write_read_and_assert(TrComputePhase::Vm(TrComputePhaseVm {
        success: false,
        msg_state_used: false,
        account_activated: false,
        gas_fees: 18740987136491.into(),
        gas_used: 45.into(),
        gas_limit: 58.into(),
        gas_credit: Some(58.into()),
        mode: -56,
        exit_code: -65432,
        exit_arg: Some(-294579),
        vm_steps: 0xffffffff,
        vm_init_state_hash: "1000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .unwrap(),
        vm_final_state_hash: "1000000000000000000000000000000000000000000000000000000000000003"
            .parse()
            .unwrap(),
    }));

    write_read_and_assert(TrComputePhase::Vm(TrComputePhaseVm {
        success: true,
        msg_state_used: false,
        account_activated: true,
        gas_fees: 18740987136491.into(),
        gas_used: 45.into(),
        gas_limit: 58.into(),
        gas_credit: None,
        mode: -56,
        exit_code: -65432,
        exit_arg: None,
        vm_steps: 0xffffffff,
        vm_init_state_hash: "1000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .unwrap(),
        vm_final_state_hash: "1000000000000000000000000000000000000000000000000000000000000003"
            .parse()
            .unwrap(),
    }));
}

#[test]
fn test_tr_storage_phase() {
    write_read_and_assert(TrStoragePhase {
        storage_fees_collected: 653.into(),
        storage_fees_due: Some(12345679567653.into()),
        status_change: AccStatusChange::Frozen,
    });
    write_read_and_assert(TrStoragePhase {
        storage_fees_collected: 653.into(),
        storage_fees_due: None,
        status_change: AccStatusChange::Deleted,
    });
}

#[test]
fn test_bounce_phase() {
    write_read_and_assert(TrBouncePhase::Negfunds);

    write_read_and_assert(TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
        msg_size: StorageUsed::with_values_checked(4, 5).unwrap(),
        req_fwd_fees: 3425978345987.into(),
    }));

    write_read_and_assert(TrBouncePhase::Ok(TrBouncePhaseOk {
        msg_size: StorageUsed::with_values_checked(4, 5).unwrap(),
        msg_fees: Grams::one(),
        fwd_fees: 456.into(),
    }));
}

#[test]
fn test_credit_phase() {
    let mut credit = CurrencyCollection::with_grams(1);
    credit.set_other(500, 9_000_000 + 777).unwrap();
    credit.set_other(1005001, 8_000_000 + 1005700).unwrap();
    credit.set_other(1005002, 555_000_000 + 1070500).unwrap();
    credit.set_other(10023, 1_000_000 + 1).unwrap();
    credit.set_other(1005004, 6_767_000_000 + 8888).unwrap();
    credit.set_other(10035, 13_000_000 + 1).unwrap();
    credit.set_other(1005006, 4_000_000 + 6).unwrap();
    credit.set_other(1005007, 5_000_000 + 7).unwrap();
    credit.set_other(10047, 1_000_000 + 1).unwrap();
    credit.set_other(10050, 1_111_000_000 + 100500).unwrap();
    credit.set_other(1001, 10_042_222_000_000 + 1006500).unwrap();
    credit.set_other(105, 1_000_000 + 1).unwrap();
    credit.set_other(1000, 2_000_000 + 5).unwrap();
    credit.set_other(10500, 3_000_000 + 6).unwrap();
    credit.set_other(10, 4_000_000 + 777).unwrap();
    credit.set_other(100, 74_000_000 + 7).unwrap();
    credit.set_other(1000, 1_000_000 + 1).unwrap();
    credit.set_other(1005000, 1_005_050_000_000 + 100500).unwrap();
    credit.set_other(80, 100_500_000_000 + 8).unwrap();

    write_read_and_assert(TrCreditPhase { due_fees_collected: Some(123600079553.into()), credit });

    let mut credit = CurrencyCollection::with_grams(1);
    credit.set_other(500, 9_000_000 + 777).unwrap();
    credit.set_other(1005001, 8_000_000 + 1005700).unwrap();
    credit.set_other(1005002, 555_000_000 + 1070500).unwrap();
    credit.set_other(10023, 1_000_000 + 1).unwrap();

    write_read_and_assert(TrCreditPhase { due_fees_collected: None, credit });
}

#[test]
fn test_transaction_tick_tock() {
    write_read_and_assert(TransactionTickTock::Tick);
    write_read_and_assert(TransactionTickTock::Tock);
}

#[test]
fn test_tr_action_phase() {
    write_read_and_assert(TrActionPhase {
        success: true,
        valid: false,
        no_funds: true,
        status_change: AccStatusChange::Frozen,
        total_fwd_fees: Some(111.into()),
        total_action_fees: None,
        result_code: -123456,
        result_arg: Some(-876),
        tot_actions: 345,
        spec_actions: 5435,
        skipped_actions: 1,
        msgs_created: 12,
        action_list_hash: "1000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
        tot_msg_size: StorageUsed::with_values_checked(1, 2).unwrap(),
        action_fine: Default::default(),
    });

    write_read_and_assert(TrActionPhase {
        success: false,
        valid: true,
        no_funds: false,
        status_change: AccStatusChange::Frozen,
        total_fwd_fees: None,
        total_action_fees: Some(111.into()),
        result_code: -123,
        result_arg: None,
        tot_actions: 45,
        spec_actions: 4,
        skipped_actions: 1,
        msgs_created: 12,
        action_list_hash: "1000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
        tot_msg_size: StorageUsed::with_values_checked(1, 2).unwrap(),
        action_fine: Default::default(),
    });
}

#[test]
fn test_split_merge_info() {
    write_read_and_assert(SplitMergeInfo {
        cur_shard_pfx_len: 0b00101111,
        acc_split_depth: 0,
        this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
        sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
    });
}

#[test]
fn test_split_merge_info2() {
    SplitMergeInfo {
        cur_shard_pfx_len: 0b01001111,
        acc_split_depth: 0,
        this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
        sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
    }
    .serialize()
    .expect_err("Should be error on invalid shard prefix");
}

#[test]
fn test_tr_descr_order() {
    // TrStoragePhase //
    let stor_phase = TrStoragePhase {
        storage_fees_collected: 653.into(),
        storage_fees_due: Some(12345679567653.into()),
        status_change: AccStatusChange::Frozen,
    };

    // TrCreditPhase //
    let mut credit = CurrencyCollection::with_grams(1);
    credit.set_other(500, 9_000_000 + 777).unwrap();
    credit.set_other(1005001, 8_000_000 + 1005700).unwrap();
    credit.set_other(1005002, 555_000_000 + 1070500).unwrap();
    credit.set_other(10023, 1_000_000 + 1).unwrap();
    credit.set_other(1005004, 6_767_000_000 + 8888).unwrap();
    credit.set_other(10035, 13_000_000 + 1).unwrap();
    credit.set_other(1005006, 4_000_000 + 6).unwrap();
    credit.set_other(1005007, 5_000_000 + 7).unwrap();
    credit.set_other(10047, 1_000_000 + 1).unwrap();
    credit.set_other(10050, 1_111_000_000 + 100500).unwrap();
    credit.set_other(1001, 10_042_222_000_000 + 1006500).unwrap();
    credit.set_other(105, 1_000_000 + 1).unwrap();
    credit.set_other(1000, 2_000_000 + 5).unwrap();
    credit.set_other(10500, 3_000_000 + 6).unwrap();
    credit.set_other(10, 4_000_000 + 777).unwrap();
    credit.set_other(100, 74_000_000 + 7).unwrap();
    credit.set_other(1000, 1_000_000 + 1).unwrap();
    credit.set_other(1005000, 1_005_050_000_000 + 100500).unwrap();
    credit.set_other(80, 100_500_000_000 + 8).unwrap();

    let credit_phase = TrCreditPhase { due_fees_collected: Some(123600079553.into()), credit };

    // TrComputePhase Vm //
    let compute_phase = TrComputePhase::Vm(TrComputePhaseVm {
        success: true,
        msg_state_used: false,
        account_activated: true,
        gas_fees: 18740987136491.into(),
        gas_used: 45.into(),
        gas_limit: 58.into(),
        gas_credit: Some(3.into()),
        mode: -56,
        exit_code: -65432,
        exit_arg: None,
        vm_steps: 0xffffffff,
        vm_init_state_hash: "1000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .unwrap(),
        vm_final_state_hash: "1000000000000000000000000000000000000000000000000000000000000003"
            .parse()
            .unwrap(),
    });

    // TrActionPhase //
    let action_phase = TrActionPhase {
        success: true,
        valid: false,
        no_funds: true,
        status_change: AccStatusChange::Frozen,
        total_fwd_fees: Some(111.into()),
        total_action_fees: None,
        result_code: -123456,
        result_arg: Some(-876),
        tot_actions: 345,
        spec_actions: 5435,
        skipped_actions: 1,
        msgs_created: 12,
        action_list_hash: "1000000000000000000000000000000000000000000000000000000000000055"
            .parse()
            .unwrap(),
        tot_msg_size: StorageUsed::with_values_checked(1, 2).unwrap(),
        action_fine: Default::default(),
    };

    // TrBouncePhase //
    let bounce_phase = TrBouncePhase::Nofunds(TrBouncePhaseNofunds {
        msg_size: StorageUsed::with_values_checked(4, 5).unwrap(),
        req_fwd_fees: 3425978345987.into(),
    });

    write_read_and_assert(TransactionDescr::Ordinary(TransactionDescrOrdinary {
        credit_first: false,
        storage_ph: Some(stor_phase),
        credit_ph: Some(credit_phase),
        compute_ph: compute_phase,
        action: Some(action_phase),
        aborted: false,
        bounce: Some(bounce_phase),
        destroyed: true,
    }));
}

#[test]
fn test_tr_descr_tick_tock() {
    write_read_and_assert(TransactionDescr::TickTock(TransactionDescrTickTock {
        tt: TransactionTickTock::Tick,
        storage: TrStoragePhase {
            storage_fees_collected: 653.into(),
            storage_fees_due: Some(12345679567653.into()),
            status_change: AccStatusChange::Frozen,
        },
        compute_ph: TrComputePhase::Skipped(TrComputePhaseSkipped {
            reason: ComputeSkipReason::BadState,
        }),
        action: None,
        aborted: true,
        destroyed: true,
    }));

    write_read_and_assert(TransactionDescr::TickTock(TransactionDescrTickTock {
        tt: TransactionTickTock::Tick,
        storage: TrStoragePhase {
            storage_fees_collected: 653.into(),
            storage_fees_due: Some(12345679567653.into()),
            status_change: AccStatusChange::Frozen,
        },
        compute_ph: TrComputePhase::Vm(TrComputePhaseVm {
            success: true,
            msg_state_used: false,
            account_activated: true,
            gas_fees: 18740987136491.into(),
            gas_used: 45.into(),
            gas_limit: 58.into(),
            gas_credit: None,
            mode: -56,
            exit_code: -65432,
            exit_arg: None,
            vm_steps: 0xffffffff,
            vm_init_state_hash: "1000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            vm_final_state_hash: "1000000000000000000000000000000000000000000000000000000000000003"
                .parse()
                .unwrap(),
        }),
        action: Some(TrActionPhase {
            success: true,
            valid: false,
            no_funds: true,
            status_change: AccStatusChange::Frozen,
            total_fwd_fees: Some(111.into()),
            total_action_fees: None,
            result_code: -123456,
            result_arg: Some(-876),
            tot_actions: 345,
            spec_actions: 5435,
            skipped_actions: 1,
            msgs_created: 12,
            action_list_hash: "1000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            tot_msg_size: StorageUsed::with_values_checked(1, 2).unwrap(),
            action_fine: Default::default(),
        }),
        aborted: false,
        destroyed: true,
    }));
}

#[test]
fn test_tr_descr_split_prepare() {
    write_read_and_assert(TransactionDescr::SplitPrepare(TransactionDescrSplitPrepare {
        split_info: SplitMergeInfo {
            cur_shard_pfx_len: 0b00101111,
            acc_split_depth: 0,
            this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
        },
        compute_ph: TrComputePhase::Skipped(TrComputePhaseSkipped {
            reason: ComputeSkipReason::BadState,
        }),
        action: Some(TrActionPhase {
            success: true,
            valid: false,
            no_funds: true,
            status_change: AccStatusChange::Frozen,
            total_fwd_fees: Some(111.into()),
            total_action_fees: None,
            result_code: -123456,
            result_arg: Some(-876),
            tot_actions: 345,
            spec_actions: 5435,
            skipped_actions: 1,
            msgs_created: 12,
            action_list_hash: "1000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            tot_msg_size: StorageUsed::with_values_checked(1, 2).unwrap(),
            action_fine: Default::default(),
        }),
        aborted: false,
        destroyed: true,
    }));

    write_read_and_assert(TransactionDescr::SplitPrepare(TransactionDescrSplitPrepare {
        split_info: SplitMergeInfo {
            cur_shard_pfx_len: 0b00101111,
            acc_split_depth: 0,
            this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
        },
        compute_ph: TrComputePhase::Vm(TrComputePhaseVm {
            success: true,
            msg_state_used: false,
            account_activated: true,
            gas_fees: 18740987136491.into(),
            gas_used: 45.into(),
            gas_limit: 58.into(),
            gas_credit: None,
            mode: -56,
            exit_code: -65432,
            exit_arg: None,
            vm_steps: 0xffffffff,
            vm_init_state_hash: "1000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            vm_final_state_hash: "1000000000000000000000000000000000000000000000000000000000000003"
                .parse()
                .unwrap(),
        }),
        action: None,
        aborted: false,
        destroyed: true,
    }));
}

#[test]
fn test_tr_descr_split_install() {
    write_read_and_assert(TransactionDescr::SplitInstall(TransactionDescrSplitInstall {
        split_info: SplitMergeInfo {
            cur_shard_pfx_len: 0b00101111,
            acc_split_depth: 0,
            this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
        },
        prepare_transaction: Arc::new(generate_tranzaction(AccountId::from([1; 32]))),
        installed: true,
    }));
}

#[test]
fn test_tr_descr_mege_prepare() {
    write_read_and_assert(TransactionDescr::MergePrepare(TransactionDescrMergePrepare {
        split_info: SplitMergeInfo {
            cur_shard_pfx_len: 0b00101111,
            acc_split_depth: 0,
            this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
        },
        storage_ph: TrStoragePhase {
            storage_fees_collected: 653.into(),
            storage_fees_due: Some(12345679567653.into()),
            status_change: AccStatusChange::Frozen,
        },
        aborted: true,
    }));
}

#[test]
fn test_tr_descr_mege_install() {
    // TrCreditPhase //
    let mut credit = CurrencyCollection::with_grams(1);
    credit.set_other(500, 9_000_000 + 777).unwrap();
    credit.set_other(1005001, 8_000_000 + 1005700).unwrap();
    credit.set_other(1005002, 555_000_000 + 1070500).unwrap();
    credit.set_other(10023, 1_000_000 + 1).unwrap();
    credit.set_other(1005004, 6_767_000_000 + 8888).unwrap();
    credit.set_other(10035, 13_000_000 + 1).unwrap();
    credit.set_other(1005006, 4_000_000 + 6).unwrap();
    credit.set_other(1005007, 5_000_000 + 7).unwrap();
    credit.set_other(10047, 1_000_000 + 1).unwrap();
    credit.set_other(10050, 1_111_000_000 + 100500).unwrap();
    credit.set_other(1001, 10_042_222_000_000 + 1006500).unwrap();
    credit.set_other(105, 1_000_000 + 1).unwrap();
    credit.set_other(1000, 2_000_000 + 5).unwrap();
    credit.set_other(10500, 3_000_000 + 6).unwrap();
    credit.set_other(10, 4_000_000 + 777).unwrap();
    credit.set_other(100, 74_000_000 + 7).unwrap();
    credit.set_other(1000, 1_000_000 + 1).unwrap();
    credit.set_other(1005000, 1_005_050_000_000 + 100500).unwrap();
    credit.set_other(80, 100_500_000_000 + 8).unwrap();

    let credit_phase = TrCreditPhase { due_fees_collected: Some(123600079553.into()), credit };

    write_read_and_assert(TransactionDescr::MergeInstall(TransactionDescrMergeInstall {
        split_info: SplitMergeInfo {
            cur_shard_pfx_len: 0b00101111,
            acc_split_depth: 0,
            this_addr: "6000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            sibling_addr: "3000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
        },
        prepare_transaction: Arc::new(generate_tranzaction(AccountId::from([1; 32]))),
        credit_ph: Some(credit_phase),
        compute_ph: TrComputePhase::Vm(TrComputePhaseVm {
            success: true,
            msg_state_used: false,
            account_activated: true,
            gas_fees: 18740987136491.into(),
            gas_used: 45.into(),
            gas_limit: 58.into(),
            gas_credit: None,
            mode: -56,
            exit_code: -65432,
            exit_arg: None,
            vm_steps: 0xffffffff,
            vm_init_state_hash: "1000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            vm_final_state_hash: "1000000000000000000000000000000000000000000000000000000000000003"
                .parse()
                .unwrap(),
        }),
        action: Some(TrActionPhase {
            success: true,
            valid: false,
            no_funds: true,
            status_change: AccStatusChange::Frozen,
            total_fwd_fees: Some(111.into()),
            total_action_fees: None,
            result_code: -123456,
            result_arg: Some(-876),
            tot_actions: 345,
            spec_actions: 5435,
            skipped_actions: 1,
            msgs_created: 12,
            action_list_hash: "1000000000000000000000000000000000000000000000000000000000000055"
                .parse()
                .unwrap(),
            tot_msg_size: StorageUsed::with_values_checked(1, 2).unwrap(),
            action_fine: Default::default(),
        }),
        aborted: false,
        destroyed: true,
    }));
}

#[test]
fn test_hash_update_serialization() {
    let hu = HashUpdate::with_hashes(
        "1000000000000000000000012300000000000000000000000000000000000001".parse().unwrap(),
        "1000000000000000000000000000004560000000000000000000000000000001".parse().unwrap(),
    );

    write_read_and_assert(hu);
}

#[test]
fn test_shard_account_block() {
    let address = AccountId::from([0x11; 32]);
    generate_account_block(address, 32).unwrap();
}

#[allow(dead_code)]
pub fn generate_tranzaction(address: AccountId) -> Transaction {
    let s_status_update = HashUpdate::default();
    let s_tr_desc = TransactionDescr::default();

    let data = create_test_transaction_set();
    let mut tr = Transaction::with_address_and_status(address, data.orig_status.clone());
    tr.write_in_msg(Some(&data.in_msg)).unwrap();
    for ref msg in data.out_msgs {
        tr.add_out_message(msg).unwrap();
    }
    tr.set_logical_time(data.lt);
    tr.set_end_status(AccountStatus::AccStateFrozen);
    tr.set_total_fees(CurrencyCollection::with_grams(653));
    tr.write_state_update(&s_status_update).unwrap();
    tr.write_description(&s_tr_desc).unwrap();
    tr
}

fn generate_account_block(address: AccountId, tr_count: usize) -> Result<AccountBlock> {
    let s_status_update = HashUpdate::default();
    let mut acc_block = AccountBlock::with_address(address.clone());

    for _ in 0..tr_count {
        let transaction = generate_tranzaction(address.clone());
        acc_block.add_transaction(&transaction)?;
    }
    acc_block.write_state_update(&s_status_update).unwrap();

    Ok(acc_block)
}

pub fn generate_test_shard_account_block() -> ShardAccountBlocks {
    let mut shard_block = ShardAccountBlocks::default();

    for n in 0..10 {
        let address = AccountId::from([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, n as u8,
        ]);
        let account_block = generate_account_block(address.clone(), n + 1).unwrap();
        shard_block.insert(&account_block).unwrap();
    }
    shard_block
}
