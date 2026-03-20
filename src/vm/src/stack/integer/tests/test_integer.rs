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
mod test_equal {

    use crate::stack::integer::IntegerData;

    #[test]
    fn test_same_positive_values() {
        assert_eq!(IntegerData::one(), IntegerData::from_u32(1));
    }

    #[test]
    fn test_same_negative_values() {
        assert_eq!(IntegerData::minus_one(), IntegerData::from_i32(-1));
    }

    #[test]
    fn test_different_positive_values() {
        assert_ne!(IntegerData::from_u32(2), IntegerData::one());
    }

    #[test]
    fn test_different_negative_values() {
        assert_ne!(IntegerData::from_i32(-2), IntegerData::minus_one());
    }

    #[test]
    fn test_zeros_comparison() {
        assert_eq!(IntegerData::zero(), IntegerData::from_u32(0));
    }

    #[test]
    fn test_different_sign_same_modulus() {
        assert_ne!(IntegerData::minus_one(), IntegerData::one());
    }
}

mod test_bitsize {

    use crate::stack::integer::IntegerData;

    #[test]
    fn ubitsize_pos() {
        assert_eq!(IntegerData::zero().ubitsize().unwrap(), 0);
        assert_eq!(IntegerData::one().ubitsize().unwrap(), 1);
        assert_eq!(IntegerData::from_u32(2).ubitsize().unwrap(), 2);
        assert_eq!(IntegerData::from_u32(3).ubitsize().unwrap(), 2);
        assert_eq!(IntegerData::from_u32(4).ubitsize().unwrap(), 3);
    }

    #[test]
    fn bitsize_pos() {
        assert_eq!(IntegerData::zero().bitsize().unwrap(), 1, "0");
        assert_eq!(IntegerData::one().bitsize().unwrap(), 2, "1");
        assert_eq!(IntegerData::from_u32(2).bitsize().unwrap(), 3, "2");
        assert_eq!(IntegerData::from_u32(3).bitsize().unwrap(), 3, "3");
        assert_eq!(IntegerData::from_u32(4).bitsize().unwrap(), 4, "4");
    }

    #[test]
    fn bitsize_neg() {
        assert_eq!(IntegerData::minus_one().bitsize().unwrap(), 1, "-1");
        assert_eq!(IntegerData::from_i32(-2).bitsize().unwrap(), 2, "-2");
        assert_eq!(IntegerData::from_i32(-3).bitsize().unwrap(), 3, "-3");
        assert_eq!(IntegerData::from_i32(-4).bitsize().unwrap(), 3, "-4");
        assert_eq!(IntegerData::from_i32(-5).bitsize().unwrap(), 4, "-5");
        assert_eq!(IntegerData::from_i32(-6).bitsize().unwrap(), 4, "-6");
        assert_eq!(IntegerData::from_i32(-7).bitsize().unwrap(), 4, "-7");
        assert_eq!(IntegerData::from_i32(-8).bitsize().unwrap(), 4, "-8");
    }
}

mod test_minus_2_pow_256 {

    use crate::stack::integer::{behavior::Signaling, IntegerData};
    use ton_block::{ExceptionCode, Result};

    #[test]
    fn test_2_pow_256_overflows() {
        assert_eq!(
            crate::error::tvm_exception_code(
                &IntegerData::from_str_radix(
                    "1_00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000",
                    16
                )
                .expect_err("Integer overflow is expected")
            ),
            Some(ExceptionCode::IntegerOverflow)
        );
    }

    #[test]
    fn test_minus_2_pow_256_negate_overflows() {
        let value = create_minus_2_pow_256();
        assert!(value.is_neg());
        assert_eq!(
            crate::error::tvm_exception_code(
                &value.neg::<Signaling>().expect_err("Integer overflow is expected")
            ),
            Some(ExceptionCode::IntegerOverflow)
        );
    }

    #[test]
    fn test_minus_2_pow_256_plus_one_negate_ok() -> Result<()> {
        let value = create_minus_2_pow_256().add::<Signaling>(&IntegerData::one())?;
        assert!(value.is_neg());
        value.neg::<Signaling>()?;
        Ok(())
    }

    #[test]
    fn test_minus_2_pow_256_minus_one_overflows() {
        let value = create_minus_2_pow_256();
        assert!(value.is_neg());
        assert_eq!(
            crate::error::tvm_exception_code(
                &value
                    .sub::<Signaling>(&IntegerData::one())
                    .expect_err("Integer overflow is expected")
            ),
            Some(ExceptionCode::IntegerOverflow)
        );
    }

