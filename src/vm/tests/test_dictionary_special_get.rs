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
    int,
    stack::{integer::IntegerData, StackItem},
};

// PUSHINT 10 -> 0x7A
// PUSHINT 12 -> 0x7C
const CREATE_DICTU_INSTRUCTIONS: &str = "
    PUSHSLICE x7A8_
    PUSHINT 1
    NEWDICT
    PUSHINT 8
    DICTUSET
    PUSHSLICE x800C8_
    SWAP
    PUSHINT 2
    SWAP
    PUSHINT 8
    DICTUSET
";

const CREATE_DICTI_INSTRUCTIONS: &str = "
    PUSHSLICE x7A8_
    PUSHINT -1
    NEWDICT
    PUSHINT 8
    DICTISET
    PUSHSLICE x800C8_
    SWAP
    PUSHINT -2
    SWAP
    PUSHINT 8
    DICTISET
";

mod dictugetjmp {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHINT 1
            {}
            PUSHINT 8
            DICTUGETJMP ",
            CREATE_DICTU_INSTRUCTIONS
        ))
        .expect_item(int!(10));

        test_case(format!(
            "
            PUSHINT 2
            {}
            PUSHINT 8
            DICTUGETJMP ",
            CREATE_DICTU_INSTRUCTIONS
        ))
        .expect_item(int!(12));
    }
}

mod dictugetexec {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHINT 1
            {}
            PUSHINT 8
            DICTUGETEXEC",
            CREATE_DICTU_INSTRUCTIONS
        ))
        .expect_item(int!(10));

        test_case(format!(
            "
            PUSHINT 2
            {}
            PUSHINT 8
            DICTUGETEXEC ",
            CREATE_DICTU_INSTRUCTIONS
        ))
        .expect_item(int!(12));
    }
}

mod dictigetjmp {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHINT -1
            {}
            PUSHINT 8
            DICTIGETJMP ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(10));

        test_case(format!(
            "
            PUSHINT -2
            {}
            PUSHINT 8
            DICTIGETJMP ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(12));
    }

    #[test]
    fn test_failure_flow() {
        test_case(format!(
            "
            PUSHINT 666
            PUSHINT -3
            {}
            PUSHINT 8
            DICTIGETJMP",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(666));
    }
}

mod dictigetexec {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHINT -1
            {}
            PUSHINT 8
            DICTIGETEXEC",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(10));

        test_case(format!(
            "
            PUSHINT -2
            {}
            PUSHINT 8
            DICTIGETEXEC ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(12));
    }

    #[test]
    fn test_failure_flow() {
        test_case(format!(
            "
            PUSHINT 666
            PUSHINT -3
            {}
            PUSHINT 8
            DICTIGETEXEC",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(666));
    }
}

mod dictigetjmpz {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHINT -1
            {}
            PUSHINT 8
            DICTIGETJMPZ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(10));

        test_case(format!(
            "
            PUSHINT -2
            {}
            PUSHINT 8
            DICTIGETJMPZ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(12));
    }

    #[test]
    fn test_failure_flow() {
        test_case(format!(
            "
            PUSHINT -3
            {}
            PUSHINT 8
            DICTIGETJMPZ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(-3));
    }
}

mod dictigetexecz {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHINT -1
            {}
            PUSHINT 8
            DICTIGETEXECZ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(10));

        test_case(format!(
            "
            PUSHINT -2
            {}
            PUSHINT 8
            DICTIGETEXECZ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(12));
    }

    #[test]
    fn test_failure_flow() {
        test_case(format!(
            "
            PUSHINT -3
            {}
            PUSHINT 8
            DICTIGETEXECZ",
            CREATE_DICTI_INSTRUCTIONS
        ))
        .expect_item(int!(-3));
    }
}
