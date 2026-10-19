//! Regular expression throughput: `cargo bench --bench regexp`.

use std::hint::black_box;
use std::time::Instant;

use yaxml::regexp::Regex;

fn main() {
    let ascii = "abcdefghij".repeat(100_000);
    let words = "Lorem ipsum dolor sit amet, ünïcödé text ".repeat(25_000);
    let cases = [
        ("[a-z]*", &ascii, "short class loop"),
        ("(a|b|c|d|e|f|g|h|i|j)*", &ascii, "alternation loop"),
        ("([a-z]{1,3})*k|([a-j]{2,5})*", &ascii, "counted repetition"),
        ("(\\p{L}|\\s|\\p{P})*", &words, "categories"),
        (
            "[\\i-[:]][\\c-[:]]*",
            &words.replace([' ', ','], "_"),
            "NCName",
        ),
        ("(.*a){8}.*", &words, "nested quantifiers"),
    ];
    for (pattern, input, label) in cases {
        let regex = Regex::new(pattern).expect("pattern compiles");
        let start = Instant::now();
        let mut rounds = 0;
        while rounds < 3 || start.elapsed().as_secs_f64() < 0.5 {
            black_box(regex.is_match(black_box(input)));
            rounds += 1;
        }
        let seconds = start.elapsed().as_secs_f64();
        let mb = (input.len() * rounds) as f64 / 1e6;
        println!("{label:20} {pattern:34} {:8.1} MB/s", mb / seconds);
    }
}
