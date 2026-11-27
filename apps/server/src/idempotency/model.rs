//! Every order of steps for one request id, checked against the promise a
//! request id makes: however a call is retried, its write is made once.
//!
//! The decision at each arrival is the server's own `standing`. The steps
//! around it are the dispatcher's, in `ops/dispatch.rs::dispatch`: with the
//! write (`call.tx()`, where the id, the write and the answer commit
//! together) or apart from it (`OWN_TRANSACTION`: `reserve_apart` commits,
//! the command runs in its own transactions, then `complete` or `release`
//! commits). Each step here is one transaction there; the advisory lock in
//! `reserve` is what makes an arrival one step.
//!
//! Not in the model: a different request under the same id (`Mismatch`),
//! what a replayed answer contains, and whether a replay re-checks access.
//! `.claude/skills/request-receipts/SKILL.md` says how to read a failure.

use std::fmt;

use super::{Kept, Standing, standing};
use crate::model_check::{Model, Path, explore, never};

/// Where the request id is kept relative to the command's write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reserved {
    WithTheWrite,
    Apart,
}

/// When an unanswered reservation lapses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnansweredLife {
    /// With the id, a day after it was reserved.
    ADay,
    /// `reserve_briefly`, before #358: after ten minutes.
    TenMinutes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    FailsOnlyBeforeItsCommit,
    MayFailAfterItsCommit,
}

