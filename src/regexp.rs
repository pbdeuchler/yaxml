//! XML Schema 1.0 regular expressions (Part 2, Appendix F) as libxml2 2.15.4
//! `xmlregexp.c` parses them, including its extensions and quirks (see
//! `docs/schema-types.md`). A pattern matches a whole string.
//!
//! A pattern compiles to a Thompson NFA. Its size, including every copy of a
//! counted repetition and every distinct character class, is charged to a
//! budget, so `a{1000000}` is rejected rather than expanded. Matching runs a
//! lazily built DFA with a bounded cache and never backtracks: each character
//! costs one cached lookup, or on a miss one NFA step bounded by the
//! automaton size.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

pub(crate) mod unicode;

/// Budget of [`Regex::new`]: NFA instructions, repetition copies and
/// distinct class ranges.
pub const DEFAULT_BUDGET: usize = 100_000;

/// libxml2's limit on nested groups, also applied to class subtractions.
const MAX_DEPTH: u32 = 50;
/// DFA cache size above which the cache is discarded and rebuilt.
const CACHE_BYTES: usize = 1 << 20;
const UNKNOWN: u32 = u32::MAX;

const MAX_CHAR: u32 = 0x10_FFFF;
const XML_CHAR: &[(u32, u32)] = &[
    (0x9, 0xA),
    (0xD, 0xD),
    (0x20, 0xD7FF),
    (0xE000, 0xFFFD),
    (0x1_0000, MAX_CHAR),
];
const SPACE: &[(u32, u32)] = &[(0x9, 0xA), (0xD, 0xD), (0x20, 0x20)];
const NEWLINES: &[(u32, u32)] = &[(0xA, 0xA), (0xD, 0xD)];
/// `SingleCharEsc` plus libxml2's non-standard escapes (`\u` is separate).
const SINGLE_ESCAPES: &[u8] = b"nrt\\|.?*+(){}-[]^!\"#$%,/:;=>@`~";
/// Category letters and their optional second letters.
const CATEGORIES: &[(u8, &[u8])] = &[
    (b'L', b"ultmo"),
    (b'M', b"nce"),
    (b'N', b"dlo"),
    (b'P', b"cdseifo"),
    (b'Z', b"slp"),
    (b'S', b"mcko"),
    (b'C', b"cfon"),
];

/// Why a pattern did not compile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// Invalid syntax at a byte offset of the pattern.
    Syntax {
        offset: usize,
        message: &'static str,
    },
    /// The automaton would exceed the compile budget.
    TooComplex,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { offset, message } => {
                write!(f, "invalid regular expression at byte {offset}: {message}")
            }
            Self::TooComplex => f.write_str("regular expression exceeds the compile budget"),
        }
    }
}

impl std::error::Error for Error {}

/// A compiled pattern.
pub struct Regex {
    pattern: Box<str>,
    nfa: Box<Nfa>,
    cache: Mutex<Box<Dfa>>,
}

impl Regex {
    /// Compiles `pattern` with [`DEFAULT_BUDGET`].
    pub fn new(pattern: &str) -> Result<Self, Error> {
        Self::with_budget(pattern, DEFAULT_BUDGET)
    }

    /// Compiles `pattern`, spending at most `budget` units (see the module
    /// documentation). Like libxml2, the pattern ends at its first NUL.
    pub fn with_budget(pattern: &str, budget: usize) -> Result<Self, Error> {
        let text = pattern.split('\0').next().unwrap_or("");
        let mut parser = Parser {
            text,
            pos: 0,
            depth: 0,
            budget,
            atoms: Vec::new(),
            ids: HashMap::new(),
        };
        let root = parser.regexp()?;
        if parser.cur() != 0 {
            return parser.fail("extra characters");
        }
        Ok(Self {
            pattern: pattern.into(),
            nfa: Box::new(Nfa::compile(&root, &parser.atoms, parser.budget)?),
            cache: Mutex::default(),
        })
    }

