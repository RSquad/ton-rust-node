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
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, StackItem},
};

mod sempty {
    use super::*;

    #[test]
    fn test_empty_slice() {
        test_case(
            "
            PUSHSLICE x8_
            SEMPTY",
        )
        .expect_bytecode(vec![0x8B, 0x08, 0xC7, 0x00, 0x80])
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_slice_with_1_bit_of_data() {
        test_case(
            "
            PUSHSLICE x4_
            SEMPTY",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_slice_with_1_reference() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            SEMPTY",
        )
        .expect_item(boolean!(false));
    }
}

mod sdempty {
    use super::*;

    #[test]
    fn test_empty_slice() {
        test_case(
            "
            PUSHSLICE x8_
            SDEMPTY",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_slice_with_1_bit_of_data() {
        test_case(
            "
            PUSHSLICE x4_
            SDEMPTY",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_slice_with_1_reference() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            SDEMPTY",
        )
        .expect_item(boolean!(true));
    }
}

mod srempty {
    use super::*;

    #[test]
    fn test_empty_slice() {
        test_case(
            "
            PUSHSLICE x8_
            SREMPTY",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_slice_with_1_bit_of_data() {
        test_case(
            "
            PUSHSLICE x4_
            SREMPTY",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_slice_with_1_reference() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            SREMPTY",
        )
        .expect_item(boolean!(false));
    }
}

mod sdeq {
    use super::*;

