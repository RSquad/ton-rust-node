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
macro_rules! simple_commands {

    // quantity of nothing is 0
    (@count ) => { 0u8 };

    // count quantity recursively
    (@count $_x:ident = $_y:ident; $($pname:ident = $parser:ident;)*) => {
        1u8 + simple_commands!(@count $($pname = $parser;)* )
    };

    // parse command without parameters
    (@resolve $command:ident => $($code:expr),+) => {
        #[allow(non_snake_case)]
        pub fn $command(
            &mut self,
            par: &[&str],
            destination: &mut Units,
            pos: DbgPos
        ) -> CompileResult {
            par.assert_empty()?;
            destination.write_command(&[$($code),*], DbgNode::from(pos))
        }
    };

    // parse command with any parameters
    (@resolve $command:ident $($pname:ident = $parser:ident);+ => $($code:expr),+) => {
        #[allow(non_snake_case)]
        pub fn $command(
            &mut self,
            par: &[&str],
            destination: &mut Units,
            pos: DbgPos
        ) -> CompileResult {
            let n_params = simple_commands!(@count $($pname = $parser;)*);
            par.assert_len(n_params as usize)?;
            let mut result: Vec<u8> = vec![];
            let mut _parameters_i_:usize = 0;
            $(
                let $pname = $parser(par[_parameters_i_]).parameter("arg ".to_string() + &_parameters_i_.to_string())?;
                _parameters_i_ += 1;
            )*
            $({
                result.push($code);
            })*
            destination.write_command(result.as_slice(), DbgNode::from(pos))
        }
    };

    // parse whole block of simple commands
    ($enumerate_commands:ident $($command: ident $($pname:ident = $parser:ident);* => $($code:expr),+ )*) => {
        $(
            simple_commands!(@resolve $command $($pname = $parser);* => $($code),*);
        )*
        pub fn $enumerate_commands() -> &'static [(&'static str, CompileHandler)] {
            &[
                $( (stringify!($command), Engine::$command), )*
            ]
        }
    };

}
