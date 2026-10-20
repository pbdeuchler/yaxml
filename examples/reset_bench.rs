//! 100k tiny documents through a new `PushParser` each versus one reused with
//! `reset`; prints medians of 31 rounds (`cargo run --release --example reset_bench`).
use std::ops::ControlFlow;
use std::time::Instant;
use yaxml::PushParser;
fn med(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}
fn main() {
    let docs: [(&str, &[u8]); 3] = [
        ("tiny", b"<a/>"),
        ("ns", br#"<a xmlns:p="u" x="1"><p:b y="2">t</p:b><c/></a>"#),
        ("rec", br#"<?xml version="1.0"?><r><i id="1" k="v">one</i><i id="2" k="w">two</i><i id="3"/></r>"#),
    ];
    for (name, doc) in docs {
        let (mut f, mut r) = (vec![], vec![]);
        for _ in 0..31 {
            let t = Instant::now();
            for _ in 0..100_000 {
                let mut p = PushParser::new();
                p.feed(doc, |_, _| ControlFlow::Continue(())).unwrap();
                p.finish(|_, _| ControlFlow::Continue(())).unwrap();
            }
            f.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let mut p = PushParser::new();
            for _ in 0..100_000 {
                p.reset();
                p.feed(doc, |_, _| ControlFlow::Continue(())).unwrap();
                p.finish(|_, _| ControlFlow::Continue(())).unwrap();
            }
            r.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (mf, mr) = (med(f), med(r));
        println!(
            "{name}: fresh {mf:.1} ms reset {mr:.1} ms  gain {:.1}%",
            (1.0 - mr / mf) * 100.0
        );
    }
}
