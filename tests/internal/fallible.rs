use std::io::Write;

use super::*;

#[test]
fn helpers_copy_and_fill() {
    assert_eq!(copied("abc"), Ok("abc".to_owned()));
    assert_eq!(concat(&["a", "", "bc"]), Ok("abc".to_owned()));
    assert_eq!(concat(&[]), Ok(String::new()));
    assert_eq!(cloned(&[1, 2]), Ok(vec![1, 2]));
    assert_eq!(collected(0..3), Ok(vec![0, 1, 2]));
    assert_eq!(filled(7u8, 2), Ok(vec![7, 7]));
    let (mut items, mut text) = (vec![1], String::from("a"));
    assert_eq!((items.try_push(2), text.try_push("b")), (Ok(()), Ok(())));
    assert_eq!((items, text.as_str()), (vec![1, 2], "ab"));
}

#[test]
fn helpers_report_capacity_they_cannot_reserve() {
    assert!(filled(0u64, usize::MAX).is_err());
    let mut items = vec![0u64];
    assert!(items.try_reserve(usize::MAX).is_err());
    assert_eq!(items, [0], "a refusal leaves the vector as it was");
}

#[test]
fn text_finishes_unless_refused() {
    let mut text = Text::default();
    text.push_str("a");
    text.push('é');
    assert_eq!(text.finish(), Some("aé".to_owned()));
    let refused = Text {
        text: "partial".into(),
        refused: true,
    };
    assert_eq!(refused.finish(), None);
}

#[test]
fn memory_sink_appends() {
    let mut memory = Memory::default();
    assert_eq!(memory.write(b"ab").unwrap(), 2);
    memory.write_all(b"c").unwrap();
    memory.flush().unwrap();
    assert_eq!(memory.0, b"abc");
}
