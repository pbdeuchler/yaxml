#![no_main]

#[allow(dead_code)]
#[path = "../../tests/support/fuzz/driver.rs"]
mod driver;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    driver::exercise_pull(data, data.get(data.len() / 2).copied().unwrap_or(0));
});