    /// The pattern as given.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// Whether the whole of `value` matches. Characters outside XML's `Char`
    /// production never match.
    pub fn is_match(&self, value: &str) -> bool {
        match self.cache.try_lock() {
            Ok(mut dfa) => dfa.run(&self.nfa, value),
            Err(_) => Dfa::default().run(&self.nfa, value),
        }
    }
}

impl Clone for Regex {
    fn clone(&self) -> Self {
        Self {
            pattern: self.pattern.clone(),
            nfa: self.nfa.clone(),
            cache: Mutex::default(),
        }
    }
}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Regex").field(&self.pattern).finish()
    }
}

/// Sorted, disjoint, non-adjacent inclusive code point ranges.
type Set = Vec<(u32, u32)>;

fn normalize(set: &mut Set) {
    set.sort_unstable();
    let mut merged: Set = Vec::with_capacity(set.len());
    for &(lo, hi) in set.iter() {
        match merged.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    *set = merged;
}

fn complement(set: &[(u32, u32)]) -> Set {
    let mut out = Vec::with_capacity(set.len() + 1);
    let mut next = 0;
    for &(lo, hi) in set {
        if lo > next {
            out.push((next, lo - 1));
        }
        next = hi + 1;
    }
    if next <= MAX_CHAR {
        out.push((next, MAX_CHAR));
    }
    out
}

fn difference(set: &[(u32, u32)], minus: &[(u32, u32)]) -> Set {
    let mut out = complement(set);
    out.extend_from_slice(minus);
    normalize(&mut out);
    complement(&out)
}

fn category(name: &str) -> &'static [(u32, u32)] {
    unicode::CATEGORIES
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .map_or(&[], |i| unicode::CATEGORIES[i].1)
}

/// Ranges accumulated for one negation state, compacted amortizedly so a
/// long class cannot hold more than about twice its distinct ranges.
#[derive(Default)]
struct Parts {
    ranges: Set,
    merged: usize,
}

impl Parts {
    fn add(&mut self, set: &[(u32, u32)]) {
        self.ranges.extend_from_slice(set);
        if self.ranges.len() > 2 * self.merged + 64 {
            normalize(&mut self.ranges);
            self.merged = self.ranges.len();
        }
    }

    fn take(&mut self) -> Set {
        let mut ranges = std::mem::take(&mut self.ranges);
        normalize(&mut ranges);
        ranges
    }
}

/// A bracketed class: ranges added in libxml2's negation states 0 (plain),
/// 1 (`^`) and 2 (subtracted).
#[derive(Default)]
struct Group {
    parts: [Parts; 3],
    negated: bool,
}

impl Group {
    fn add(&mut self, set: &[(u32, u32)], neg: u8) {
        self.negated |= neg == 1;
        self.parts[usize::from(neg)].add(set);
    }

    /// `xmlRegCheckCharacter` for `XML_REGEXP_RANGES`: a character is
    /// rejected by any negated or subtracted range it is in, and otherwise
    /// accepted by a plain range it is in or a negated range it is not in.
    fn set(mut self) -> Set {
        let mut accept = self.parts[0].take();
        let negated = self.parts[1].take();
        let mut excluded = self.parts[2].take();
        if self.negated {
            accept.extend(complement(&negated));
            normalize(&mut accept);
        }
        excluded.extend(negated);
        normalize(&mut excluded);
        difference(&accept, &excluded)
    }
}

enum Node {
    Atom(u32),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    /// Minimum and maximum (`None` is unbounded) occurrences.
    Repeat(Box<Node>, u32, Option<u32>),
}

/// A port of libxml2's recursive-descent `xmlFAParse*` functions over bytes.
struct Parser<'a> {
    text: &'a str,
    pos: usize,
    depth: u32,
    budget: usize,
    atoms: Vec<Set>,
    ids: HashMap<Set, u32>,
}

