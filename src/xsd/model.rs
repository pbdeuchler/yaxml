//! Content models compiled to Glushkov automata extended with counters.
//!
//! Positions are the element and wildcard particles. A repetition with a
//! finite `maxOccurs` above one (or a `minOccurs` above one) gets a counter
//! instead of being unrolled, so `maxOccurs="1000000"` costs one counter.
//! Each transition records how many enclosing counters it keeps, whether it
//! starts another iteration of the innermost kept one, and which counters it
//! leaves (they must have run `minOccurs` times) or enters (set to one).
//!
//! Unique Particle Attribution makes at most one position match a child from
//! any state, so a run is one position plus the set of counter vectors that
//! reach it; the set holds more than one vector only for nested repetitions
//! such as `(a{2,3}){2}`, and is capped. Construction spends a work budget
//! because follow sets can be quadratic in the positions.

use super::components::{ElemId, NsConstraint, QName, WildId};

/// What a position matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Leaf {
    Element(QName, ElemId),
    Wildcard(NsConstraint, WildId),
}

impl Leaf {
    pub(crate) fn matches(&self, name: QName) -> bool {
        match self {
            Leaf::Element(own, _) => *own == name,
            Leaf::Wildcard(ns, _) => ns.allows(name.ns),
        }
    }

    /// Whether some name matches both (the UPA conflict test).
    fn overlaps(&self, other: &Leaf) -> bool {
        use NsConstraint::{Any, Not, Set};
        match (self, other) {
            (Leaf::Element(a, _), Leaf::Element(b, _)) => a == b,
            (Leaf::Element(name, _), Leaf::Wildcard(ns, _))
            | (Leaf::Wildcard(ns, _), Leaf::Element(name, _)) => ns.allows(name.ns),
            (Leaf::Wildcard(a, _), Leaf::Wildcard(b, _)) => match (a, b) {
                (Any, _) | (_, Any) | (Not(_), Not(_)) => true,
                (Set(x), Set(y)) => x.iter().any(|ns| y.contains(ns)),
                (Not(_), set @ Set(_)) | (set @ Set(_), Not(_)) => {
                    let Set(members) = set else { unreachable!() };
                    let other = if matches!(a, Not(_)) { a } else { b };
                    members.iter().any(|&ns| other.allows(ns))
                }
            },
        }
    }
}

/// A particle tree to compile.
#[derive(Clone, Debug)]
pub(crate) enum Expr {
    Leaf(Leaf),
    Seq(Vec<Expr>),
    Choice(Vec<Expr>),
    /// A repetition `{min, max}`; `None` is unbounded.
    Repeat(Box<Expr>, u32, Option<u32>),
}

impl Expr {
    pub(crate) fn empty() -> Expr {
        Expr::Seq(Vec::new())
    }
}

#[derive(Clone, Copy, Debug)]
struct Counter {
    min: u32,
    max: Option<u32>,
    /// An iteration may be empty, so any count satisfies `min`.
    nullable: bool,
}

