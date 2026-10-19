//! Element content models compiled to their Glushkov (position) automaton.
//!
//! XML 1.0 Appendix E requires models to be deterministic, which is exactly
//! a Glushkov automaton with one transition per state and name. Such an
//! automaton matches children in one lookup each. Construction is bounded by
//! a caller-supplied work budget, because follow sets can be quadratic in the
//! number of names (`(a?, b?, c?, ...)`); there is no subset construction.

use crate::dtd::{Connector, ModelTokenView, Occurrence};

/// A compiled `children` content model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Model {
    /// Sorted, distinct qualified names.
    names: Vec<Box<str>>,
    /// `(state, name, target)`, sorted. State 0 is the start; state `p + 1`
    /// is entered by matching position `p`. Empty when not deterministic.
    transitions: Vec<(u32, u32, u32)>,
    accepting: Vec<bool>,
    deterministic: bool,
}

/// The work budget ran out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TooComplex;

/// A particle's nullability and first/last positions.
#[derive(Default)]
struct Fragment {
    nullable: bool,
    first: Vec<u32>,
    last: Vec<u32>,
}

#[derive(Default)]
struct Group {
    connector: Option<Connector>,
    fragment: Option<Fragment>,
}

struct Builder<'a> {
    positions: Vec<&'a str>,
    follow: Vec<Vec<u32>>,
    budget: usize,
}

impl Builder<'_> {
    fn charge(&mut self, work: usize) -> Result<(), TooComplex> {
        self.budget = self.budget.checked_sub(work).ok_or(TooComplex)?;
        Ok(())
    }

    /// Adds `targets` to the follow set of every position in `sources`.
    fn link(&mut self, sources: &[u32], targets: &[u32]) -> Result<(), TooComplex> {
        self.charge(sources.len().saturating_mul(targets.len()))?;
        for &source in sources {
            self.follow[source as usize].extend_from_slice(targets);
        }
        Ok(())
    }

    fn join(&mut self, mut left: Vec<u32>, right: &[u32]) -> Result<Vec<u32>, TooComplex> {
        self.charge(right.len())?;
        left.extend_from_slice(right);
        Ok(left)
    }

    fn combine(
        &mut self,
        connector: Option<Connector>,
        left: Fragment,
        right: Fragment,
    ) -> Result<Fragment, TooComplex> {
        Ok(match connector {
            Some(Connector::Choice) => Fragment {
                nullable: left.nullable || right.nullable,
                first: self.join(left.first, &right.first)?,
                last: self.join(left.last, &right.last)?,
            },
            _ => {
                self.link(&left.last, &right.first)?;
                Fragment {
                    nullable: left.nullable && right.nullable,
                    first: match left.nullable {
                        true => self.join(left.first, &right.first)?,
                        false => left.first,
                    },
                    last: match right.nullable {
                        true => self.join(left.last, &right.last)?,
                        false => right.last,
                    },
                }
            }
        })
    }

    fn repeat(
        &mut self,
        fragment: Fragment,
        occurrence: Occurrence,
    ) -> Result<Fragment, TooComplex> {
        if occurrence != Occurrence::Optional {
            self.link(&fragment.last, &fragment.first)?;
        }
        Ok(Fragment {
            nullable: fragment.nullable || occurrence != Occurrence::OneOrMore,
            ..fragment
        })
    }

    /// Folds the particle just completed into its enclosing group.
    fn flush(
        &mut self,
        stack: &mut [Group],
        done: &mut Option<Fragment>,
    ) -> Result<(), TooComplex> {
        if let (Some(fragment), Some(group)) = (done.take(), stack.last_mut()) {
            group.fragment = Some(match group.fragment.take() {
                Some(left) => self.combine(group.connector, left, fragment)?,
                None => fragment,
            });
        }
        Ok(())
    }
}

impl Model {
    /// Compiles the tokens of a well-formed `children` model.
    pub(crate) fn compile<'a>(
        tokens: impl IntoIterator<Item = ModelTokenView<'a>>,
        budget: &mut usize,
    ) -> Result<Self, TooComplex> {
        let mut builder = Builder {
            positions: Vec::new(),
            follow: Vec::new(),
            budget: *budget,
        };
        let mut stack: Vec<Group> = Vec::new();
        let mut done = None;
        for token in tokens {
            match token {
                ModelTokenView::Open => {
                    builder.flush(&mut stack, &mut done)?;
                    stack.push(Group::default());
                }
                ModelTokenView::Name(name) => {
                    builder.flush(&mut stack, &mut done)?;
                    let position = builder.positions.len() as u32;
                    builder.positions.push(name);
                    builder.follow.push(Vec::new());
                    done = Some(Fragment {
                        nullable: false,
                        first: vec![position],
                        last: vec![position],
                    });
                }
                ModelTokenView::Occurrence(occurrence) => {
                    let fragment = done.take().unwrap_or_default();
                    done = Some(builder.repeat(fragment, occurrence)?);
                }
                ModelTokenView::Connector(connector) => {
                    builder.flush(&mut stack, &mut done)?;
                    if let Some(group) = stack.last_mut() {
                        group.connector = Some(connector);
                    }
                }
                ModelTokenView::Close => {
                    builder.flush(&mut stack, &mut done)?;
                    done = stack.pop().and_then(|group| group.fragment);
                }
                ModelTokenView::Pcdata => {}
            }
        }
        let root = done.unwrap_or_default();
        *budget = builder.budget;
        let mut names: Vec<Box<str>> = builder.positions.iter().map(|&name| name.into()).collect();
        names.sort_unstable();
        names.dedup();
        let label = |position: u32| {
            names
                .binary_search_by(|name| (**name).cmp(builder.positions[position as usize]))
                .expect("interned") as u32
        };
        let mut transitions = Vec::new();
        let mut deterministic = true;
        for (state, targets) in std::iter::once(root.first.clone())
            .chain(builder.follow)
            .enumerate()
        {
            let mut row: Vec<_> = targets
                .into_iter()
                .map(|position| (state as u32, label(position), position + 1))
                .collect();
            row.sort_unstable();
            row.dedup();
            deterministic &= row.windows(2).all(|pair| pair[0].1 != pair[1].1);
            transitions.extend(row);
        }
        let mut accepting = vec![false; builder.positions.len() + 1];
        accepting[0] = root.nullable;
        for position in root.last {
            accepting[position as usize + 1] = true;
        }
        if !deterministic {
            transitions = Vec::new();
        }
        Ok(Self {
            names,
            transitions,
            accepting,
            deterministic,
        })
    }

    pub(crate) fn deterministic(&self) -> bool {
        self.deterministic
    }

    /// Approximate retained bytes, for storage accounting.
    pub(crate) fn bytes(&self) -> usize {
        self.names.iter().map(|name| name.len() + 16).sum::<usize>()
            + self.transitions.len() * 12
            + self.accepting.len()
    }

    /// The state after child `name`, or `None` when the model rejects it.
    pub(crate) fn step(&self, state: u32, name: &str) -> Option<u32> {
        let label = self.names.binary_search_by(|n| (**n).cmp(name)).ok()? as u32;
        let index = self
            .transitions
            .binary_search_by(|&(s, n, _)| (s, n).cmp(&(state, label)))
            .ok()?;
        Some(self.transitions[index].2)
    }

    pub(crate) fn accepts(&self, state: u32) -> bool {
        self.accepting[state as usize]
    }
}

#[cfg(test)]
#[path = "../../tests/internal/valid_model.rs"]
mod tests;
