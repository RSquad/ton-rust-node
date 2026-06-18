#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use ton_block::BocReader;

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    let _ = BocReader::new().stream_read(&mut cursor);
});