impl Counter {
    fn exitable(&self, count: u32) -> bool {
        count >= self.min || self.nullable
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Edge {
    from: u32,
    to: u32,
    /// Leading counters of both chains carried over.
    keep: u16,
    /// Whether the counter at `keep` starts another iteration.
    bump: bool,
}

/// A compiled sequence/choice model. State 0 is the start; state `p + 1`
/// follows a match of position `p`.
#[derive(Clone, Debug)]
pub(crate) struct Graph {
    leaves: Vec<Leaf>,
    /// Enclosing counters of each state, outermost first.
    chains: Vec<Vec<u32>>,
    counters: Vec<Counter>,
    edges: Vec<Edge>,
    /// `edges[offsets[s]..offsets[s + 1]]` leave state `s`.
    offsets: Vec<u32>,
    accepting: Vec<bool>,
}

/// An `all` group: each item at most once, required ones at least once.
#[derive(Clone, Debug)]
pub(crate) struct AllGroup {
    /// Each item's alternatives (a substitution group shares one slot).
    pub(crate) items: Vec<(Vec<Leaf>, bool)>,
    /// `minOccurs="0"` on the group: no children at all is valid.
    pub(crate) optional: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum Model {
    Graph(Graph),
    All(AllGroup),
}

/// Construction ran out of work budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TooComplex;

#[derive(Default)]
struct Fragment {
    nullable: bool,
    first: Vec<u32>,
    last: Vec<u32>,
}

struct Builder {
    leaves: Vec<Leaf>,
    chains: Vec<Vec<u32>>,
    counters: Vec<Counter>,
    edges: Vec<Edge>,
    budget: usize,
}

impl Builder {
    fn charge(&mut self, work: usize) -> Result<(), TooComplex> {
        self.budget = self.budget.checked_sub(work).ok_or(TooComplex)?;
        Ok(())
    }

    fn link(
        &mut self,
        from: &[u32],
        to: &[u32],
        keep: usize,
        bump: bool,
    ) -> Result<(), TooComplex> {
        self.charge(from.len().saturating_mul(to.len()))?;
        let keep = u16::try_from(keep).map_err(|_| TooComplex)?;
        for &from in from {
            for &to in to {
                self.edges.push(Edge {
                    from,
                    to,
                    keep,
                    bump,
                });
            }
        }
        Ok(())
    }

    fn join(&mut self, mut left: Vec<u32>, right: &[u32]) -> Result<Vec<u32>, TooComplex> {
        self.charge(right.len())?;
        left.extend_from_slice(right);
        Ok(left)
    }

    /// States of the positions; `chain` holds the counters enclosing `expr`.
    fn build(&mut self, expr: &Expr, chain: &mut Vec<u32>) -> Result<Fragment, TooComplex> {
        self.charge(1)?;
        Ok(match expr {
            Expr::Leaf(leaf) => {
                self.leaves.push(leaf.clone());
                self.chains.push(chain.clone());
                let state = self.leaves.len() as u32;
                Fragment {
                    nullable: false,
                    first: vec![state],
                    last: vec![state],
                }
            }
            Expr::Seq(items) => {
                let mut whole = Fragment {
                    nullable: true,
                    ..Fragment::default()
                };
                for item in items {
                    let next = self.build(item, chain)?;
                    self.link(&whole.last, &next.first, chain.len(), false)?;
                    whole = Fragment {
                        first: match whole.nullable {
                            true => self.join(whole.first, &next.first)?,
                            false => whole.first,
                        },
                        last: match next.nullable {
                            true => self.join(whole.last, &next.last)?,
                            false => next.last,
                        },
                        nullable: whole.nullable && next.nullable,
                    };
                }
                whole
            }
            Expr::Choice(items) => {
                let mut whole = Fragment::default();
                for item in items {
                    let next = self.build(item, chain)?;
                    whole = Fragment {
                        nullable: whole.nullable || next.nullable,
                        first: self.join(whole.first, &next.first)?,
                        last: self.join(whole.last, &next.last)?,
                    };
                }
                whole
            }
            Expr::Repeat(_, _, Some(0)) => Fragment {
                nullable: true,
                ..Fragment::default()
            },
            &Expr::Repeat(ref body, min, max) => {
                let counted = min > 1 || max.is_some_and(|max| max > 1);
                if counted {
                    chain.push(self.counters.len() as u32);
                    self.counters.push(Counter {
                        min,
                        max,
                        nullable: false,
                    });
                }
                let inner = self.build(body, chain)?;
                if max != Some(1) {
                    let keep = chain.len() - usize::from(counted);
                    self.link(&inner.last, &inner.first, keep, counted)?;
                }
                if counted {
                    let id = chain.pop().expect("pushed") as usize;
                    self.counters[id].nullable = inner.nullable;
                }
                Fragment {
                    nullable: inner.nullable || min == 0,
                    ..inner
                }
            }
        })
    }
}

/// Model compilation failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompileError {
    TooComplex,
    /// Two positions reachable from one state match a common name.
    NotDeterministic,
}

impl Graph {
    /// Compiles `expr`, spending `budget`.
    pub(crate) fn compile(expr: &Expr, budget: &mut usize) -> Result<Graph, CompileError> {
        let mut builder = Builder {
            leaves: Vec::new(),
            chains: vec![Vec::new()],
            counters: Vec::new(),
            edges: Vec::new(),
            budget: *budget,
        };
        let root = builder
            .build(expr, &mut Vec::new())
            .map_err(|_| CompileError::TooComplex)?;
        builder
            .link(&[0], &root.first, 0, false)
            .map_err(|_| CompileError::TooComplex)?;
        *budget = builder.budget;
        let mut edges = builder.edges;
        edges.sort_unstable();
        edges.dedup();
        let states = builder.leaves.len() + 1;
        let mut offsets = vec![0u32; states + 1];
        for edge in &edges {
            offsets[edge.from as usize + 1] += 1;
        }
        for state in 0..states {
            offsets[state + 1] += offsets[state];
        }
        let mut accepting = vec![false; states];
        accepting[0] = root.nullable;
        for state in root.last {
            accepting[state as usize] = true;
        }
        let graph = Graph {
            leaves: builder.leaves,
            chains: builder.chains,
            counters: builder.counters,
            edges,
            offsets,
            accepting,
        };
        graph.check_determinism(budget)?;
        Ok(graph)
    }

    fn leaf(&self, state: u32) -> &Leaf {
        &self.leaves[state as usize - 1]
    }

    fn out(&self, state: u32) -> &[Edge] {
        let range = self.offsets[state as usize]..self.offsets[state as usize + 1];
        &self.edges[range.start as usize..range.end as usize]
    }

    /// Edges to distinct positions must not match a common name; edges to
    /// one position with different counter actions are fine (libxml2 treats
    /// equal atoms with different counters as deterministic).
    fn check_determinism(&self, budget: &mut usize) -> Result<(), CompileError> {
        for state in 0..self.accepting.len() as u32 {
            let mut targets: Vec<u32> = self.out(state).iter().map(|edge| edge.to).collect();
            targets.dedup();
            let work = targets.len().saturating_mul(targets.len());
            *budget = budget.checked_sub(work).ok_or(CompileError::TooComplex)?;
            for (i, &a) in targets.iter().enumerate() {
                if targets[..i]
                    .iter()
                    .any(|&b| self.leaf(a).overlaps(self.leaf(b)))
                {
                    return Err(CompileError::NotDeterministic);
                }
            }
        }
        Ok(())
    }

    fn exitable(&self, state: u32, counts: &[u32], from: usize) -> bool {
        let chain = &self.chains[state as usize];
        (from..chain.len()).all(|i| self.counters[chain[i] as usize].exitable(counts[i]))
    }

    /// The counters after `edge` from `counts`, if its guards pass.
    fn fire(&self, edge: Edge, counts: &[u32], out: &mut Vec<u32>) -> bool {
        let keep = edge.keep as usize;
        if !self.exitable(edge.from, counts, keep + usize::from(edge.bump)) {
            return false;
        }
        out.extend_from_slice(&counts[..keep]);
        if edge.bump {
            let counter = self.counters[self.chains[edge.from as usize][keep] as usize];
            let next = match counter.max {
                Some(max) if counts[keep] >= max => return false,
                Some(_) => counts[keep] + 1,
                None => (counts[keep] + 1).min(counter.min.max(1)),
            };
            out.push(next);
        }
        let width = self.chains[edge.to as usize].len();
        out.resize(out.len() + width - keep - usize::from(edge.bump), 1);
        true
    }
}

/// Where a child matched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Matched {
    Element(ElemId),
    Wildcard(WildId),
}

impl Leaf {
    fn target(&self) -> Matched {
        match *self {
            Leaf::Element(_, id) => Matched::Element(id),
            Leaf::Wildcard(_, id) => Matched::Wildcard(id),
        }
    }
}

/// A child was not expected, or there were too many counter vectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepError {
    Unexpected,
    TooComplex,
}

