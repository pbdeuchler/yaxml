//! Content models compiled to Glushkov automata extended with counters.
//!
//! Positions are the element and wildcard particles. A repetition with a
//! finite `maxOccurs` above one (or a `minOccurs` above one) gets a counter
//! instead of being unrolled, so `maxOccurs="1000000"` costs one counter.
//! Each transition records how many enclosing counters it keeps, whether it
//! starts another iteration of the innermost kept one, and which counters it
//! leaves (they must have run `minOccurs` times) or enters (set to one).
//!
//! A run is the set of reachable configurations (position and counter
//! values), so matching decides membership without backtracking; for
//! deterministic models it holds one configuration except under nested
//! repetitions such as `(a{2,3}){2}`, and it is capped. Construction spends
//! a work budget because follow sets can be quadratic in the positions.
//! Whether a model is deterministic is decided separately, as libxml2 does,
//! by [`super::automaton`].

use std::collections::HashSet;

use super::components::{ElemId, NO_NS, NsConstraint, QName, Sym, WildId};

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

    /// libxml2's automaton atoms for this leaf, in its order: `name` or
    /// `name|ns`; `*|*` then `*` for `##any`; `*|ns` or `*` per listed
    /// namespace; a negated `*|ns` or `*` for `##other`.
    pub(crate) fn atoms(&self) -> Vec<Atom> {
        let ns_part = |ns: Sym| (ns != NO_NS).then_some(Some(ns));
        let origin = self.target();
        let any = |ns| Atom {
            negated: false,
            local: None,
            ns,
            origin,
        };
        match self {
            Leaf::Element(name, _) => vec![Atom {
                negated: false,
                local: Some(name.local),
                ns: ns_part(name.ns),
                origin,
            }],
            Leaf::Wildcard(NsConstraint::Any, _) => vec![any(Some(None)), any(None)],
            Leaf::Wildcard(NsConstraint::Set(set), _) => {
                set.iter().map(|&ns| any(ns_part(ns))).collect()
            }
            Leaf::Wildcard(NsConstraint::Not(ns), _) => vec![Atom {
                negated: true,
                local: None,
                ns: ns_part(*ns),
                origin,
            }],
        }
    }
}

/// A libxml2 regexp string atom `local|ns`, `None` standing for `*`; `ns`
/// is absent for atoms without a `|` part.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Atom {
    pub(crate) negated: bool,
    pub(crate) local: Option<Sym>,
    pub(crate) ns: Option<Option<Sym>>,
    /// The particle the atom stands for.
    pub(crate) origin: Matched,
}

impl Atom {
    /// `xmlFACompareAtoms`: atoms with and without a namespace part never
    /// conflict, so `##any` and a `##other` of a schema without target
    /// namespace do not either.
    pub(crate) fn conflicts(&self, other: &Atom) -> bool {
        if self.ns.is_some() != other.ns.is_some() {
            return false;
        }
        let part = |a: Option<Sym>, b: Option<Sym>| a.is_none() || b.is_none() || a == b;
        let equal = part(self.local, other.local)
            && match (self.ns, other.ns) {
                (Some(a), Some(b)) => part(a, b),
                _ => true,
            };
        equal != (self.negated != other.negated)
    }
}

/// A particle: a term with its occurrence range (`None` is unbounded).
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) min: u32,
    pub(crate) max: Option<u32>,
    pub(crate) kind: Kind,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    /// An element, a substitution group (head first) or a wildcard.
    Leaves {
        leaves: Vec<Leaf>,
        substitution: bool,
    },
    /// An abstract element without substitutes: libxml2 adds no transition.
    Abstract,
    /// A term that did not resolve, or an empty group definition.
    Empty,
    Sequence(Vec<Node>),
    Choice(Vec<Node>),
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
    /// Particles libxml2 never reports because its determinism pass drops
    /// their transitions in favour of a later equal one, with that one.
    shadowed: Vec<(Matched, Matched)>,
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

    fn choice(&mut self, whole: Fragment, next: Fragment) -> Result<Fragment, TooComplex> {
        Ok(Fragment {
            nullable: whole.nullable || next.nullable,
            first: self.join(whole.first, &next.first)?,
            last: self.join(whole.last, &next.last)?,
        })
    }

    /// The fragment of the term of `node`, without its repetition.
    fn term(&mut self, node: &Node, chain: &mut Vec<u32>) -> Result<Fragment, TooComplex> {
        let mut whole = Fragment::default();
        match &node.kind {
            Kind::Abstract | Kind::Empty => whole.nullable = true,
            Kind::Leaves { leaves, .. } => {
                for leaf in leaves {
                    self.leaves.push(leaf.clone());
                    self.chains.push(chain.clone());
                    let state = self.leaves.len() as u32;
                    let next = Fragment {
                        nullable: false,
                        first: vec![state],
                        last: vec![state],
                    };
                    whole = self.choice(whole, next)?;
                }
            }
            Kind::Choice(items) => {
                for item in items {
                    let next = self.build(item, chain)?;
                    whole = self.choice(whole, next)?;
                }
            }
            Kind::Sequence(items) => {
                whole.nullable = true;
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
            }
        }
        Ok(whole)
    }

    /// States of the positions; `chain` holds the counters enclosing `node`.
    fn build(&mut self, node: &Node, chain: &mut Vec<u32>) -> Result<Fragment, TooComplex> {
        self.charge(1)?;
        let (min, max) = (node.min, node.max);
        if max == Some(0) {
            return Ok(Fragment {
                nullable: true,
                ..Fragment::default()
            });
        }
        let counted = min > 1 || max.is_some_and(|max| max > 1);
        if counted {
            chain.push(self.counters.len() as u32);
            self.counters.push(Counter {
                min,
                max,
                nullable: false,
            });
        }
        let inner = self.term(node, chain)?;
        if max != Some(1) {
            let keep = chain.len() - usize::from(counted);
            self.link(&inner.last, &inner.first, keep, counted)?;
        }
        if counted {
            let id = chain.pop().expect("pushed") as usize;
            self.counters[id].nullable = inner.nullable;
        }
        Ok(Fragment {
            nullable: inner.nullable || min == 0,
            ..inner
        })
    }
}

