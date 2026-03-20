/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

#[test]
fn test_to_snake_case() {
    assert_eq!(to_snake_case("test"), "test");
    assert_eq!(to_snake_case("foo_bar"), "foo_bar");
    assert_eq!(to_snake_case("test_"), "test_");
    assert_eq!(to_snake_case("_test_"), "_test_");
    assert_eq!(to_snake_case("_test"), "_test");
    assert_eq!(to_snake_case("Test"), "test");
    assert_eq!(to_snake_case("TestMsg"), "test_msg");
    assert_eq!(to_snake_case("TestA"), "test_a");
    assert_eq!(to_snake_case("TTest"), "t_test");
}