/// The matching state of one element's children.
#[derive(Clone, Debug)]
pub(crate) enum Run {
    Graph {
        state: u32,
        /// Counter vectors, each `chains[state].len()` long.
        counts: Vec<u32>,
        vectors: usize,
    },
    All(Vec<bool>),
}

impl Model {
    pub(crate) fn start(&self) -> Run {
        match self {
            Model::Graph(_) => Run::Graph {
                state: 0,
                counts: Vec::new(),
                vectors: 1,
            },
            Model::All(all) => Run::All(vec![false; all.items.len()]),
        }
    }

    /// Consumes one child named `name`.
    pub(crate) fn step(
        &self,
        run: &mut Run,
        name: QName,
        cap: usize,
    ) -> Result<Matched, StepError> {
        match (self, run) {
            (
                Model::Graph(graph),
                Run::Graph {
                    state,
                    counts,
                    vectors,
                },
            ) => {
                let width = graph.chains[*state as usize].len();
                let mut next: Vec<Vec<u32>> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                let mut found: Option<(u32, Matched)> = None;
                for &edge in graph.out(*state) {
                    let leaf = graph.leaf(edge.to);
                    if found.is_some_and(|(to, _)| to != edge.to) || !leaf.matches(name) {
                        continue;
                    }
                    for vector in rows(counts, width, *vectors) {
                        let mut fresh = Vec::new();
                        if !graph.fire(edge, vector, &mut fresh) {
                            continue;
                        }
                        found = Some((edge.to, leaf.target()));
                        if seen.insert(fresh.clone()) {
                            next.push(fresh);
                            if next.len() > cap {
                                return Err(StepError::TooComplex);
                            }
                        }
                    }
                }
                let (to, matched) = found.ok_or(StepError::Unexpected)?;
                *state = to;
                *vectors = next.len();
                *counts = next.concat();
                Ok(matched)
            }
            (Model::All(all), Run::All(seen)) => {
                let (index, leaf) = all
                    .items
                    .iter()
                    .enumerate()
                    .find_map(|(i, (leaves, _))| {
                        leaves
                            .iter()
                            .find(|leaf| leaf.matches(name))
                            .map(|leaf| (i, leaf))
                    })
                    .ok_or(StepError::Unexpected)?;
                if std::mem::replace(&mut seen[index], true) {
                    return Err(StepError::Unexpected);
                }
                Ok(leaf.target())
            }
            _ => Err(StepError::Unexpected),
        }
    }