    #[test]
    fn test_empty_slices() {
        test_case(
            "
            PUSHSLICE x8_
            PUSHSLICE x8_
            SDEQ",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_compare_empty_and_1_bit_of_data() {
        test_case(
            "
            PUSHSLICE x4_
            PUSHSLICE x8_
            SDEQ",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_compare_slice_with_1_reference_and_empty() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            PUSHSLICE x8_
            SDEQ",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_shifted_slices() {
        test_case(
            "
            PUSHSLICE x6_
            SDBEGINS x4_
            PUSHSLICE xF_
            SDBEGINS xE_
            SDEQ",
        )
        .expect_item(boolean!(true));
    }
}

mod sdlexcmp {
    use super::*;

    #[test]
    fn test_empty_slices() {
        test_case(
            "
            PUSHSLICE x8_
            PUSHSLICE x8_
            SDLEXCMP",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_compare_empty_and_1_bit_of_data() {
        test_case(
            "
            PUSHSLICE x4_
            PUSHSLICE x8_
            SDLEXCMP",
        )
        .expect_item(int!(1));
    }

    #[test]
    fn test_compare_1_bit_of_data_and_empty() {
        test_case(
            "
            PUSHSLICE x8_
            PUSHSLICE x4_
            SDLEXCMP",
        )
        .expect_item(int!(-1));
    }

    #[test]
    fn test_compare_slice_with_1_reference_and_empty() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            PUSHSLICE x8_
            SDLEXCMP",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_shifted_slices_less() {
        test_case(
            "
            PUSHSLICE x6_
            PUSHSLICE xF_
            SDLEXCMP
        ",
        )
        .expect_item(int!(-1));
    }

    #[test]
    fn test_shifted_slices_greater() {
        test_case(
            "
            PUSHSLICE xF_
            PUSHSLICE x6_
            SDLEXCMP
        ",
        )
        .expect_item(int!(1));
    }

    #[test]
    fn test_slices_less() {
        test_case(
            "
            PUSHSLICE xC_
            PUSHSLICE xF_
            SDLEXCMP
        ",
        )
        .expect_item(int!(-1));
    }

    #[test]
    fn test_slices_greater() {
        test_case(
            "
            PUSHSLICE xF_
            PUSHSLICE xC_
            SDLEXCMP
        ",
        )
        .expect_item(int!(1));
    }
}

mod sdcntlead0 {
    use super::*;

    #[test]
    fn test_bitstring_with_one_zero() {
        test_case(
            "
            PUSHSLICE x4_
            SDCNTLEAD0
            ",
        )
        .expect_item(int!(1));
    }
}

mod sdcntlead1 {
    use super::*;

    #[test]
    fn test_bitstring_with_one_zero() {
        test_case(
            "
            PUSHSLICE x4_
            SDCNTLEAD1
            ",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_bitstring_with_a_one() {
        test_case(
            "
            PUSHSLICE xC_
            SDCNTLEAD1
            ",
        )
        .expect_item(int!(1));
    }
}

mod sdcnttrail0 {
    use super::*;

    #[test]
    fn test_bitstring_with_one_zero() {
        test_case(
            "
            PUSHSLICE x4_
            SDCNTTRAIL0
            ",
        )
        .expect_item(int!(1));
    }

    #[test]
    fn test_bitstring_with_a_one() {
        test_case(
            "
            PUSHSLICE xC_
            SDCNTTRAIL0
            ",
        )
        .expect_item(int!(0));
    }
}

mod sdcnttrail1 {
    use super::*;

    #[test]
    fn test_bitstring_with_one_zero() {
        test_case(
            "
            PUSHSLICE x4_
            SDCNTTRAIL1
            ",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn test_bitstring_with_a_one() {
        test_case(
            "
            PUSHSLICE xC_
            SDCNTTRAIL1
            ",
        )
        .expect_item(int!(1));
    }
}

mod sdpfx {
    use super::*;

    #[test]
    fn test_prefix_exist() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x55
            SDPFX",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_prefix_non_exist() {
        test_case(
            "
            PUSHSLICE x55
            PUSHSLICE x5_
            SDPFX",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_equal_args() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x5_
            SDPFX",
        )
        .expect_item(boolean!(true));
    }
}

mod sdpfxrev {
    use super::*;

    #[test]
    fn test_prefix_exist() {
        test_case(
            "
            PUSHSLICE x55
            PUSHSLICE x5_
            SDPFXREV",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_prefix_non_exist() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x55
            SDPFXREV",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_equal_args() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x5_
            SDPFXREV",
        )
        .expect_item(boolean!(true));
    }
}

mod sdppfx {
    use super::*;

    #[test]
    fn test_proper_prefix_exist() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x55
            SDPPFX",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_proper_prefix_non_exist() {
        test_case(
            "
            PUSHSLICE x55
            PUSHSLICE x5_
            SDPPFX",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_equal_args() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x5_
            SDPPFX",
        )
        .expect_item(boolean!(false));
    }
}

mod sdppfxrev {
    use super::*;

    #[test]
    fn test_proper_prefix_exist() {
        test_case(
            "
            PUSHSLICE x55
            PUSHSLICE x5_
            SDPPFXREV",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_proper_prefix_non_exist() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x55
            SDPPFXREV",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_equal_args() {
        test_case(
            "
            PUSHSLICE x5_
            PUSHSLICE x5_
            SDPPFX",
        )
        .expect_item(boolean!(false));
    }
}

mod sdfirst {
    use super::*;

    #[test]
    fn test_empty_slice() {
        test_case(
            "
            PUSHSLICE x8_
            SDFIRST",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_slice_with_zero_bit_of_data() {
        test_case(
            "
            PUSHSLICE x4_
            SDFIRST",
        )
        .expect_item(boolean!(false));
    }

    #[test]
    fn test_slice_with_one_bit_of_data() {
        test_case(
            "
            PUSHSLICE xC_
            SDFIRST",
        )
        .expect_item(boolean!(true));
    }
}

mod sd_suffix {
    use super::*;

    #[test]
    fn test_suffix_simple() {
        test_case(
            "
            PUSHSLICE x4
            PUSHSLICE xF4
            SDSFX
            ",
        )
        .expect_item(boolean!(true));

        test_case(
            "
            PUSHSLICE xF4
            PUSHSLICE x4
            SDSFXREV
            ",
        )
        .expect_item(boolean!(true));
    }

    #[test]
    fn test_suffix_proper() {
        test_case(
            "
            PUSHSLICE x4
            PUSHSLICE xF4
            SDPSFX
            ",
        )
        .expect_item(boolean!(true));

        test_case(
            "
            PUSHSLICE xF4
            PUSHSLICE x4
            SDPSFXREV
            ",
        )
        .expect_item(boolean!(true));
    }
}