    fn create_minus_2_pow_256() -> IntegerData {
        IntegerData::from_str_radix(
            "-1_00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000",
            16,
        )
        .unwrap()
    }
}

mod test_behavior {

    use crate::stack::integer::{
        behavior::{Quiet, Signaling},
        IntegerData,
    };

    #[test]
    fn add_quiet_vs_signaling() {
        let x = IntegerData::nan();
        let y = IntegerData::one();
        x.add::<Signaling>(&y).expect_err("Adding a NaN with a number should fail.");
        assert!(
            x.add::<Quiet>(&y)
                .expect("Adding a NaN with a number doesn't fail in quiet version.")
                .is_nan(),
            "Result value should be a NaN"
        );
    }
}

mod test_bitlogics {

    use crate::stack::integer::{behavior::Signaling, IntegerData};

    fn test_and(x: i64, y: i64) {
        let xdst = IntegerData::from_i64(x);
        let ydst = IntegerData::from_i64(y);
        assert_eq!(IntegerData::from_i64(x & y), xdst.and::<Signaling>(&ydst).unwrap());
    }

    fn test_or(x: i64, y: i64) {
        let xdst = IntegerData::from_i64(x);
        let ydst = IntegerData::from_i64(y);
        assert_eq!(IntegerData::from_i64(x | y), xdst.or::<Signaling>(&ydst).unwrap());
    }

    fn test_xor(x: i64, y: i64) {
        let xdst = IntegerData::from_i64(x);
        let ydst = IntegerData::from_i64(y);
        assert_eq!(IntegerData::from_i64(x ^ y), xdst.xor::<Signaling>(&ydst).unwrap());
    }

    fn test_not(x: i64) {
        let xdst = IntegerData::from_i64(x);
        assert_eq!(IntegerData::from_i64(!x), xdst.not::<Signaling>().unwrap());
    }

    fn test_shl(x: i64, shift: usize) {
        let xdst = IntegerData::from_i64(x);
        assert_eq!(IntegerData::from_i64(x << shift as i64), xdst.shl::<Signaling>(shift).unwrap());
    }

    fn test_shr(x: i64, shift: usize) {
        let xdst = IntegerData::from_i64(x);
        assert_eq!(IntegerData::from_i64(x >> shift as i64), xdst.shr::<Signaling>(shift).unwrap());
    }

    #[test]
    fn test_positive_and_positive() {
        test_and(0xF0FF, 0xF0F0);
    }

    #[test]
    fn test_positive_and_negative() {
        test_and(0xF0FF, -0xF0F0);
    }

    #[test]
    fn test_negative_and_positive() {
        test_and(-0xF0FF, 0xF0F0);
    }

    #[test]
    fn test_negative_and_negative() {
        test_and(-0xF0FF, -0xF0F0);
    }

    #[test]
    fn test_positive_or_positive() {
        test_or(0xF0FF, 0xF0F0);
    }

    #[test]
    fn test_positive_or_negative() {
        test_or(0xF0FF, -0xF0F0);
    }

    #[test]
    fn test_negative_or_positive() {
        test_or(-0xF0FF, 0xF0F0);
    }

    #[test]
    fn test_negative_or_negative() {
        test_or(-0xF0FF, -0xF0F0);
    }

    #[test]
    fn test_positive_xor_positive() {
        test_xor(0xF0FF, 0xF0F0);
    }

    #[test]
    fn test_positive_xor_negative() {
        test_xor(0xF0FF, -0xF0F0);
    }

    #[test]
    fn test_negative_xor_positive() {
        test_xor(-0xF0FF, 0xF0F0);
    }

    #[test]
    fn test_negative_xor_negative() {
        test_xor(-0xF0FF, -0xF0F0);
    }

    #[test]
    fn test_not_positive() {
        test_not(0xF0FF);
    }

    #[test]
    fn test_not_negative() {
        test_not(-0xF0F0);
    }

    #[test]
    fn test_shl_positive() {
        test_shl(1, 1);
        test_shl(1, 0);
        test_shl(0, 0);
        test_shl(0, 1);
        test_shl(12, 5);
    }

    #[test]
    fn test_shl_negative() {
        test_shl(-1, 1);
        test_shl(-1, 0);
        test_shl(-12, 5);
    }

    #[test]
    fn test_shr_positive() {
        test_shr(1, 1);
        test_shr(1, 0);
        test_shr(0, 0);
        test_shr(0, 1);
        test_shr(12, 5);
    }

    #[test]
    fn test_shr_negative() {
        test_shr(-1, 1);
        test_shr(-1, 0);
        test_shr(-12, 5);
    }
}
