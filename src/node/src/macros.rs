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
#[macro_export]
macro_rules! dump {
    ($data: expr) => {
        {
            let mut dump = String::new();
            for i in 0..$data.len() {
                dump.push_str(
                    &format!(
                        "{:02x}{}",
                        $data[i],
                        if (i + 1) % 16 == 0 { '\n' } else { ' ' }
                    )
                )
            }
            dump
        }
    };
    (debug, $target:expr, $msg:expr, $data:expr) => {
        if log::log_enabled!(log::Level::Debug) {
            log::debug!(target: $target, "{}:\n{}", $msg, dump!($data))
        }
    };
    (trace, $target:expr, $msg:expr, $data:expr) => {
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(target: $target, "{}:\n{}", $msg, dump!($data))
        }
    }
}

#[macro_export]
macro_rules! CHECK {
    ($exp:expr) => {
        // TODO: remove for production
        if !($exp) {
            ton_block::fail!("{} {}:{}", stringify!($exp), file!(), line!())
        }
    };
    ($exp:expr, inited) => {
        // TODO: remove for production
        if $exp == &Default::default() {
            ton_block::fail!("{} {}:{}", stringify!($exp), file!(), line!())
        }
    };
    ($exp:expr, default) => {
        // TODO: remove for production
        if $exp != &Default::default() {
            ton_block::fail!("{} {}:{}", stringify!($exp), file!(), line!())
        }
    };
    ($exp1:expr, $exp2:expr) => {{
        // TODO: remove for production
        #[cfg(test)]
        pretty_assertions::assert_eq!($exp1, $exp2);
        if $exp1 != $exp2 {
            ton_block::fail!(
                "{} != {} {}:{}",
                stringify!($exp1),
                stringify!($exp2),
                file!(),
                line!()
            )
        }
    }};
}
