use super::*;

#[test]
fn set_operations() {
    let mut set = vec![(5, 6), (1, 2), (3, 4), (10, 12), (11, 20)];
    normalize(&mut set);
    assert_eq!(set, [(1, 6), (10, 20)]);
    assert_eq!(complement(&set), [(0, 0), (7, 9), (21, MAX_CHAR)]);
    assert_eq!(complement(&[]), [(0, MAX_CHAR)]);
    assert_eq!(complement(&[(0, MAX_CHAR)]), []);
    assert_eq!(difference(&set, &[(2, 11)]), [(1, 1), (12, 20)]);
    assert_eq!(difference(&set, &[]), set);
    assert_eq!(category("Lt").first(), Some(&(0x1c5, 0x1c5)));
    assert!(category("Cn").is_empty() && category("Xx").is_empty());
}

#[test]
fn group_negation_states() {
    type Items = &'static [(&'static [(u32, u32)], u8)];
    let cases: &[(Items, &[(u32, u32)])] = &[
        (&[(&[(1, 5)], 0), (&[(3, 4)], 2)], &[(1, 2), (5, 5)]),
        (&[(&[(1, 5)], 1)], &[(0, 0), (6, MAX_CHAR)]),
        (&[(&[], 1)], &[(0, MAX_CHAR)]),
        (&[(&[(1, 5)], 1), (&[(3, 9)], 0)], &[(0, 0), (6, MAX_CHAR)]),
        (&[(&[(7, 7)], 2)], &[]),
    ];
    for (items, expected) in cases {
        let mut group = Group::default();
        for (set, neg) in *items {
            group.add(set, *neg);
        }
        assert_eq!(group.set(), *expected, "{items:?}");
    }
    let mut parts = Parts::default();
    for c in (0..10_000).step_by(2) {
        parts.add(&[(c, c)]);
        assert!(parts.ranges.len() <= c as usize + 66);
    }
    assert_eq!(parts.take().len(), 5000);
}

#[test]
fn dfa_cache_is_bounded() {
    let regex = Regex::new("[ab]*a[ab]{14}").unwrap();
    let value: String = (0..20_000)
        .map(|i| if i * 13 % 7 < 3 { 'a' } else { 'b' })
        .collect();
    let mut dfa = Dfa::default();
    let expected = value.as_bytes()[value.len() - 15] == b'a';
    assert_eq!(dfa.run(&regex.nfa, &value), expected);
    assert!(dfa.bytes <= CACHE_BYTES + 4096);
    // A warm cache answers the same way.
    assert_eq!(dfa.run(&regex.nfa, &value), expected);
    assert!(!dfa.run(&regex.nfa, "c"));
}

#[test]
fn wide_alphabets_simulate_the_nfa() {
    let class: String = (0..2100u32)
        .map(|i| format!("\\u{:04X}", 0x1000 + 2 * i))
        .collect();
    let regex = Regex::new(&format!("[{class}]+x")).unwrap();
    assert!(regex.nfa.starts.len() * 4 * 64 > CACHE_BYTES);
    let even: String = (0..2100u32)
        .map(|i| char::from_u32(0x1000 + 2 * i).unwrap())
        .collect();
    assert!(regex.is_match(&(even.clone() + "x")));
    assert!(!regex.is_match(&even));
    assert!(!regex.is_match("\u{1001}x"));
    assert!(!regex.is_match("x"));
}
