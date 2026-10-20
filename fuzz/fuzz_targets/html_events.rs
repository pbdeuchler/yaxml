#![no_main]

#[allow(dead_code)]
#[path = "../driver.rs"]
mod driver;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    driver::exercise_html_events(data);
});