#[derive(Clone, Copy, Debug)]
struct Receipts {
    calls: usize,
    reserved: Reserved,
    unanswered_life: UnansweredLife,
    command: Command,
    /// Whether time may run past the day an id is kept for.
    past_a_day: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Age {
    Fresh,
    TenMinutes,
    /// `STALLED_AFTER`.
    HalfAnHour,
    ADay,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Row {
    answered: bool,
    /// Since it was reserved, or since its answer was kept (`complete`
    /// pushes `expires_at` a day on).
    age: Age,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Call {
    NotSent,
    Arrived,
    Reserved,
    Wrote,
    Failing,
    Ended(Answer),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Answer {
    Done,
    Replayed,
    /// `command in progress`: the caller waits and asks again.
    Wait,
    OutcomeUnknown,
    Refused,
    /// The server stopped: the caller heard nothing.
    Lost,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct World {
    row: Option<Row>,
    writes: u8,
    calls: Vec<Call>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Step {
    Arrives(usize),
    Reserves(usize),
    Replays(usize),
    ToldToWait(usize),
    ToldOutcomeUnknown(usize),
    CommandRefuses(usize),
    CommandCommits(usize),
    CommandFailsAfterItsCommit(usize),
    GivesBack(usize),
    KeepsAnswer(usize),
    CannotKeepAnswer(usize),
    RunsInOneTransaction(usize),
    RollsBack(usize),
    Stops(usize),
    TimePasses(Age),
}

impl fmt::Display for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let call = |index: &usize| index + 1;
        match self {
            Self::Arrives(index) => write!(formatter, "call {} arrives", call(index)),
            Self::Reserves(index) => {
                write!(
                    formatter,
                    "call {} reserves the id and commits",
                    call(index)
                )
            }
            Self::Replays(index) => {
                write!(
                    formatter,
                    "call {} is answered with the kept answer",
                    call(index)
                )
            }
            Self::ToldToWait(index) => write!(
                formatter,
                "call {} is told the first call is still running",
                call(index)
            ),
            Self::ToldOutcomeUnknown(index) => write!(
                formatter,
                "call {} is refused: the first call's outcome is unknown",
                call(index)
            ),
            Self::CommandRefuses(index) => write!(
                formatter,
                "call {}'s command refuses, having made nothing",
                call(index)
            ),
            Self::CommandCommits(index) => {
                write!(
                    formatter,
                    "call {}'s command commits its write",
                    call(index)
                )
            }
            Self::CommandFailsAfterItsCommit(index) => write!(
                formatter,
                "call {}'s command fails after its commit",
                call(index)
            ),
            Self::GivesBack(index) => {
                write!(formatter, "call {} gives its reservation back", call(index))
            }
            Self::KeepsAnswer(index) => {
                write!(
                    formatter,
                    "call {} keeps its answer under the id",
                    call(index)
                )
            }
            Self::CannotKeepAnswer(index) => write!(
                formatter,
                "call {} finds its reservation gone and answers with an error",
                call(index)
            ),
            Self::RunsInOneTransaction(index) => write!(
                formatter,
                "call {} reserves the id, writes and keeps its answer in one transaction",
                call(index)
            ),
            Self::RollsBack(index) => {
                write!(formatter, "call {}'s transaction rolls back", call(index))
            }
            Self::Stops(index) => write!(formatter, "call {}'s server stops", call(index)),
            Self::TimePasses(age) => write!(formatter, "time passes: {age:?}"),
        }
    }
}

impl Receipts {
    fn kept(&self, row: Row) -> Kept<()> {
        let lapses_at = match self.unanswered_life {
            UnansweredLife::TenMinutes if !row.answered => Age::TenMinutes,
            UnansweredLife::TenMinutes | UnansweredLife::ADay => Age::ADay,
        };
        Kept {
            same_request: true,
            answer: row.answered.then_some(()),
            stalled: row.age >= Age::HalfAnHour,
            lapsed: row.age >= lapses_at,
        }
    }

    fn standing(&self, world: &World) -> Standing<()> {
        standing(world.row.map(|row| self.kept(row)))
    }
}

impl Model for Receipts {
    type State = World;
    type Step = Step;

    fn start(&self) -> World {
        World {
            row: None,
            writes: 0,
            calls: vec![Call::NotSent; self.calls],
        }
    }

    fn steps(&self, world: &World) -> Vec<(Step, World)> {
        let mut steps = Vec::new();
        let mut then = |step: Step, change: &dyn Fn(&mut World)| {
            let mut next = world.clone();
            change(&mut next);
            steps.push((step, next));
        };
        let fresh = Row {
            answered: false,
            age: Age::Fresh,
        };

        for (index, call) in world.calls.iter().enumerate() {
            let end = |answer| move |next: &mut World| next.calls[index] = Call::Ended(answer);
            match call {
                Call::NotSent => {
                    let earlier_sent = world.calls[..index]
                        .iter()
                        .all(|call| *call != Call::NotSent);
                    if earlier_sent {
                        then(Step::Arrives(index), &|next| {
                            next.calls[index] = Call::Arrived
                        });
                    }
                }
                Call::Arrived => {
                    match (self.standing(world), self.reserved) {
                        (Standing::Free, Reserved::Apart) => {
                            then(Step::Reserves(index), &|next| {
                                next.row = Some(fresh);
                                next.calls[index] = Call::Reserved;
                            });
                        }
                        (Standing::Free, Reserved::WithTheWrite) => {
                            then(Step::RunsInOneTransaction(index), &|next| {
                                next.row = Some(Row {
                                    answered: true,
                                    age: Age::Fresh,
                                });
                                next.writes += 1;
                                next.calls[index] = Call::Ended(Answer::Done);
                            });
                            then(Step::RollsBack(index), &end(Answer::Refused));
                        }
                        (Standing::Answered(()), _) => {
                            then(Step::Replays(index), &end(Answer::Replayed));
                        }
                        (Standing::Running, _) => {
                            then(Step::ToldToWait(index), &end(Answer::Wait));
                        }
                        (Standing::OutcomeUnknown, _) => {
                            then(
                                Step::ToldOutcomeUnknown(index),
                                &end(Answer::OutcomeUnknown),
                            );
                        }
                        (Standing::Mismatch, _) => {
                            unreachable!("every call in the model is the same request")
                        }
                    }
                    then(Step::Stops(index), &end(Answer::Lost));
                }
                Call::Reserved => {
                    then(Step::CommandRefuses(index), &|next| {
                        next.calls[index] = Call::Failing;
                    });
                    then(Step::CommandCommits(index), &|next| {
                        next.writes += 1;
                        next.calls[index] = Call::Wrote;
                    });
                    then(Step::Stops(index), &end(Answer::Lost));
                }
                Call::Wrote => {
                    let reservation_stands = world
                        .row
                        .is_some_and(|row| !row.answered && !self.kept(row).lapsed);
                    if reservation_stands {
                        then(Step::KeepsAnswer(index), &|next| {
                            next.row = Some(Row {
                                answered: true,
                                age: Age::Fresh,
                            });
                            next.calls[index] = Call::Ended(Answer::Done);
                        });
                    } else {
                        then(Step::CannotKeepAnswer(index), &end(Answer::Refused));
                    }
                    if self.command == Command::MayFailAfterItsCommit {
                        then(Step::CommandFailsAfterItsCommit(index), &|next| {
                            next.calls[index] = Call::Failing;
                        });
                    }
                    then(Step::Stops(index), &end(Answer::Lost));
                }
                Call::Failing => {
                    then(Step::GivesBack(index), &|next| {
                        if next.row.is_some_and(|row| !row.answered) {
                            next.row = None;
                        }
                        next.calls[index] = Call::Ended(Answer::Refused);
                    });
                    then(Step::Stops(index), &end(Answer::Lost));
                }
                Call::Ended(_) => {}
            }
        }

        if let Some(row) = world.row {
            let later = match row.age {
                Age::Fresh => Some(Age::TenMinutes),
                Age::TenMinutes => Some(Age::HalfAnHour),
                Age::HalfAnHour => self.past_a_day.then_some(Age::ADay),
                Age::ADay => None,
            };
            if let Some(age) = later {
                then(Step::TimePasses(age), &|next| {
                    next.row = Some(Row { age, ..row });
                });
            }
        }
        steps
    }
}

const STATE_LIMIT: usize = 200_000;

fn written_twice(receipts: Receipts) -> Option<Path<Receipts>> {
    explore(&receipts, STATE_LIMIT).breaks(|world| world.writes <= 1)
}

fn told(steps: &[Step]) -> Vec<String> {
    steps.iter().map(ToString::to_string).collect()
}

const TODAY: Receipts = Receipts {
    calls: 3,
    reserved: Reserved::Apart,
    unanswered_life: UnansweredLife::ADay,
    command: Command::FailsOnlyBeforeItsCommit,
    past_a_day: false,
};

#[test]
fn an_id_kept_with_the_write_is_written_once_and_never_seen_unanswered() {
    let receipts = Receipts {
        reserved: Reserved::WithTheWrite,
        ..TODAY
    };
    let explored = explore(&receipts, STATE_LIMIT);
    never(explored.breaks(|world| world.writes <= 1));
    let unanswered_seen = explored.reaches(|world| {
        world
            .calls
            .iter()
            .any(|call| matches!(call, Call::Ended(Answer::Wait | Answer::OutcomeUnknown)))
    });
    never(unanswered_seen);
}

#[test]
fn an_id_kept_apart_is_written_once_within_its_day() {
    never(written_twice(TODAY));
}

#[test]
fn a_caller_told_done_or_replayed_has_its_write() {
    for reserved in [Reserved::WithTheWrite, Reserved::Apart] {
        let explored = explore(&Receipts { reserved, ..TODAY }, STATE_LIMIT);
        let told_done_without_a_write = explored.reaches(|world| {
            world.writes == 0
                && world
                    .calls
                    .iter()
                    .any(|call| matches!(call, Call::Ended(Answer::Done | Answer::Replayed)))
        });
        never(told_done_without_a_write);
    }
}

#[test]
fn a_call_whose_command_refused_can_run_again() {
    for reserved in [Reserved::WithTheWrite, Reserved::Apart] {
        let explored = explore(&Receipts { reserved, ..TODAY }, STATE_LIMIT);
        let retried = explored.reaches(|world| {
            world.calls[0] == Call::Ended(Answer::Refused)
                && world.calls[1] == Call::Ended(Answer::Done)
        });
        assert!(retried.is_some(), "{reserved:?}: a refusal held its id");
    }
}

/// The bug #358 fixed: `reserve_briefly` let an unanswered reservation
/// lapse after ten minutes, so a call whose server stopped after its
/// command committed was run again by its retry.
#[test]
fn the_ten_minute_lapse_ran_a_lost_call_twice() {
    let before_358 = Receipts {
        unanswered_life: UnansweredLife::TenMinutes,
        ..TODAY
    };
    let path = written_twice(before_358).expect("the ten-minute lapse writes twice");
    assert_eq!(
        told(&path.steps),
        [
            "call 1 arrives",
            "call 1 reserves the id and commits",
            "call 1's command commits its write",
            "call 2 arrives",
            "time passes: TenMinutes",
            "call 2 reserves the id and commits",
            "call 2's command commits its write",
        ]
    );
}

/// A finding, not a fixed bug: the dispatcher gives a reservation back on
/// any error (`dispatch`, "A refused or failed command made nothing"), and
/// nothing checks that claim. A command kept apart that commits and then
/// returns an error is run again by a retry. When the dispatcher or the
/// commands close this, this test turns into the at-most-once one above.
#[test]
fn a_command_kept_apart_that_fails_after_its_commit_is_run_again() {
    let receipts = Receipts {
        command: Command::MayFailAfterItsCommit,
        ..TODAY
    };
    let path = written_twice(receipts).expect("an error after a commit frees the id");
    assert_eq!(
        told(&path.steps),
        [
            "call 1 arrives",
            "call 1 reserves the id and commits",
            "call 1's command commits its write",
            "call 1's command fails after its commit",
            "call 1 gives its reservation back",
            "call 2 arrives",
            "call 2 reserves the id and commits",
            "call 2's command commits its write",
        ]
    );
}

/// The promise holds for the day an id is kept (the v2 contract's window),
/// not after it.
#[test]
fn past_its_day_a_lost_call_can_run_again() {
    let receipts = Receipts {
        past_a_day: true,
        ..TODAY
    };
    let lost_then_run_again = explore(&receipts, STATE_LIMIT)
        .reaches(|world| world.calls[0] == Call::Ended(Answer::Lost) && world.writes == 2);
    assert!(lost_then_run_again.is_some());
}

/// A retry of a call that never answered waits only while that call could
/// still be running: after half an hour it is told the outcome isn't known.
#[test]
fn a_lost_calls_retry_stops_waiting() {
    let explored = explore(&TODAY, STATE_LIMIT);
    let told_unknown = explored.reaches(|world| {
        world.calls[0] == Call::Ended(Answer::Lost)
            && world.calls[1] == Call::Ended(Answer::OutcomeUnknown)
    });
    assert!(told_unknown.is_some(), "a lost call's retries wait all day");
}
