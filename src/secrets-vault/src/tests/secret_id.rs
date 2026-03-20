/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{make_secret_id, types::secret_id::SecretId};

#[test]
fn test_empty_string() {
    assert_eq!(SecretId::escape(""), "");
}

#[test]
fn test_no_special_characters() {
    assert_eq!(SecretId::escape("hello world"), "hello world");
    assert_eq!(SecretId::escape("abc123"), "abc123");
    assert_eq!(SecretId::escape("Test String!"), "Test String!");
}

#[test]
fn test_escape_forward_slash() {
    assert_eq!(SecretId::escape("."), "\\.");
    assert_eq!(SecretId::escape("a.b"), "a\\.b");
    assert_eq!(SecretId::escape(".path.to.file"), "\\.path\\.to\\.file");
}

#[test]
fn test_escape_backslash() {
    assert_eq!(SecretId::escape("\\"), "\\\\");
    assert_eq!(SecretId::escape("a\\b"), "a\\\\b");
    assert_eq!(SecretId::escape("C:\\Users\\Name"), "C:\\\\Users\\\\Name");
}

#[test]
fn test_escape_both_slashes() {
    assert_eq!(SecretId::escape(".\\"), "\\.\\\\");
    assert_eq!(SecretId::escape("\\."), "\\\\\\.");
    assert_eq!(SecretId::escape("a.b\\c"), "a\\.b\\\\c");
    assert_eq!(SecretId::escape("path.to\\file"), "path\\.to\\\\file");
}

#[test]
fn test_multiple_consecutive_slashes() {
    assert_eq!(SecretId::escape(".."), "\\.\\.");
    assert_eq!(SecretId::escape("\\\\"), "\\\\\\\\");
    assert_eq!(SecretId::escape("..."), "\\.\\.\\.");
    assert_eq!(SecretId::escape("\\\\\\"), "\\\\\\\\\\\\");
}

#[test]
fn test_slashes_at_boundaries() {
    assert_eq!(SecretId::escape(".start"), "\\.start");
    assert_eq!(SecretId::escape("end."), "end\\.");
    assert_eq!(SecretId::escape("\\start"), "\\\\start");
    assert_eq!(SecretId::escape("end\\"), "end\\\\");
}

#[test]
fn test_mixed_content() {
    assert_eq!(SecretId::escape("Hello.World\\Test"), "Hello\\.World\\\\Test");
    assert_eq!(
        SecretId::escape("http:..example.com.path\\file.txt"),
        "http:\\.\\.example\\.com\\.path\\\\file\\.txt"
    );
}

#[test]
fn test_unicode_characters() {
    assert_eq!(SecretId::escape("café"), "café");
    assert_eq!(SecretId::escape("emoji 😀"), "emoji 😀");
}

#[test]
fn test_special_characters_except_slashes() {
    assert_eq!(SecretId::escape("a@b#c$d"), "a@b#c$d");
    assert_eq!(SecretId::escape("tab\there"), "tab\there");
    assert_eq!(SecretId::escape("new\nline"), "new\nline");
    assert_eq!(SecretId::escape("quote\"test"), "quote\"test");
}

#[test]
fn test_capacity_hint() {
    let long_string = "a".repeat(1000);
    assert_eq!(SecretId::escape(&long_string), long_string);

    let long_with_slashes = ".".repeat(1000);
    let expected = "\\.".repeat(1000);
    assert_eq!(SecretId::escape(&long_with_slashes), expected);
}

#[test]
fn test_alternating_slashes() {
    assert_eq!(SecretId::escape(".\\.\\.\\"), "\\.\\\\\\.\\\\\\.\\\\");
    assert_eq!(SecretId::escape("\\.\\.\\."), "\\\\\\.\\\\\\.\\\\\\.");
}

#[test]
fn test_real_world_paths() {
    assert_eq!(SecretId::escape(".usr.local.bin"), "\\.usr\\.local\\.bin");
    assert_eq!(SecretId::escape("C:\\Program Files\\App"), "C:\\\\Program Files\\\\App");
    assert_eq!(SecretId::escape(".home.user.file.txt"), "\\.home\\.user\\.file\\.txt");
}

#[test]
fn test_json_like_strings() {
    assert_eq!(SecretId::escape(r#"{"path": ".data"}"#), r#"{"path": "\.data"}"#);
    assert_eq!(SecretId::escape(r#"{"windows": "C:\"}"#), r#"{"windows": "C:\\"}"#);
}

#[test]
fn test_secret_id_macro_single_arg() {
    let id = make_secret_id!("private_keys");
    assert_eq!(id.as_str(), "private_keys");
}

#[test]
fn test_secret_id_macro_two_args() {
    let key_id = "my_key";
    let id = make_secret_id!("private_keys", key_id);
    assert_eq!(id.as_str(), "private_keys.my_key");
}

#[test]
fn test_secret_id_macro_multiple_args() {
    let id = make_secret_id!("private_keys", "user_123", "nested", "path");
    assert_eq!(id.as_str(), "private_keys.user_123.nested.path");
}

#[test]
fn test_secret_id_macro_with_numbers() {
    let user_id = 42;
    let id = make_secret_id!("users", user_id, "secrets");
    assert_eq!(id.as_str(), "users.42.secrets");
}

#[test]
fn test_secret_id_macro_escapes_forward_slash() {
    let path = "some.path";
    let id = make_secret_id!("prefix", path);
    assert_eq!(id.as_str(), "prefix.some\\.path");
}

#[test]
fn test_secret_id_macro_escapes_backslash() {
    let path = "some\\path";
    let id = make_secret_id!("prefix", path);
    assert_eq!(id.as_str(), "prefix.some\\\\path");
}

#[test]
fn test_secret_id_macro_escapes_both_slashes() {
    let path = "some.path\\with\\both";
    let id = make_secret_id!("root", path, "end");
    assert_eq!(id.as_str(), "root.some\\.path\\\\with\\\\both.end");
}

#[test]
fn test_secret_id_macro_with_trailing_comma() {
    let id = make_secret_id!("private_keys", "my_key",);
    assert_eq!(id.as_str(), "private_keys.my_key");
}

#[test]
fn test_secret_id_macro_with_string_types() {
    let owned = String::from("owned");
    let borrowed = "borrowed";
    let id = make_secret_id!("prefix", owned, borrowed);
    assert_eq!(id.as_str(), "prefix.owned.borrowed");
}

#[test]
fn test_secret_id_macro_empty_string() {
    let id = make_secret_id!("prefix", "", "suffix");
    assert_eq!(id.as_str(), "prefix..suffix");
}

#[test]
fn test_secret_id_macro_with_uuid() {
    let uuid = uuid::Uuid::new_v4();
    let id = make_secret_id!("keys", uuid);
    assert_eq!(id.as_str(), format!("keys.{}", uuid));
}

#[test]
fn test_secret_id_macro_special_characters() {
    let special = "test@#$%^&*()";
    let id = make_secret_id!("prefix", special);
    assert_eq!(id.as_str(), "prefix.test@#$%^&*()");
}

#[test]
fn test_secret_id_macro_complex_escaping() {
    let complex = "path.with\\mixed.slashes\\here";
    let id = make_secret_id!("root", complex, "end");
    assert_eq!(id.as_str(), "root.path\\.with\\\\mixed\\.slashes\\\\here.end");
}

#[test]
fn test_secret_id_macro_with_expressions() {
    let base = "keys";
    let index = 5;
    let id = make_secret_id!(base, index + 10, "final");
    assert_eq!(id.as_str(), "keys.15.final");
}
