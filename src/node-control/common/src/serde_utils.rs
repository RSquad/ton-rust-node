/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod account_status_as_str {
    use std::fmt::Formatter;
    use ton_block::AccountStatus;

    pub fn serialize<S>(v: &AccountStatus, s: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let name = match v {
            AccountStatus::AccStateUninit => "uninit",
            AccountStatus::AccStateFrozen => "frozen",
            AccountStatus::AccStateActive => "active",
            AccountStatus::AccStateNonexist => "nonexist",
        };
        s.serialize_str(name)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<AccountStatus, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = AccountStatus;

            fn expecting(&self, f: &mut Formatter) -> std::fmt::Result {
                f.write_str(r#"a status string: "uninit" | "frozen" | "active" | "nonexist""#)
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    // accept a few aliases just in case
                    "uninit" | "AccStateUninit" => Ok(AccountStatus::AccStateUninit),
                    "frozen" | "AccStateFrozen" => Ok(AccountStatus::AccStateFrozen),
                    "active" | "AccStateActive" => Ok(AccountStatus::AccStateActive),
                    "nonexist" | "AccStateNonexist" => Ok(AccountStatus::AccStateNonexist),
                    _ => Err(E::invalid_value(
                        serde::de::Unexpected::Str(v),
                        &r#""uninit", "frozen", "active", or "nonexist""#,
                    )),
                }
            }
        }

        d.deserialize_any(V)
    }
}

pub mod b64 {
    use base64::Engine;
    use serde::Deserialize;

    pub fn serialize<S>(bytes: &[u8], ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<Vec<u8>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

pub mod option_b64 {
    use base64::Engine;
    use serde::Deserialize;

    pub fn serialize<S>(value: &Option<Vec<u8>>, ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match value {
            Some(bytes) => {
                ser.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
            }
            None => ser.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<Option<Vec<u8>>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(de)?;
        match opt {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

pub mod i64_as_str {
    use serde::Deserialize;

    pub fn serialize<S>(num: &i64, ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&num.to_string())
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<i64, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        s.parse::<i64>().map_err(serde::de::Error::custom)
    }
}

pub mod u64_as_str {
    use serde::Deserialize;

    pub fn serialize<S>(num: &u64, ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&num.to_string())
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<u64, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        s.parse::<u64>().map_err(serde::de::Error::custom)
    }
}

pub mod u64_as_str_or_num {
    use std::fmt::Formatter;

    pub fn serialize<S>(num: &u64, ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&num.to_string())
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<u64, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = u64;

            fn expecting(&self, f: &mut Formatter) -> std::fmt::Result {
                f.write_str("a u64 as a string or number")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<u64, E> {
                v.parse::<u64>().map_err(E::custom)
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<u64, E> {
                Ok(v)
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<u64, E> {
                u64::try_from(v).map_err(E::custom)
            }
        }

        de.deserialize_any(V)
    }

    #[cfg(test)]
    mod tests {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Wrapper {
            #[serde(with = "super")]
            value: u64,
        }

        #[test]
        fn deserializes_string() {
            let w: Wrapper = serde_json::from_str(r#"{"value":"12345"}"#).unwrap();
            assert_eq!(w.value, 12345);
        }

        #[test]
        fn deserializes_number() {
            let w: Wrapper = serde_json::from_str(r#"{"value":12345}"#).unwrap();
            assert_eq!(w.value, 12345);
        }

        #[test]
        fn deserializes_zero_as_string() {
            let w: Wrapper = serde_json::from_str(r#"{"value":"0"}"#).unwrap();
            assert_eq!(w.value, 0);
        }

        #[test]
        fn deserializes_zero_as_number() {
            let w: Wrapper = serde_json::from_str(r#"{"value":0}"#).unwrap();
            assert_eq!(w.value, 0);
        }

        #[test]
        fn deserializes_max_as_string() {
            let input = format!(r#"{{"value":"{}"}}"#, u64::MAX);
            let w: Wrapper = serde_json::from_str(&input).unwrap();
            assert_eq!(w.value, u64::MAX);
        }

        #[test]
        fn deserializes_max_as_number() {
            let input = format!(r#"{{"value":{}}}"#, u64::MAX);
            let w: Wrapper = serde_json::from_str(&input).unwrap();
            assert_eq!(w.value, u64::MAX);
        }

        #[test]
        fn serializes_as_string() {
            let w = Wrapper { value: 12345 };
            assert_eq!(serde_json::to_string(&w).unwrap(), r#"{"value":"12345"}"#);
        }

        #[test]
        fn rejects_negative_number() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":-1}"#).is_err(),
                "expected error for negative number"
            );
        }

        #[test]
        fn rejects_negative_string() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":"-1"}"#).is_err(),
                "expected error for negative string"
            );
        }

        #[test]
        fn rejects_overflow_string() {
            // u64::MAX + 1
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":"18446744073709551616"}"#).is_err(),
                "expected error for overflow string"
            );
        }

        #[test]
        fn rejects_float() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":1.5}"#).is_err(),
                "expected error for float"
            );
        }

        #[test]
        fn rejects_empty_string() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":""}"#).is_err(),
                "expected error for empty string"
            );
        }

        #[test]
        fn rejects_non_numeric_string() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":"abc"}"#).is_err(),
                "expected error for non-numeric string"
            );
        }

        #[test]
        fn rejects_hex_string() {
            // only decimal strings are accepted
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":"0x1A"}"#).is_err(),
                "expected error for hex string"
            );
        }

        #[test]
        fn rejects_null() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":null}"#).is_err(),
                "expected error for null"
            );
        }

        #[test]
        fn rejects_bool() {
            assert!(
                serde_json::from_str::<Wrapper>(r#"{"value":true}"#).is_err(),
                "expected error for bool"
            );
        }
    }
}

pub mod hex_string {
    use serde::Deserialize;

    pub fn serialize<S>(bytes: &[u8], ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<Vec<u8>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        hex::decode(&s).map_err(serde::de::Error::custom)
    }
}

pub mod base64_key {
    use serde::Deserialize;

    pub fn serialize<S>(bytes: &[u8], ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            bytes,
        ))
    }

    pub fn deserialize<'de, D>(de: D) -> std::result::Result<Vec<u8>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

pub mod serde_level {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(level: &tracing::Level, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(level.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<tracing::Level, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<tracing::Level>().map_err(serde::de::Error::custom)
    }
}