impl Graph {
    /// Compiles `node`, spending `budget`.
    pub(crate) fn compile(
        node: &Node,
        shadowed: Vec<(Matched, Matched)>,
        budget: &mut usize,
    ) -> Result<Graph, TooComplex> {
        let mut builder = Builder {
            leaves: Vec::new(),
            chains: vec![Vec::new()],
            counters: Vec::new(),
            edges: Vec::new(),
            budget: *budget,
        };
        let root = builder.build(node, &mut Vec::new())?;
        builder.link(&[0], &root.first, 0, false)?;
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
        Ok(Graph {
            leaves: builder.leaves,
            chains: builder.chains,
            counters: builder.counters,
            edges,
            offsets,
            accepting,
            shadowed,
        })
    }

    fn leaf(&self, state: u32) -> &Leaf {
        &self.leaves[state as usize - 1]
    }

    fn out(&self, state: u32) -> &[Edge] {
        let range = self.offsets[state as usize]..self.offsets[state as usize + 1];
        &self.edges[range.start as usize..range.end as usize]
    }

    fn exitable(&self, state: u32, counts: &[u32], from: usize) -> bool {
        let chain = &self.chains[state as usize];
        (from..chain.len()).all(|i| self.counters[chain[i] as usize].exitable(counts[i]))
    }

    /// The counters after `edge` from `counts`, if its guards pass.
    fn fire(&self, edge: Edge, counts: &[u32]) -> Option<Vec<u32>> {
        let keep = edge.keep as usize;
        if !self.exitable(edge.from, counts, keep + usize::from(edge.bump)) {
            return None;
        }
        let mut out = counts[..keep].to_vec();
        if edge.bump {
            let counter = self.counters[self.chains[edge.from as usize][keep] as usize];
            out.push(match counter.max {
                Some(max) if counts[keep] >= max => return None,
                Some(_) => counts[keep] + 1,
                None => (counts[keep] + 1).min(counter.min.max(1)),
            });
        }
        let width = self.chains[edge.to as usize].len();
        out.resize(width, 1);
        Some(out)
    }
}

/// Where a child matched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Matched {
    Element(ElemId),
    Wildcard(WildId),
}

impl Leaf {
    pub(crate) fn target(&self) -> Matched {
        match *self {
            Leaf::Element(_, id) => Matched::Element(id),
            Leaf::Wildcard(_, id) => Matched::Wildcard(id),
        }
    }
}

/// A child was not expected, or there were too many configurations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepError {
    Unexpected,
    TooComplex,
}

/// A configuration: a state and its counter values.
type Config = (u32, Vec<u32>);

/// The matching state of one element's children.
#[derive(Clone, Debug)]
pub(crate) enum Run {
    Graph(Vec<Config>),
    All(Vec<bool>),
}

impl Model {
    pub(crate) fn start(&self) -> Run {
        match self {
            Model::Graph(_) => Run::Graph(vec![(0, Vec::new())]),
            Model::All(all) => Run::All(vec![false; all.items.len()]),
        }
    }

    /// Consumes one child named `name`; the first matching position decides
    /// its declaration.
    pub(crate) fn step(
        &self,
        run: &mut Run,
        name: QName,
        cap: usize,
    ) -> Result<Matched, StepError> {
        match (self, run) {
            (Model::Graph(graph), Run::Graph(configs)) => {
                let mut next = Vec::new();
                let mut seen = HashSet::new();
                let mut matched = None;
                for (state, counts) in configs.iter() {
                    for &edge in graph.out(*state) {
                        let leaf = graph.leaf(edge.to);
                        if !leaf.matches(name) {
                            continue;
                        }
                        let Some(counts) = graph.fire(edge, counts) else {
                            continue;
                        };
                        matched.get_or_insert(leaf.target());
                        let config = (edge.to, counts);
                        if seen.insert(config.clone()) {
                            next.push(config);
                            if next.len() > cap {
                                return Err(StepError::TooComplex);
                            }
                        }
                    }
                }
                let matched = matched.ok_or(StepError::Unexpected)?;
                *configs = next;
                // Follow the chain of hidden particles to the one kept.
                let shown = std::iter::successors(Some(matched), |hidden| {
                    graph
                        .shadowed
                        .iter()
                        .find(|(h, _)| h == hidden)
                        .map(|&(_, shown)| shown)
                });
                Ok(shown.take(graph.shadowed.len() + 1).last().expect("first"))
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
            (Model::Graph(graph), Run::Graph(configs)) => configs.iter().any(|(state, counts)| {
                graph.accepting[*state as usize] && graph.exitable(*state, counts, 0)
            }),
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
            (Model::Graph(graph), Run::Graph(configs)) => {
                for (state, counts) in configs {
                    for &edge in graph.out(*state) {
                        let leaf = graph.leaf(edge.to);
                        if graph.fire(edge, counts).is_some() && !out.contains(&leaf) {
                            out.push(leaf);
                        }
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
