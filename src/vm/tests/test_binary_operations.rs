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
mod common;
use common::*;
use ton_block::ExceptionCode;
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

#[test]
fn test_div() {
    test_case(
        "PUSHINT 15
         PUSHINT 4
         DIV
         PUSHINT 15
         PUSHINT 4
         DIVR
         PUSHINT 15
         PUSHINT 4
         DIVC
         PUSHINT 15
         PUSHINT 4
         MOD
         PUSHINT 15
         PUSHINT 4
         DIVMOD
         PUSHINT 15
         PUSHINT 4
         DIVMODR
         PUSHINT 15
         PUSHINT 4
         DIVMODC
         PUSHINT 15
         MODPOW2 2
         PUSHINT 7
         PUSHINT 3
         PUSHINT 4
         MULDIV
         PUSHINT 7
         PUSHINT 3
         PUSHINT 4
         MULDIVR
         PUSHINT 7
         PUSHINT 3
         PUSHINT 4
         MULDIVC
         PUSHINT 7
         PUSHINT 3
         PUSHINT 4
         MULDIVMOD",
    )
    .expect_stack(
        Stack::new()
            .push(int!(3))
            .push(int!(4))
            .push(int!(4))
            .push(int!(3))
            .push(int!(3))
            .push(int!(3))
            .push(int!(4))
            .push(int!(-1))
            .push(int!(4))
            .push(int!(-1))
            .push(int!(3))
            .push(int!(5))
            .push(int!(5))
            .push(int!(6))
            .push(int!(5))
            .push(int!(1)),
    );
}

#[test]
fn zero_shift() {
    test_case(
        "
        PUSHINT 0
        LSHIFT  1
        PUSHINT 0
        PUSHINT 0
        LSHIFT
        PUSHINT 0
        PUSHINT 1
        LSHIFT
        PUSHINT 2
        PUSHINT 0
        LSHIFT

        PUSHINT 0
        RSHIFT  1
        PUSHINT 0
        PUSHINT 0
        RSHIFT
        PUSHINT 0
        PUSHINT 1
        RSHIFT
        PUSHINT 2
        PUSHINT 0
        RSHIFT
        ",
    )
    .expect_stack(
        Stack::new()
            .push(int!(0))
            .push(int!(0))
            .push(int!(0))
            .push(int!(2))
            .push(int!(0))
            .push(int!(0))
            .push(int!(0))
            .push(int!(2)),
    );
}

#[test]
fn lshift_by_a_constant() {
    test_case(
        "PUSHINT 3
         LSHIFT  2",
    )
    .expect_item(int!(0b1100));
}

#[test]
fn lshift() {
    test_case(
        "PUSHINT 3
         PUSHINT 2
         LSHIFT",
    )
    .expect_item(int!(0b1100));
}

#[test]
fn rshift() {
    test_case(
        "PUSHINT 9
         PUSHINT 1
         RSHIFT",
    )
    .expect_item(int!(0b100));
}

#[test]
fn rshift_by_a_constant() {
    test_case(
        "PUSHINT 9
         RSHIFT  1",
    )
    .expect_item(int!(0b100));
}

#[test]
fn pow2() {
    test_case(
        "PUSHINT 4
         POW2",
    )
    .expect_item(int!(0b10000));
}

