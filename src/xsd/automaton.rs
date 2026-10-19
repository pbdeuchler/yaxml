//! libxml2's content-model automaton, rebuilt only to decide determinism
//! as libxml2 does. `xmlSchemaBuildAContentModel` builds a Thompson-style
//! automaton with counters, `xmlFAEliminateEpsilonTransitions` folds the
//! epsilons, and `xmlFAComputesDeterminism` compares the atoms leaving each
//! state; conflicts hidden behind counter transitions are only partly seen,
//! so the result can differ from Unique Particle Attribution proper. The
//! construction order is libxml2's because the check depends on it.

use super::model::{Atom, Kind, Matched, Node};

#[derive(Clone, Copy, Debug)]
struct Trans {
    atom: Option<usize>,
    /// Target state; negative once removed.
    to: i64,
    counter: i64,
    count: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Type {
    Start,
    Final,
    Transition,
    Sink,
    Unreachable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mark {
    Normal,
    Start,
    Visited,
}

#[derive(Clone, Debug)]
struct State {
    kind: Type,
    trans: Vec<Trans>,
    trans_to: Vec<usize>,
    mark: Mark,
    markd: bool,
    reached: Mark,
}

/// Construction ran out of budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TooComplex;

struct Automaton {
    states: Vec<Option<State>>,
    atoms: Vec<Atom>,
    current: usize,
    counters: i64,
    budget: usize,
}

const UNBOUNDED: Option<u32> = None;

impl Automaton {
    fn charge(&mut self, work: usize) -> Result<(), TooComplex> {
        self.budget = self.budget.checked_sub(work).ok_or(TooComplex)?;
        Ok(())
    }

    fn state(&self, id: usize) -> &State {
        self.states[id].as_ref().expect("live state")
    }

    fn state_mut(&mut self, id: usize) -> &mut State {
        self.states[id].as_mut().expect("live state")
    }

    fn push_state(&mut self) -> Result<usize, TooComplex> {
        self.charge(1)?;
        self.states.push(Some(State {
            kind: Type::Transition,
            trans: Vec::new(),
            trans_to: Vec::new(),
            mark: Mark::Normal,
            markd: false,
            reached: Mark::Normal,
        }));
        Ok(self.states.len() - 1)
    }

    /// `xmlRegStateAddTrans`, ignoring exact duplicates.
    fn add(
        &mut self,
        from: usize,
        atom: Option<usize>,
        to: usize,
        counter: i64,
        count: i64,
    ) -> Result<(), TooComplex> {
        self.charge(1)?;
        let to_i = to as i64;
        let state = self.state_mut(from);
        if state
            .trans
            .iter()
            .any(|t| t.atom == atom && t.to == to_i && t.counter == counter && t.count == count)
        {
            return Ok(());
        }
        state.trans.push(Trans {
            atom,
            to: to_i,
            counter,
            count,
        });
        self.state_mut(to).trans_to.push(from);
        Ok(())
    }

    fn target(&mut self, to: Option<usize>) -> Result<usize, TooComplex> {
        match to {
            Some(to) => Ok(to),
            None => {
                let to = self.push_state()?;
                self.current = to;
                Ok(to)
            }
        }
    }

    /// `xmlAutomataNewEpsilon`.
    fn epsilon(&mut self, from: usize, to: Option<usize>) -> Result<usize, TooComplex> {
        let to = self.target(to)?;
        self.add(from, None, to, -1, -1)?;
        Ok(to)
    }

    /// `xmlAutomataNewTransition2` and `xmlAutomataNewNegTrans`.
    fn transition(
        &mut self,
        from: usize,
        to: Option<usize>,
        atom: Atom,
    ) -> Result<usize, TooComplex> {
        self.atoms.push(atom);
        let atom = self.atoms.len() - 1;
        let to = match to {
            Some(to) => to,
            None => self.push_state()?,
        };
        self.add(from, Some(atom), to, -1, -1)?;
        self.current = to;
        Ok(to)
    }

    fn counter(&mut self) -> i64 {
        self.counters += 1;
        self.counters - 1
    }

    /// `xmlAutomataNewCountedTrans`: increments `counter`.
    fn counted(
        &mut self,
        from: usize,
        to: Option<usize>,
        counter: i64,
    ) -> Result<usize, TooComplex> {
        let to = self.target(to)?;
        self.add(from, None, to, counter, -1)?;
        Ok(to)
    }

    /// `xmlAutomataNewCounterTrans`: allowed when `counter` is in range.
    fn counter_trans(
        &mut self,
        from: usize,
        to: Option<usize>,
        counter: i64,
    ) -> Result<usize, TooComplex> {
        let to = self.target(to)?;
        self.add(from, None, to, -1, counter)?;
        Ok(to)
    }

    /// `xmlSchemaBuildContentModelForSubstGroup` with no counter.
    fn subst_group(
        &mut self,
        atoms: &[Atom],
        min: u32,
        max: Option<u32>,
    ) -> Result<bool, TooComplex> {
        let start = self.current;
        let end = self.push_state()?;
        if max == Some(1) {
            for &atom in atoms {
                let tmp = self.transition(start, None, atom)?;
                self.epsilon(tmp, Some(end))?;
            }
        } else {
            let counter = self.counter();
            let hop = self.push_state()?;
            for &atom in atoms {
                let tmp = self.transition(start, None, atom)?;
                self.epsilon(tmp, Some(hop))?;
            }
            self.counted(hop, Some(start), counter)?;
            self.counter_trans(hop, Some(end), counter)?;
        }
        let nullable = min == 0;
        if nullable {
            self.epsilon(start, Some(end))?;
        }
        self.current = end;
        Ok(nullable)
    }

    /// `xmlSchemaBuildContentModelForElement` for a non-head element.
    fn element(&mut self, atom: Atom, min: u32, max: Option<u32>) -> Result<bool, TooComplex> {
        let start;
        if max == Some(1) {
            start = self.current;
            self.transition(start, None, atom)?;
        } else if max == UNBOUNDED && min < 2 {
            start = self.current;
            let first = self.transition(start, None, atom)?;
            self.transition(first, Some(first), atom)?;
        } else {
            start = self.epsilon(self.current, None)?;
            let counter = self.counter();
            let state = self.transition(start, None, atom)?;
            self.counted(state, Some(start), counter)?;
            self.current = self.counter_trans(state, None, counter)?;
        }
        let nullable = min == 0;
        if nullable {
            let current = self.current;
            self.epsilon(start, Some(current))?;
        }
        Ok(nullable)
    }

    /// The wildcard case of `xmlSchemaBuildAContentModel`: each atom leads
    /// to `end` (or to the counting `hop`) through its own state, except a
    /// negated one, which `xmlAutomataNewNegTrans` links directly.
    fn wildcard(&mut self, atoms: &[Atom], min: u32, max: Option<u32>) -> Result<bool, TooComplex> {
        let start = self.current;
        let end = self.push_state()?;
        let counter = (max != Some(1)).then(|| self.counter());
        let target = match counter {
            Some(_) => self.push_state()?,
            None => end,
        };
        for &atom in atoms {
            match atom.negated {
                true => {
                    self.transition(start, Some(target), atom)?;
                }
                false => {
                    let state = self.transition(start, None, atom)?;
                    self.epsilon(state, Some(target))?;
                }
            }
        }
        if let Some(counter) = counter {
            self.counted(target, Some(start), counter)?;
            self.counter_trans(target, Some(end), counter)?;
        }
        let nullable = min == 0;
        if nullable {
            self.epsilon(start, Some(end))?;
        }
        self.current = end;
        Ok(nullable)
    }

    /// `xmlSchemaBuildAContentModel`; whether the particle is emptiable.
    fn build(&mut self, node: &Node) -> Result<bool, TooComplex> {
        self.charge(1)?;
        let (min, max) = (node.min, node.max);
        match &node.kind {
            Kind::Empty => Ok(true),
            Kind::Abstract => Ok(false),
            Kind::Leaves {
                leaves,
                substitution,
            } => {
                let atoms: Vec<Atom> = leaves.iter().flat_map(|leaf| leaf.atoms()).collect();
                match (substitution, &leaves[0]) {
                    (true, _) => self.subst_group(&atoms, min, max),
                    (false, super::model::Leaf::Wildcard(..)) => self.wildcard(&atoms, min, max),
                    (false, _) => self.element(atoms[0], min, max),
                }
            }
            Kind::Sequence(children) => self.sequence(children, min, max),
            Kind::Choice(children) => self.choice(children, min, max),
        }
    }

    fn children(&mut self, children: &[Node]) -> Result<bool, TooComplex> {
        let mut nullable = true;
        for child in children {
            if !self.build(child)? {
                nullable = false;
            }
        }
        Ok(nullable)
    }

    fn sequence(
        &mut self,
        children: &[Node],
        min: u32,
        max: Option<u32>,
    ) -> Result<bool, TooComplex> {
        if min == 1 && max == Some(1) {
            return self.children(children);
        }
        let mut oldstate = self.current;
        let mut nullable;
        if max == UNBOUNDED {
            if min > 1 {
                self.current = self.epsilon(oldstate, None)?;
                oldstate = self.current;
                let counter = self.counter();
                nullable = self.children(children)?;
                let tmp = self.current;
                self.counted(tmp, Some(oldstate), counter)?;
                self.current = self.counter_trans(tmp, None, counter)?;
                if nullable {
                    let current = self.current;
                    self.epsilon(oldstate, Some(current))?;
                }
            } else {
                self.current = self.epsilon(oldstate, None)?;
                oldstate = self.current;
                nullable = self.children(children)?;
                let current = self.current;
                self.epsilon(current, Some(oldstate))?;
                self.current = self.epsilon(current, None)?;
                if min == 0 {
                    let current = self.current;
                    self.epsilon(oldstate, Some(current))?;
                    nullable = true;
                }
            }
        } else if max.is_some_and(|max| max > 1) || min > 1 {
            self.current = self.epsilon(oldstate, None)?;
            oldstate = self.current;
            let counter = self.counter();
            nullable = self.children(children)?;
            let tmp = self.current;
            self.counted(tmp, Some(oldstate), counter)?;
            self.current = self.counter_trans(tmp, None, counter)?;
            if min == 0 || nullable {
                let current = self.current;
                self.epsilon(oldstate, Some(current))?;
                nullable = true;
            }
        } else {
            nullable = self.children(children)?;
            let current = self.current;
            self.current = self.epsilon(current, None)?;
            if min == 0 {
                let current = self.current;
                self.epsilon(oldstate, Some(current))?;
                nullable = true;
            }
        }
        Ok(nullable)
    }

    fn choice(
        &mut self,
        children: &[Node],
        min: u32,
        max: Option<u32>,
    ) -> Result<bool, TooComplex> {
        let start = self.current;
        let end = self.push_state()?;
        let mut nullable = false;
        if max == Some(1) {
            for child in children {
                self.current = start;
                if self.build(child)? {
                    nullable = true;
                }
                let current = self.current;
                self.epsilon(current, Some(end))?;
            }
        } else {
            let counter = self.counter();
            let hop = self.push_state()?;
            let base = self.push_state()?;
            for child in children {
                self.current = base;
                if self.build(child)? {
                    nullable = true;
                }
                let current = self.current;
                self.epsilon(current, Some(hop))?;
            }
            self.epsilon(start, Some(base))?;
            self.counted(hop, Some(base), counter)?;
            self.counter_trans(hop, Some(end), counter)?;
            if nullable {
                self.epsilon(base, Some(end))?;
            }
        }
        if min == 0 {
            self.epsilon(start, Some(end))?;
            nullable = true;
        }
        self.current = end;
        Ok(nullable)
    }

    // ---- xmlFAEliminateEpsilonTransitions ------------------------------------

    fn eliminate_simple_epsilons(&mut self) -> Result<(), TooComplex> {
        for id in 0..self.states.len() {
            let Some(state) = &self.states[id] else {
                continue;
            };
            if state.trans.len() != 1 || matches!(state.kind, Type::Unreachable | Type::Final) {
                continue;
            }
            let t = state.trans[0];
            if t.atom.is_some() || t.to < 0 || t.to == id as i64 || t.counter >= 0 || t.count >= 0 {
                continue;
            }
            if state.kind == Type::Start {
                continue;
            }
            let newto = t.to as usize;
            for from in state.trans_to.clone() {
                let count = self.state(from).trans.len();
                for j in 0..count {
                    let tj = self.state(from).trans[j];
                    if tj.to == id as i64 {
                        self.state_mut(from).trans[j].to = -1;
                        self.add(from, tj.atom, newto, tj.counter, tj.count)?;
                    }
                }
            }
            let state = self.state_mut(id);
            state.trans.clear();
            state.kind = Type::Unreachable;
        }
        for slot in &mut self.states {
            if slot.as_ref().is_some_and(|s| s.kind == Type::Unreachable) {
                *slot = None;
            }
        }
        Ok(())
    }

    fn reduce(&mut self, from: usize, to: usize, counter: i64) -> Result<(), TooComplex> {
        let Some(target) = &self.states[to] else {
            return Ok(());
        };
        if matches!(target.mark, Mark::Start | Mark::Visited) {
            return Ok(());
        }
        self.charge(1)?;
        self.state_mut(to).mark = Mark::Visited;
        if self.state(to).kind == Type::Final {
            self.state_mut(from).kind = Type::Final;
        }
        let mut i = 0;
        while i < self.state(to).trans.len() {
            let t = self.state(to).trans[i];
            i += 1;
            if t.to < 0 {
                continue;
            }
            let tcounter = if t.counter >= 0 { t.counter } else { counter };
            match t.atom {
                None if t.to == from as i64 => {}
                None if t.count >= 0 => self.add(from, None, t.to as usize, -1, t.count)?,
                None => self.reduce(from, t.to as usize, tcounter)?,
                Some(atom) => self.add(from, Some(atom), t.to as usize, tcounter, -1)?,
            }
        }
        Ok(())
    }

    fn finish_reduce(&mut self, to: usize) {
        let mut stack = vec![to];
        while let Some(id) = stack.pop() {
            let Some(state) = self.states[id].as_mut() else {
                continue;
            };
            if matches!(state.mark, Mark::Start | Mark::Normal) {
                continue;
            }
            state.mark = Mark::Normal;
            stack.extend(
                state
                    .trans
                    .iter()
                    .filter(|t| t.to >= 0 && t.atom.is_none())
                    .map(|t| t.to as usize),
            );
        }
    }

    fn eliminate_epsilons(&mut self) -> Result<(), TooComplex> {
        self.eliminate_simple_epsilons()?;
        let mut has_epsilon = false;
        for id in (0..self.states.len()).rev() {
            let Some(state) = &mut self.states[id] else {
                continue;
            };
            if state.trans.is_empty() && state.kind != Type::Final {
                state.kind = Type::Sink;
            }
            let mut i = 0;
            while i < self.state(id).trans.len() {
                let t = self.state(id).trans[i];
                if t.atom.is_none() && t.to >= 0 {
                    if t.to == id as i64 {
                        self.state_mut(id).trans[i].to = -1;
                    } else if t.count < 0 {
                        has_epsilon = true;
                        self.state_mut(id).trans[i].to = -2;
                        self.state_mut(id).mark = Mark::Start;
                        self.reduce(id, t.to as usize, t.counter)?;
                        self.finish_reduce(t.to as usize);
                        self.state_mut(id).mark = Mark::Normal;
                    }
                }
                i += 1;
            }
        }
        if has_epsilon {
            for state in self.states.iter_mut().flatten() {
                for t in &mut state.trans {
                    if t.atom.is_none() && t.count < 0 && t.to >= 0 {
                        t.to = -1;
                    }
                }
            }
        }
        for state in self.states.iter_mut().flatten() {
            state.reached = Mark::Normal;
        }
        let mut stack = vec![0];
        if let Some(start) = self.states[0].as_mut() {
            start.reached = Mark::Visited;
        }
        while let Some(id) = stack.pop() {
            let targets: Vec<usize> = self
                .state(id)
                .trans
                .iter()
                .filter(|t| t.to >= 0 && (t.atom.is_some() || t.count >= 0))
                .map(|t| t.to as usize)
                .collect();
            for to in targets {
                if let Some(state) = self.states[to].as_mut()
                    && state.reached == Mark::Normal
                {
                    state.reached = Mark::Visited;
                    stack.push(to);
                }
            }
        }
        for slot in &mut self.states {
            if slot.as_ref().is_some_and(|s| s.reached == Mark::Normal) {
                *slot = None;
            }
        }
        Ok(())
    }

    // ---- xmlFAComputesDeterminism ----------------------------------------------

    fn compare(&self, a: usize, b: usize) -> bool {
        a == b || self.atoms[a].conflicts(&self.atoms[b])
    }

    fn equal(&self, a: usize, b: usize) -> bool {
        let (x, y) = (&self.atoms[a], &self.atoms[b]);
        a == b || (x.local == y.local && x.ns == y.ns)
    }

    /// `xmlFARecurseDeterminism`.
    fn recurse(&mut self, state: usize, from: i64, to: i64, atom: usize) -> bool {
        let Some(s) = &self.states[state] else {
            return true;
        };
        if s.markd {
            return true;
        }
        let mut deterministic = true;
        let count = s.trans.len();
        for i in 0..count {
            let t = self.state(state).trans[i];
            if t.to < 0 || t.to == from {
                continue;
            }
            match t.atom {
                None => {
                    self.state_mut(state).markd = true;
                    if !self.recurse(t.to as usize, from, to, atom) {
                        deterministic = false;
                    }
                }
                Some(other) => {
                    if self.compare(other, atom) && (t.to != to || !self.equal(other, atom)) {
                        deterministic = false;
                    }
                }
            }
        }
        deterministic
    }

    fn finish_recurse(&mut self, state: usize) {
        let mut stack = vec![state];
        while let Some(id) = stack.pop() {
            let Some(s) = self.states.get_mut(id).and_then(Option::as_mut) else {
                continue;
            };
            if !s.markd {
                continue;
            }
            s.markd = false;
            stack.extend(
                s.trans
                    .iter()
                    .filter(|t| t.atom.is_none() && t.to >= 0)
                    .map(|t| t.to as usize),
            );
        }
    }

    /// Drops equal duplicate transitions, keeping the later one and
    /// recording which particle it hides, then compares the rest.
    fn deterministic(&mut self, shadowed: &mut Vec<(Matched, Matched)>) -> bool {
        for id in 0..self.states.len() {
            let Some(state) = &self.states[id] else {
                continue;
            };
            if state.trans.len() < 2 {
                continue;
            }
            for i in 0..state.trans.len() {
                let t1 = self.state(id).trans[i];
                let Some(a1) = t1.atom else { continue };
                if t1.to < 0 {
                    continue;
                }
                for j in 0..i {
                    let t2 = self.state(id).trans[j];
                    if t2.to < 0 {
                        continue;
                    }
                    if let Some(a2) = t2.atom
                        && t1.to == t2.to
                        && self.equal(a1, a2)
                        && t1.counter == t2.counter
                        && t1.count == t2.count
                    {
                        self.state_mut(id).trans[j].to = -1;
                        let pair = (self.atoms[a2].origin, self.atoms[a1].origin);
                        if pair.0 != pair.1 && !shadowed.contains(&pair) {
                            shadowed.push(pair);
                        }
                    }
                }
            }
        }
        let mut deterministic = true;
        for id in 0..self.states.len() {
            let Some(state) = &self.states[id] else {
                continue;
            };
            if state.trans.len() < 2 {
                continue;
            }
            for i in 0..state.trans.len() {
                let t1 = self.state(id).trans[i];
                let Some(a1) = t1.atom else { continue };
                if t1.to < 0 {
                    continue;
                }
                for j in 0..i {
                    let t2 = self.state(id).trans[j];
                    if t2.to < 0 {
                        continue;
                    }
                    match t2.atom {
                        Some(a2) => {
                            if self.compare(a1, a2)
                                && (t1.to != t2.to
                                    || t1.counter == t2.counter
                                    || !self.equal(a1, a2))
                            {
                                deterministic = false;
                            }
                        }
                        None => {
                            let ok = self.recurse(t2.to as usize, id as i64, t1.to, a1);
                            self.finish_recurse(t2.to as usize);
                            if !ok {
                                deterministic = false;
                            }
                        }
                    }
                }
            }
        }
        deterministic
    }
}

/// Whether libxml2 would find the content model `node` deterministic, with
/// the particles it shadows (see [`Automaton::deterministic`]); `Err` when
/// `budget` runs out.
pub(crate) fn deterministic(
    node: &Node,
    budget: &mut usize,
) -> Result<(bool, Vec<(Matched, Matched)>), TooComplex> {
    let mut automaton = Automaton {
        states: Vec::new(),
        atoms: Vec::new(),
        current: 0,
        counters: 0,
        budget: *budget,
    };
    automaton.push_state()?;
    automaton.state_mut(0).kind = Type::Start;
    automaton.build(node)?;
    let last = automaton.current;
    automaton.state_mut(last).kind = Type::Final;
    automaton.eliminate_epsilons()?;
    let work = automaton
        .states
        .iter()
        .flatten()
        .map(|s| s.trans.len().pow(2))
        .sum();
    automaton.charge(work)?;
    let mut shadowed = Vec::new();
    let result = automaton.deterministic(&mut shadowed);
    *budget = automaton.budget;
    Ok((result, shadowed))
}