    /// Whether the children so far form a complete content.
    pub(crate) fn accepts(&self, run: &Run) -> bool {
        match (self, run) {
            (
                Model::Graph(graph),
                Run::Graph {
                    state,
                    counts,
                    vectors,
                },
            ) => {
                let width = graph.chains[*state as usize].len();
                graph.accepting[*state as usize]
                    && rows(counts, width, *vectors).any(|vector| graph.exitable(*state, vector, 0))
            }
            (Model::All(all), Run::All(seen)) => {
                (all.optional && !seen.contains(&true))
                    || all
                        .items
                        .iter()
                        .zip(seen)
                        .all(|((_, required), seen)| *seen || !required)
            }
            _ => false,
        }
    }

    /// The leaves a next child could match, for error messages.
    pub(crate) fn expected<'m>(&'m self, run: &Run) -> Vec<&'m Leaf> {
        let mut out: Vec<&Leaf> = Vec::new();
        match (self, run) {
            (
                Model::Graph(graph),
                Run::Graph {
                    state,
                    counts,
                    vectors,
                },
            ) => {
                let width = graph.chains[*state as usize].len();
                let mut scratch = Vec::new();
                for &edge in graph.out(*state) {
                    let enabled = rows(counts, width, *vectors).any(|vector| {
                        scratch.clear();
                        graph.fire(edge, vector, &mut scratch)
                    });
                    let leaf = graph.leaf(edge.to);
                    if enabled && !out.contains(&leaf) {
                        out.push(leaf);
                    }
                }
            }
            (Model::All(all), Run::All(seen)) => {
                for ((leaves, _), seen) in all.items.iter().zip(seen) {
                    if !seen {
                        out.extend(leaves);
                    }
                }
            }
            _ => {}
        }
        out
    }
}

/// The `count` counter vectors of width `width` packed in `counts`.
fn rows(counts: &[u32], width: usize, count: usize) -> impl Iterator<Item = &[u32]> {
    (0..count).map(move |i| &counts[i * width..(i + 1) * width])
}