#[test]
fn pow2_nan() {
    test_case(
        "PUSHNAN
         POW2",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn pow2_large() {
    test_case(
        "PUSHINT 1024
         POW2",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn pow2_small() {
    test_case(
        "PUSHINT 256
         POW2",
    )
    .expect_failure(ExceptionCode::IntegerOverflow);
}

#[test]
fn qpow2() {
    test_case(
        "PUSHINT 4
         QPOW2",
    )
    .expect_item(int!(0b10000));
}

#[test]
fn qpow2_nan() {
    test_case(
        "PUSHNAN
         QPOW2",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn qpow2_large() {
    test_case(
        "PUSHINT 1024
         QPOW2",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn qpow2_small() {
    test_case(
        "PUSHINT 256
         QPOW2",
    )
    .expect_item(int!(nan));
}

#[test]
fn and_unsigned() {
    test_case(
        "PUSHINT 5
         PUSHINT 4
         AND",
    )
    .expect_item(int!(0b100));
}

#[test]
fn or_unsigned() {
    test_case(
        "PUSHINT 2
         PUSHINT 9
         OR",
    )
    .expect_item(int!(0b1011));
}

#[test]
fn xor_unsigned() {
    test_case(
        "PUSHINT 3
         PUSHINT 9
         XOR",
    )
    .expect_item(int!(0b1010));
}

mod test_not {
    use super::*;

    #[test]
    fn not_zero() {
        test_case(
            "PUSHINT 0
            NOT",
        )
        .expect_item(int!(-1));
    }

    #[test]
    fn not_one() {
        test_case(
            "PUSHINT 1
            NOT",
        )
        .expect_item(int!(-2));
    }

    #[test]
    fn not_186() {
        test_case(
            "PUSHINT 186
            NOT",
        )
        .expect_item(int!(-187));
    }

    #[test]
    fn not_nan() {
        test_case(
            "PUSHNAN
            NOT",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn qnot_nan() {
        test_case(
            "PUSHNAN
            QNOT",
        )
        .expect_item(int!(nan));
    }
}

// UFITS cc+1 (x – x), checks whether x is a cc+1-bit unsigned integer
// for 0 ≤ cc ≤ 255, i.e., whether 0 ≤ x < (2^(cc+1)).
mod test_ufits {
    use super::*;

    #[test]
    fn test_exact_number_of_bits() {
        // 7 < 2^3
        test_case(
            "PUSHINT 7
             UFITS 3",
        )
        .expect_item(int!(0b111));
    }

    #[test]
    fn test_requires_one_more_bit() {
        // 8 = 2^3
        test_case(
            "PUSHINT 8
             UFITS    3",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_chkbit_zero() {
        test_case(
            "PUSHINT 0
             CHKBIT",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_chkbit_one() {
        test_case(
            "PUSHINT 1
             CHKBIT",
        )
        .expect_item(int!(1));
    }

    #[test]
    fn test_chkbit_overflow() {
        test_case(
            "PUSHINT 2
             CHKBIT",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_negative_number() {
        test_case(
            "
            PUSHINT -1
            UFITS 8
        ",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }
}

mod test_ufitsx {
    use super::*;

    #[test]
    fn test_exact_number_of_bits() {
        // 7 < 2^3
        test_case(
            "PUSHINT 7
             PUSHINT 3
             UFITSX",
        )
        .expect_item(int!(0b111));
    }
    #[test]
    fn test_requires_one_more_bit() {
        // 8 = 2^3
        test_case(
            "PUSHINT 8
             PUSHINT 3
             UFITSX",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_int_nan() {
        test_case(
            "ONE
            PUSHNAN
            UFITSX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_nan_int() {
        test_case(
            "PUSHNAN
            ONE
            UFITSX",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_nan_int_quiet() {
        test_case(
            "PUSHNAN
            ONE
            QUFITSX",
        )
        .expect_item(int!(nan));
    }

    #[test]
    fn test_int_nan_quiet() {
        test_case(
            "ONE
            PUSHNAN
            QUFITSX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_negative_number() {
        test_case(
            "
            PUSHINT -1
            PUSHINT 8
            UFITSX
        ",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }
}

// FITS cc+1 (x – x), checks whether x is a cc+1-bit signed integer for 0 ≤ cc ≤ 255,
// i.e., whether −(2^cc) ≤ x < 2^cc. If not so, either triggers an integer overflow
// exception, or replaces x with a NaN (quiet version).
mod test_fits {
    use super::*;

    #[test]
    fn test_negative_number() {
        test_case(
            "PUSHINT -8
             FITS 4",
        )
        .expect_item(int!(-0b1000));
    }

    #[test]
    fn test_exact_number_of_bits() {
        test_case(
            "PUSHINT 7
             FITS    4",
        )
        .expect_item(int!(0b111));
    }

    #[test]
    fn test_requires_one_more_bit_negative_int() {
        test_case(
            "PUSHINT -9
             FITS    4",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_requires_one_more_bit() {
        test_case(
            "PUSHINT 8
             FITS    4",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_chkbool_true() {
        test_case(
            "PUSHINT -1
             CHKBOOL",
        )
        .expect_item(int!(-1));
    }

    #[test]
    fn test_chkbool_false() {
        test_case(
            "PUSHINT 0
             CHKBOOL",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_chkbool_overflow() {
        test_case(
            "PUSHINT 1
             CHKBOOL",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }
}

mod test_fitsx {
    use super::*;

    #[test]
    fn test_negative_number() {
        test_case(
            "PUSHINT -8
             PUSHINT 4
             FITSX",
        )
        .expect_item(int!(-0b1000));
    }

    #[test]
    fn test_exact_number_of_bits() {
        test_case(
            "PUSHINT 7
             PUSHINT 4
             FITSX",
        )
        .expect_item(int!(0b111));
    }

    #[test]
    fn test_requires_one_more_bit_negative_int() {
        test_case(
            "PUSHINT -9
             PUSHINT 4
             FITSX",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_requires_one_more_bit() {
        test_case(
            "PUSHINT 8
             PUSHINT 4
             FITSX",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_int_nan() {
        test_case(
            "ONE
            PUSHNAN
            FITSX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_nan_int() {
        test_case(
            "PUSHNAN
            ONE
            FITSX",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_nan_int_quiet() {
        test_case(
            "PUSHNAN
            ONE
            QFITSX",
        )
        .expect_item(int!(nan));
    }

    #[test]
    fn test_int_nan_quiet() {
        test_case(
            "ONE
            PUSHNAN
            QFITSX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
}

mod test_comparsion {
    use super::*;

    #[test]
    fn test_min() {
        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            MIN",
        )
        .expect_item(int!(17));
    }

    #[test]
    fn test_qmin() {
        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            QMIN
            PUSHNAN
            PUSHINT 25
            QMIN",
        )
        .expect_stack(Stack::new().push(int!(17)).push(int!(nan)));
    }

    #[test]
    fn test_max() {
        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            MAX",
        )
        .expect_item(int!(25));
    }

    #[test]
    fn test_qmax() {
        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            QMAX
            PUSHNAN
            PUSHINT 25
            QMAX",
        )
        .expect_stack(Stack::new().push(int!(25)).push(int!(nan)));
    }

    #[test]
    fn test_minmax_swap() {
        test_case(
            "
            PUSHINT 25
            PUSHINT 17
            MINMAX",
        )
        .expect_stack(Stack::new().push(int!(17)).push(int!(25)));

        test_case(
            "
            PUSHINT 25
            PUSHINT 17
            INTSORT2",
        )
        .expect_int_stack(&[17, 25]);
    }

    #[test]
    fn test_minmax() {
        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            MINMAX",
        )
        .expect_stack(Stack::new().push(int!(17)).push(int!(25)));

        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            INTSORT2",
        )
        .expect_int_stack(&[17, 25]);
    }

    #[test]
    fn test_qminmax() {
        test_case(
            "
            PUSHINT 17
            PUSHINT 25
            QMINMAX
            PUSHINT 17
            PUSHNAN
            QMINMAX",
        )
        .expect_stack(Stack::new().push(int!(17)).push(int!(25)).push(int!(nan)).push(int!(nan)));
    }

    #[test]
    fn test_abs() {
        test_case(
            "
            PUSHINT -17
            ABS
            PUSHINT 0
            ABS
            PUSHINT 17
            ABS",
        )
        .expect_stack(Stack::new().push(int!(17)).push(int!(0)).push(int!(17)));
    }

    #[test]
    fn test_abs_nan() {
        test_case(
            "
            PUSHNAN
            ABS",
        )
        .expect_failure(ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn test_qabs_nan() {
        test_case(
            "
            PUSHNAN
            QABS",
        )
        .expect_stack(Stack::new().push(int!(nan)));
    }
}

mod test_bitsize {
    use super::*;

    #[test]
    fn test_bitsize_neg() {
        test_case(
            "
            PUSHINT -1
            BITSIZE
            PUSHINT -2
            BITSIZE
            PUSHINT -3
            BITSIZE
            PUSHINT -4
            BITSIZE",
        )
        .expect_stack(Stack::new().push(int!(1)).push(int!(2)).push(int!(3)).push(int!(3)));
    }

    #[test]
    fn test_bitsize_pos() {
        test_case(
            "
            PUSHINT 0
            BITSIZE
            PUSHINT 1
            BITSIZE
            PUSHINT 2
            BITSIZE
            PUSHINT 3
            BITSIZE
            PUSHINT 4
            BITSIZE",
        )
        .expect_int_stack(&[0, 2, 3, 3, 4]);
    }

    #[test]
    fn test_ubitsize_pos() {
        test_case(
            "
            PUSHINT 0
            UBITSIZE
            PUSHINT 1
            UBITSIZE
            PUSHINT 2
            UBITSIZE
            PUSHINT 3
            UBITSIZE",
        )
        .expect_int_stack(&[0, 1, 2, 2]);
    }

    #[test]
    fn test_ubitsize_neg() {
        test_case(
            "
            PUSHINT -5
            UBITSIZE",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_qubitsize_neg() {
        test_case(
            "
            PUSHINT -5
            QUBITSIZE",
        )
        .expect_stack(Stack::new().push(int!(nan)));
    }

    #[test]
    fn test_ubitsize_nan() {
        test_case(
            "
            PUSHNAN
            UBITSIZE
        ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_qubitsize_nan() {
        test_case(
            "
            PUSHNAN
            QUBITSIZE",
        )
        .expect_item(int!(nan));
    }
}
