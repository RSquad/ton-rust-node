/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{
    fmt::{Display, Formatter},
    str::{from_utf8, FromStr},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SecretId {
    value: String,
}

impl SecretId {
    pub fn new(secret_id: impl Into<SecretId>) -> Self {
        secret_id.into()
    }

    pub fn new_rand_uuid() -> Self {
        Self { value: uuid::Uuid::new_v4().to_string() }
    }

    pub fn from_uuid(uuid: uuid::Uuid) -> Self {
        Self { value: uuid.to_string() }
    }

    pub fn as_str(&self) -> &str {
        self.value.as_str()
    }

    pub fn as_string(&self) -> &str {
        self.value.as_str()
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.value.as_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self, std::str::Utf8Error> {
        let s = from_utf8(bytes)?;
        Ok(Self { value: s.to_owned() })
    }

    pub fn escape(s: &str) -> String {
        let mut escaped = String::with_capacity(s.len() * 2);

        for ch in s.chars() {
            match ch {
                '.' => escaped.push_str("\\."),
                '\\' => escaped.push_str("\\\\"),
                _ => escaped.push(ch),
            }
        }

        escaped
    }
}

impl Default for SecretId {
    fn default() -> Self {
        Self::new_rand_uuid()
    }
}

impl Display for SecretId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.value)
    }
}

impl FromStr for SecretId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self { value: s.to_owned() })
    }
}

impl From<&String> for SecretId {
    fn from(s: &String) -> Self {
        Self { value: s.clone() }
    }
}

impl From<String> for SecretId {
    fn from(s: String) -> Self {
        Self { value: s }
    }
}

impl From<&str> for SecretId {
    fn from(s: &str) -> Self {
        s.parse().unwrap()
    }
}

impl From<&SecretId> for SecretId {
    fn from(s: &SecretId) -> SecretId {
        s.clone()
    }
}

impl From<SecretId> for String {
    fn from(k: SecretId) -> Self {
        k.value
    }
}

impl From<&SecretId> for String {
    fn from(k: &SecretId) -> String {
        k.to_string()
    }
}

#[macro_export]
macro_rules! make_secret_id {
    ($($arg:expr),+ $(,)?) => {{
        let parts: Vec<String> = vec![
            $($crate::types::secret_id::SecretId::escape(&$arg.to_string())),+
        ];
        $crate::types::secret_id::SecretId::from(parts.join("."))
    }};
}