impl Parser<'_> {
    fn at(&self, offset: usize) -> u8 {
        self.text
            .as_bytes()
            .get(self.pos + offset)
            .copied()
            .unwrap_or(0)
    }

    fn cur(&self) -> u8 {
        self.at(0)
    }

    fn fail<T>(&self, message: &'static str) -> Result<T, Error> {
        Err(Error::Syntax {
            offset: self.pos,
            message,
        })
    }

    fn expect(&mut self, byte: u8, message: &'static str) -> Result<(), Error> {
        if self.cur() != byte {
            return self.fail(message);
        }
        self.pos += 1;
        Ok(())
    }

    /// The character at the cursor, which is always on a boundary here.
    fn char(&self) -> (u32, usize) {
        let c = self.text[self.pos..].chars().next().unwrap_or('\0');
        (c.into(), c.len_utf8())
    }

    fn enter(&mut self) -> Result<(), Error> {
        if self.depth >= MAX_DEPTH {
            return self.fail("maximum nesting depth exceeded");
        }
        self.depth += 1;
        Ok(())
    }

    /// Interns an atom's character set, restricted to XML characters as
    /// libxml2 rejects any other input character.
    fn atom(&mut self, set: &[(u32, u32)]) -> Result<Node, Error> {
        let set = difference(set, &complement(XML_CHAR));
        if let Some(&id) = self.ids.get(&set) {
            return Ok(Node::Atom(id));
        }
        self.budget = self
            .budget
            .checked_sub(set.len())
            .ok_or(Error::TooComplex)?;
        let id = self.atoms.len() as u32;
        self.atoms.push(set.clone());
        self.ids.insert(set, id);
        Ok(Node::Atom(id))
    }

    /// `regExp ::= branch ( '|' branch )*`
    fn regexp(&mut self) -> Result<Node, Error> {
        let mut branches = vec![self.branch()?];
        while self.cur() == b'|' {
            self.pos += 1;
            branches.push(self.branch()?);
        }
        Ok(match branches.len() {
            1 => branches.pop().expect("one branch"),
            _ => Node::Alt(branches),
        })
    }

    /// `branch ::= piece*`, `piece ::= atom quantifier?`
    fn branch(&mut self) -> Result<Node, Error> {
        let mut pieces = Vec::new();
        while let Some(atom) = self.piece()? {
            pieces.push(self.quantifier(atom)?);
        }
        Ok(Node::Seq(pieces))
    }

    /// `atom ::= Char | charClass | '(' regExp ')'`; `None` ends a branch.
    fn piece(&mut self) -> Result<Option<Node>, Error> {
        Ok(Some(match self.cur() {
            0 | b'|' | b')' | b'?' | b'*' | b'+' | b']' => return Ok(None),
            b'(' => {
                self.pos += 1;
                self.enter()?;
                let node = self.regexp()?;
                self.depth -= 1;
                self.expect(b')', "expecting ')'")?;
                node
            }
            b'[' => {
                self.pos += 1;
                let mut group = Group::default();
                self.group(&mut group, 0)?;
                self.expect(b']', "']' expected")?;
                self.atom(&group.set())?
            }
            b'\\' | b'.' => {
                let (set, negated) = self.escape()?;
                self.atom(&if negated { complement(&set) } else { set })?
            }
            _ => {
                let (c, len) = self.char();
                self.pos += len;
                self.atom(&[(c, c)])?
            }
        }))
    }

    /// `quantifier ::= [?*+] | '{' QuantExact (',' QuantExact?)? '}'`.
    /// As in libxml2, `{n,0}` means `{n}`, `{n,m}` with `m < n` matches
    /// nothing and a maximum of `i32::MAX` is unbounded.
    fn quantifier(&mut self, node: Node) -> Result<Node, Error> {
        let (min, max) = match self.cur() {
            b'?' => (0, Some(1)),
            b'*' => (0, None),
            b'+' => (1, None),
            b'{' => {
                self.pos += 1;
                let min = self.count()?;
                let max = match self.cur() {
                    b',' => {
                        self.pos += 1;
                        match self.cur() {
                            b'}' => None,
                            _ => Some(self.count()?),
                        }
                    }
                    _ => Some(0),
                };
                if self.cur() != b'}' {
                    return self.fail("unterminated quantifier");
                }
                let max = match max {
                    Some(0) => Some(min),
                    Some(max) if max == i32::MAX as u32 => None,
                    max => max,
                };
                (min, max)
            }
            _ => return Ok(node),
        };
        self.pos += 1;
        if max.is_some_and(|max| max < min) {
            return self.atom(&[]);
        }
        Ok(Node::Repeat(Box::new(node), min, max))
    }

    /// `QuantExact ::= [0-9]+`, at most `i32::MAX`.
    fn count(&mut self) -> Result<u32, Error> {
        let digits = self.text[self.pos..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let count = self.text[self.pos..self.pos + digits].parse::<i32>();
        let count = count
            .map(|n| n as u32)
            .or_else(|_| self.fail("improper quantifier"))?;
        self.pos += digits;
        Ok(count)
    }

    /// `charClassEsc` or `.` outside brackets: the set, and whether `\P`
    /// asks for its complement.
    fn escape(&mut self) -> Result<(Set, bool), Error> {
        if self.cur() == b'.' {
            self.pos += 1;
            return Ok((complement(NEWLINES), false));
        }
        self.pos += 1;
        let c = self.cur();
        let set = match c {
            b'p' | b'P' => {
                self.pos += 1;
                self.expect(b'{', "expecting '{'")?;
                let set = self.property()?;
                if self.cur() != b'}' {
                    return self.fail("expecting '}'");
                }
                set
            }
            b's' | b'S' | b'i' | b'I' | b'c' | b'C' | b'd' | b'D' | b'w' | b'W' => {
                let lower = c.to_ascii_lowercase();
                let set: &[(u32, u32)] = match lower {
                    b's' => SPACE,
                    b'i' => unicode::INIT_NAME,
                    b'c' => unicode::NAME_CHAR,
                    b'd' => category("Nd"),
                    _ => unicode::NOT_WORD,
                };
                // `\w` is the complement of P, Z and C; `\W` is not.
                match c.is_ascii_uppercase() != (lower == b'w') {
                    true => complement(set),
                    false => set.to_vec(),
                }
            }
            _ => {
                let value = self.single_escape()?;
                vec![(value, value)]
            }
        };
        self.pos += 1;
        Ok((set, c == b'P'))
    }

    /// The value of a `SingleCharEsc` or `\u` escape; the cursor is on the
    /// byte after `\` and is left on the escape's last byte.
    fn single_escape(&mut self) -> Result<u32, Error> {
        Ok(match self.cur() {
            b'n' => 0xA,
            b'r' => 0xD,
            b't' => 0x9,
            b'u' => self.code_point()?,
            c if SINGLE_ESCAPES.contains(&c) => c.into(),
            _ => return self.fail("invalid escape"),
        })
    }

    fn code_unit(&mut self) -> Result<u32, Error> {
        let mut value = 0;
        for _ in 0..4 {
            self.pos += 1;
            let digit = char::from(self.cur()).to_digit(16);
            value = value * 16 + digit.map_or_else(|| self.fail("expecting hex digit"), Ok)?;
        }
        Ok(value)
    }

    /// `\uXXXX`, with a surrogate pair written as two escapes.
    fn code_point(&mut self) -> Result<u32, Error> {
        let high = self.code_unit()?;
        if !(0xD800..=0xDBFF).contains(&high) {
            return Ok(high);
        }
        self.pos += 1;
        if self.cur() == b'\\' {
            self.pos += 1;
            if self.cur() == b'u' {
                let low = self.code_unit()?;
                if (0xDC00..=0xDFFF).contains(&low) {
                    return Ok(0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00));
                }
            }
        }
        self.fail("invalid low surrogate pair code unit")
    }

    /// `charProp ::= IsCategory | IsBlock`. Unlike libxml2, which fails at
    /// match time, an unknown block name is a syntax error.
    fn property(&mut self) -> Result<Set, Error> {
        let c = self.cur();
        self.pos += 1;
        if c == b'I' {
            self.expect(b's', "IsXXXX expected")?;
            let start = self.pos;
            while self.cur().is_ascii_alphanumeric() || self.cur() == b'-' {
                self.pos += 1;
            }
            let name = &self.text[start..self.pos];
            return match unicode::BLOCKS.binary_search_by(|(n, _)| (*n).cmp(name)) {
                Ok(i) => Ok(unicode::BLOCKS[i].1.to_vec()),
                Err(_) => self.fail("unknown block name"),
            };
        }
        let Some(&(_, second)) = CATEGORIES.iter().find(|(first, _)| *first == c) else {
            self.pos -= 1;
            return self.fail("unknown char property");
        };
        let end = self.pos + usize::from(second.contains(&self.cur()));
        let name = &self.text[self.pos - 1..end];
        self.pos = end;
        Ok(category(name).to_vec())
    }

    /// `charGroup` in libxml2's negation state `neg`.
    fn group(&mut self, group: &mut Group, neg: u8) -> Result<(), Error> {
        if self.cur() == b'^' {
            self.pos += 1;
            self.positive_group(group, u8::from(neg == 0))?;
        }
        while self.cur() != b']' {
            if self.cur() == b'-' && self.at(1) == b'[' {
                self.pos += 2;
                self.enter()?;
                self.group(group, 2)?;
                self.depth -= 1;
                return self.expect(b']', "charClassExpr: ']' expected");
            }
            self.positive_group(group, neg)?;
        }
        Ok(())
    }

    /// `posCharGroup ::= ( charRange | charClassEsc )+`. Inside brackets
    /// libxml2 ignores the negation of `\P`.
    fn positive_group(&mut self, group: &mut Group, neg: u8) -> Result<(), Error> {
        loop {
            let range = match (self.cur(), self.at(1)) {
                (b'\\', b'u') => self.at(6) == b'-',
                (b'\\', c) if SINGLE_ESCAPES.contains(&c) => self.at(2) == b'-',
                (b'\\', _) => false,
                _ => true,
            };
            if range {
                self.range(group, neg)?;
            } else {
                group.add(&self.escape()?.0, neg);
            }
            if matches!(self.cur(), b']' | b'-' | 0) {
                return Ok(());
            }
        }
    }

    /// `charRange`, including libxml2's silent skip of a `-` that neither
    /// starts the group nor precedes `]`.
    fn range(&mut self, group: &mut Group, neg: u8) -> Result<(), Error> {
        if self.cur() == 0 {
            return self.fail("expecting ']'");
        }
        let (start, len) = self.range_char("expecting a char range")?;
        let prev = self.text.as_bytes()[self.pos - 1];
        if start == u32::from(b'-') && self.at(1) != b']' && prev != b'[' && prev != b'^' {
            self.pos += len;
            return Ok(());
        }
        self.pos += len;
        if self.cur() != b'-' || matches!(self.at(1), b'[' | b']') {
            group.add(&[(start, start)], neg);
            return Ok(());
        }
        self.pos += 1;
        let (end, len) = self.range_char("expecting the end of a char range")?;
        if end < start {
            return self.fail("end of range is before start of range");
        }
        self.pos += len;
        group.add(&[(start, end)], neg);
        Ok(())
    }

    /// A range endpoint and the bytes to its end from the cursor.
    fn range_char(&mut self, message: &'static str) -> Result<(u32, usize), Error> {
        match self.cur() {
            b'\\' => {
                self.pos += 1;
                Ok((self.single_escape()?, 1))
            }
            0 | b'[' | b']' => self.fail(message),
            _ => Ok(self.char()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Inst {
    Match,
    /// Consume a character of the atom, then continue.
    Atom(u32, u32),
    Split(u32, u32),
}

#[derive(Clone, Debug)]
struct Nfa {
    insts: Vec<Inst>,
    /// The epsilon closure of the start instruction.
    start: Box<[u32]>,
    /// First code point of each character class.
    starts: Box<[u32]>,
    ascii: [u32; 128],
    /// Each atom as sorted inclusive class intervals.
    atoms: Vec<Box<[(u32, u32)]>>,
}

impl Nfa {
    fn compile(root: &Node, atoms: &[Set], mut budget: usize) -> Result<Self, Error> {
        let mut starts: Vec<u32> = std::iter::once(0)
            .chain(atoms.iter().flatten().flat_map(|&(lo, hi)| [lo, hi + 1]))
            .filter(|&c| c <= MAX_CHAR)
            .collect();
        starts.sort_unstable();
        starts.dedup();
        let class = |c: u32| (starts.partition_point(|&s| s <= c) - 1) as u32;
        let atoms = atoms
            .iter()
            .map(|set| set.iter().map(|&(lo, hi)| (class(lo), class(hi))).collect())
            .collect();
        let ascii = std::array::from_fn(|c| class(c as u32));
        let mut nfa = Self {
            insts: vec![Inst::Match],
            start: Box::default(),
            starts: starts.into(),
            ascii,
            atoms,
        };
        let entry = nfa.emit(root, 0, &mut budget)?;
        let mut start = Scratch::default().close(&nfa.insts, [entry]);
        start.sort_unstable();
        nfa.start = start.into();
        Ok(nfa)
    }

    fn push(&mut self, inst: Inst, budget: &mut usize) -> Result<u32, Error> {
        *budget = budget.checked_sub(1).ok_or(Error::TooComplex)?;
        self.insts.push(inst);
        Ok(self.insts.len() as u32 - 1)
    }

    /// Emits `node` followed by instruction `next` and returns its entry.
    fn emit(&mut self, node: &Node, next: u32, budget: &mut usize) -> Result<u32, Error> {
        match node {
            Node::Atom(atom) => self.push(Inst::Atom(*atom, next), budget),
            Node::Seq(nodes) => nodes
                .iter()
                .rev()
                .try_fold(next, |next, node| self.emit(node, next, budget)),
            Node::Alt(nodes) => {
                let mut entry = self.emit(&nodes[nodes.len() - 1], next, budget)?;
                for node in nodes[..nodes.len() - 1].iter().rev() {
                    let branch = self.emit(node, next, budget)?;
                    entry = self.push(Inst::Split(branch, entry), budget)?;
                }
                Ok(entry)
            }
            Node::Repeat(node, min, max) => {
                let mut entry = match max {
                    None => {
                        let split = self.push(Inst::Split(0, next), budget)?;
                        let body = self.emit(node, split, budget)?;
                        self.insts[split as usize] = Inst::Split(body, next);
                        split
                    }
                    Some(max) => (*min..*max).try_fold(next, |rest, _| {
                        let body = self.emit(node, rest, budget)?;
                        self.push(Inst::Split(body, next), budget)
                    })?,
                };
                for _ in 0..*min {
                    *budget = budget.checked_sub(1).ok_or(Error::TooComplex)?;
                    entry = self.emit(node, entry, budget)?;
                }
                Ok(entry)
            }
        }
    }

    fn class(&self, c: char) -> u32 {
        match self.ascii.get(c as usize) {
            Some(&class) => class,
            None => (self.starts.partition_point(|&s| s <= u32::from(c)) - 1) as u32,
        }
    }

    fn accepts(&self, atom: u32, class: u32) -> bool {
        let ranges = &self.atoms[atom as usize];
        let i = ranges.partition_point(|&(_, hi)| hi < class);
        ranges.get(i).is_some_and(|&(lo, _)| lo <= class)
    }
}

/// Epsilon-closure marks shared by DFA misses and NFA simulation.
#[derive(Debug, Default)]
struct Scratch {
    seen: Vec<u32>,
    stamp: u32,
    stack: Vec<u32>,
}

impl Scratch {
    /// The non-split instructions reachable from `targets` without input.
    fn close(&mut self, insts: &[Inst], targets: impl IntoIterator<Item = u32>) -> Vec<u32> {
        if self.seen.len() != insts.len() {
            *self = Self {
                seen: vec![0; insts.len()],
                ..Self::default()
            };
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.fill(0);
            self.stamp = 1;
        }
        let mut out = Vec::new();
        for target in targets {
            self.stack.push(target);
            while let Some(i) = self.stack.pop() {
                if std::mem::replace(&mut self.seen[i as usize], self.stamp) == self.stamp {
                    continue;
                }
                match insts[i as usize] {
                    Inst::Split(first, second) => self.stack.extend([second, first]),
                    _ => out.push(i),
                }
            }
        }
        out
    }

    /// The closed set reached from the closed `set` on `class`.
    fn successor(&mut self, nfa: &Nfa, set: &[u32], class: u32) -> Vec<u32> {
        let targets = set.iter().filter_map(|&i| match nfa.insts[i as usize] {
            Inst::Atom(atom, to) if nfa.accepts(atom, class) => Some(to),
            _ => None,
        });
        self.close(&nfa.insts, targets)
    }
}

/// The lazily built DFA: state 0 is the start, `dead` the empty set.
#[derive(Debug, Default)]
struct Dfa {
    sets: Vec<Box<[u32]>>,
    index: HashMap<Box<[u32]>, u32>,
    /// `sets.len()` rows of one target (or `UNKNOWN`) per character class.
    table: Vec<u32>,
    accepting: Vec<bool>,
    dead: usize,
    bytes: usize,
    scratch: Scratch,
}

impl Dfa {
    fn run(&mut self, nfa: &Nfa, value: &str) -> bool {
        let classes = nfa.starts.len();
        // With rows too wide for 64 of them to fit, simulate the NFA.
        if classes * 4 * 64 > CACHE_BYTES {
            return self.simulate(nfa, value);
        }
        if self.sets.is_empty() {
            self.reset(nfa);
        }
        let mut state = 0;
        for c in value.chars() {
            let class = nfa.class(c) as usize;
            state = match self.table[state * classes + class] {
                UNKNOWN => self.step(nfa, state, class),
                next => next as usize,
            };
            if state == self.dead {
                return false;
            }
        }
        self.accepting[state]
    }

    fn simulate(&mut self, nfa: &Nfa, value: &str) -> bool {
        let mut set = nfa.start.to_vec();
        for c in value.chars() {
            set = self.scratch.successor(nfa, &set, nfa.class(c));
            if set.is_empty() {
                return false;
            }
        }
        set.iter()
            .any(|&i| matches!(nfa.insts[i as usize], Inst::Match))
    }

    fn reset(&mut self, nfa: &Nfa) {
        *self = Self {
            scratch: std::mem::take(&mut self.scratch),
            ..Self::default()
        };
        self.intern(nfa, nfa.start.clone());
        self.dead = self.intern(nfa, Box::default());
    }

    fn intern(&mut self, nfa: &Nfa, set: Box<[u32]>) -> usize {
        if let Some(&state) = self.index.get(&set) {
            return state as usize;
        }
        let state = self.sets.len();
        self.bytes += nfa.starts.len() * 4 + set.len() * 8 + 64;
        self.accepting.push(
            set.iter()
                .any(|&i| matches!(nfa.insts[i as usize], Inst::Match)),
        );
        self.table
            .resize(self.table.len() + nfa.starts.len(), UNKNOWN);
        self.index.insert(set.clone(), state as u32);
        self.sets.push(set);
        state
    }

    /// Computes, caches and returns the successor of `state` on `class`,
    /// first discarding a full cache.
    fn step(&mut self, nfa: &Nfa, mut state: usize, class: usize) -> usize {
        if self.bytes > CACHE_BYTES {
            let current = self.sets[state].clone();
            self.reset(nfa);
            state = self.intern(nfa, current);
        }
        let mut next = self.scratch.successor(nfa, &self.sets[state], class as u32);
        next.sort_unstable();
        let target = self.intern(nfa, next.into());
        self.table[state * nfa.starts.len() + class] = target as u32;
        target
    }
}

#[cfg(test)]
#[path = "../tests/internal/regexp.rs"]
mod tests;
