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
        let Fragment {
            nullable,
            first,
            last,
        } = done.unwrap_or_default();
        let Builder {
            positions,
            mut follow,
            budget: left,
        } = builder;
        let mut names: Vec<Box<str>> = positions.iter().map(|&name| name.into()).collect();
        names.sort_unstable();
        names.dedup();
        let labels: Vec<u32> = positions
            .iter()
            .map(|&position| {
                names
                    .binary_search_by(|name| (**name).cmp(position))
                    .expect("interned") as u32
            })
            .collect();
        let label = |position: u32| labels[position as usize];
        // Every row is sorted and checked before any transition is kept (an
        // ambiguous model keeps none); the kept ones are charged, at three
        // follow-entry units (12 bytes) each, before they are built.
        follow.insert(0, first);
        let (mut kept, mut deterministic) = (0usize, true);
        for row in &mut follow {
            row.sort_unstable_by_key(|&position| (label(position), position));
            row.dedup();
            kept += row.len();
            if row.windows(2).any(|pair| label(pair[0]) == label(pair[1])) {
                deterministic = false;
                break;
            }
        }
        let kept = kept * usize::from(deterministic);
        *budget = left.checked_sub(kept.saturating_mul(3)).ok_or(TooComplex)?;
        let mut transitions = Vec::with_capacity(kept);
        for (state, row) in follow.iter().enumerate().take_while(|_| deterministic) {
            let state = state as u32;
            transitions.extend(row.iter().map(|&p| (state, label(p), p + 1)));
        }
        let mut accepting = vec![false; positions.len() + 1];
        accepting[0] = nullable;
        for position in last {
            accepting[position as usize + 1] = true;
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
