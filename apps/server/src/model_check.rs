//! A small explicit-state model checker for the server's own protocols.
//!
//! A model is a set of starting states and the steps that can follow each
//! state. `explore` visits every state the steps can reach, breadth first,
//! so the path it reports to a state is a shortest one. A property is then a
//! question about the reachable states: one that must hold in all of them
//! (`breaks`), or one that some state must reach (`reaches`).
//!
//! A model that uses it (`idempotency/model.rs`, `jobs/model.rs`) calls the same
//! pure decision functions the server runs, so a change to one of those is
//! checked against every order of steps, not only the orders someone wrote a
//! test for.

use std::{
    collections::{HashMap, VecDeque},
    fmt::{self, Debug, Display},
    hash::Hash,
};

pub(crate) trait Model {
    type State: Clone + Eq + Hash + Debug;
    type Step: Clone + Display;

    fn start(&self) -> Self::State;

    /// Every step that can happen next, with the state it leads to.
    fn steps(&self, state: &Self::State) -> Vec<(Self::Step, Self::State)>;
}

/// The state and step a state was first reached by; none for the start.
type ReachedBy<M> = Option<(<M as Model>::State, <M as Model>::Step)>;

pub(crate) struct Explored<M: Model> {
    reached: HashMap<M::State, ReachedBy<M>>,
    order: Vec<M::State>,
}

/// The steps that lead to a state, in the model's own words.
pub(crate) struct Path<M: Model> {
    pub(crate) steps: Vec<M::Step>,
    pub(crate) end: M::State,
}

impl<M: Model> Display for Path<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, step) in self.steps.iter().enumerate() {
            writeln!(formatter, "  {}. {step}", index + 1)?;
        }
        write!(formatter, "  ends in {:?}", self.end)
    }
}

impl<M: Model> Debug for Path<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, formatter)
    }
}

/// Visits every reachable state. A model whose states don't stop within
/// `limit` is a mistake in the model, so it panics rather than reporting on
/// part of it.
pub(crate) fn explore<M: Model>(model: &M, limit: usize) -> Explored<M> {
    let start = model.start();
    let mut reached = HashMap::from([(start.clone(), None)]);
    let mut order = vec![start.clone()];
    let mut queue = VecDeque::from([start]);
    while let Some(state) = queue.pop_front() {
        for (step, next) in model.steps(&state) {
            if reached.contains_key(&next) {
                continue;
            }
            assert!(
                reached.len() < limit,
                "the model has more than {limit} states: bound it"
            );
            reached.insert(next.clone(), Some((state.clone(), step)));
            order.push(next.clone());
            queue.push_back(next);
        }
    }
    Explored { reached, order }
}

/// Fails a test with the steps that lead to a state that must not be
/// reached, one to a line.
#[track_caller]
pub(crate) fn never<M: Model>(found: Option<Path<M>>) {
    if let Some(path) = found {
        panic!("reached by:\n{path}");
    }
}

impl<M: Model> Explored<M> {
    pub(crate) fn states(&self) -> usize {
        self.order.len()
    }

    /// A shortest path to a state where `holds` is false, if there is one.
    pub(crate) fn breaks(&self, holds: impl Fn(&M::State) -> bool) -> Option<Path<M>> {
        self.reaches(|state| !holds(state))
    }

    /// A shortest path to a state where `wanted` is true, if there is one.
    pub(crate) fn reaches(&self, wanted: impl Fn(&M::State) -> bool) -> Option<Path<M>> {
        let end = self.order.iter().find(|state| wanted(state))?;
        let mut steps = Vec::new();
        let mut at = end;
        while let Some(Some((before, step))) = self.reached.get(at) {
            steps.push(step.clone());
            at = before;
        }
        steps.reverse();
        Some(Path {
            steps,
            end: end.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two counters that each step raises by one, up to a cap.
    struct Counters {
        cap: u8,
    }

    impl Model for Counters {
        type State = (u8, u8);
        type Step = &'static str;

        fn start(&self) -> Self::State {
            (0, 0)
        }

        fn steps(&self, &(left, right): &Self::State) -> Vec<(Self::Step, Self::State)> {
            let mut steps = Vec::new();
            if left < self.cap {
                steps.push(("left", (left + 1, right)));
            }
            if right < self.cap {
                steps.push(("right", (left, right + 1)));
            }
            steps
        }
    }

    #[test]
    fn every_state_is_visited_once_and_paths_are_shortest() {
        let explored = explore(&Counters { cap: 3 }, 100);
        assert_eq!(explored.states(), 16);
        assert!(
            explored
                .breaks(|&(left, right)| left <= 3 && right <= 3)
                .is_none()
        );
        let path = explored
            .breaks(|&(left, right)| left + right < 4)
            .expect("a state with four steps taken");
        assert_eq!(path.steps.len(), 4);
        assert!(explored.reaches(|&state| state == (3, 3)).is_some());
        assert!(explored.reaches(|&state| state == (4, 0)).is_none());
    }

    #[test]
    #[should_panic(expected = "bound it")]
    fn a_model_that_never_stops_is_refused() {
        explore(&Counters { cap: u8::MAX }, 100);
    }
}
