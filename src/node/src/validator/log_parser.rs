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
#![allow(dead_code)]

extern crate regex;
use self::regex::{escape, Regex};
use std::{fmt::Debug, str::FromStr};

pub struct LogParser {
    log: String,
}

impl LogParser {
    pub fn new(s: &str) -> Self {
        let mut prepared_str = " ".to_string();
        prepared_str.push_str(s);
        prepared_str.push(' ');
        LogParser { log: prepared_str }
    }

    pub fn get_field(&self, name: &str) -> Option<String> {
        let it = Regex::new(&(format!(" {} = ([^ ]*) ", escape(name)))).unwrap();
        it.captures(&self.log).map(|group| group.get(1).unwrap().as_str().to_string())
    }

    pub fn parse_field_fromstr<T>(&self, name: &str) -> T
    where
        T: FromStr,
        T::Err: Debug,
    {
        match self.get_field(name) {
            None => panic!("Cannot find field `{}`", name),
            Some(v) => T::from_str(&v).unwrap(),
        }
    }

    pub fn get_field_count(&self, name: &str) -> u32 {
        let it = Regex::new(&(format!(r" {}\.(\d+)[. ]", escape(name)))).unwrap();
        let mut fields = 0;
        for nums in it.captures_iter(&self.log) {
            let fnew = u32::from_str(&nums[1]).unwrap();
            if fnew >= fields {
                fields = fnew + 1;
            }
        }
        fields
    }

    pub fn parse_slice(&self, name: &str) -> ::ton_api::ton::bytes {
        let data = self.get_field(name).unwrap();
        hex::decode(data).unwrap()
    }
}

#[cfg(test)]
#[path = "tests/test_log_parser.rs"]
mod tests;
