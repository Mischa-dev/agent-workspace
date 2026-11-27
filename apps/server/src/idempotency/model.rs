//! Every order of steps for one request id, checked against the promise a
//! request id makes: however a call is retried, its write is made once.
//!
//! The decision at each arrival is the server's own `standing`. The steps
//! around it are the dispatcher's, in `ops/dispatch.rs::dispatch`: with the
//! write (`call.tx()`, where the id, the write and the answer commit
//! together) or apart from it (`OWN_TRANSACTION`: `reserve_apart` commits,
//! the command runs in its own transactions, then `complete` keeps its
//! answer, `release` gives the id back after a refusal, or `complete` keeps
//! "outcome unknown" after a failure). Each step here is one transaction
//! there; the advisory lock in `reserve` is what makes an arrival one step.
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

/// What the dispatcher does with the id of a command that failed, as
/// opposed to refused (`OpError::is_refusal`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AfterAFailure {
    /// Keeps "outcome unknown" under it: the command may have committed.
    KeepsTheId,
    /// As it was: gave it back, as after a refusal.
    GivesTheIdBack,
}

/// When a command may refuse. The dispatcher frees a refused call's id on
/// the commands' word that a refusal comes before any write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Refusals {
    OnlyBeforeACommit,
    AlsoAfterACommit,
}

#[derive(Clone, Copy, Debug)]
struct Receipts {
    calls: usize,
    reserved: Reserved,
    unanswered_life: UnansweredLife,
    after_a_failure: AfterAFailure,
    refusals: Refusals,
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

/// The answer kept under an id.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Stored {
    Done,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Row {
    answer: Option<Stored>,
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
    Refusing,
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
    Failed,
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
    CommandFails(usize),
    CommandCommits(usize),
    CommandRefusesAfterItsCommit(usize),
    CommandFailsAfterItsCommit(usize),
    GivesBack(usize),
    KeepsOutcomeUnknown(usize),
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
            Self::CommandFails(index) => write!(
                formatter,
                "call {}'s command fails, having made nothing",
                call(index)
            ),
            Self::CommandCommits(index) => {
                write!(
                    formatter,
                    "call {}'s command commits its write",
                    call(index)
                )
            }
            Self::CommandRefusesAfterItsCommit(index) => write!(
                formatter,
                "call {}'s command refuses after its commit",
                call(index)
            ),
            Self::CommandFailsAfterItsCommit(index) => write!(
                formatter,
                "call {}'s command fails after its commit",
                call(index)
            ),
            Self::GivesBack(index) => {
                write!(formatter, "call {} gives its reservation back", call(index))
            }
            Self::KeepsOutcomeUnknown(index) => write!(
                formatter,
                "call {} keeps \"outcome unknown\" under the id",
                call(index)
            ),
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
    fn kept(&self, row: Row) -> Kept<Stored> {
        let lapses_at = match self.unanswered_life {
            UnansweredLife::TenMinutes if row.answer.is_none() => Age::TenMinutes,
            UnansweredLife::TenMinutes | UnansweredLife::ADay => Age::ADay,
        };
        Kept {
            same_request: true,
            answer: row.answer,
            stalled: row.age >= Age::HalfAnHour,
            lapsed: row.age >= lapses_at,
        }
    }

    fn standing(&self, world: &World) -> Standing<Stored> {
        standing(world.row.map(|row| self.kept(row)))
    }

    /// Whether `complete` finds this call's reservation as it left it.
    fn reservation_stands(&self, world: &World) -> bool {
        world
            .row
            .is_some_and(|row| row.answer.is_none() && !self.kept(row).lapsed)
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
            answer: None,
            age: Age::Fresh,
        };
        let answered = |stored| {
            Some(Row {
                answer: Some(stored),
                age: Age::Fresh,
            })
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
                                next.row = answered(Stored::Done);
                                next.writes += 1;
                                next.calls[index] = Call::Ended(Answer::Done);
                            });
                            then(Step::RollsBack(index), &end(Answer::Refused));
                        }
                        (Standing::Answered(Stored::Done), _) => {
                            then(Step::Replays(index), &end(Answer::Replayed));
                        }
                        (Standing::Running, _) => {
                            then(Step::ToldToWait(index), &end(Answer::Wait));
                        }
                        (
                            Standing::OutcomeUnknown | Standing::Answered(Stored::OutcomeUnknown),
                            _,
                        ) => {
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
                        next.calls[index] = Call::Refusing;
                    });
                    then(Step::CommandFails(index), &|next| {
                        next.calls[index] = Call::Failing;
                    });
                    then(Step::CommandCommits(index), &|next| {
                        next.writes += 1;
                        next.calls[index] = Call::Wrote;
                    });
                    then(Step::Stops(index), &end(Answer::Lost));
                }
                Call::Wrote => {
                    if self.reservation_stands(world) {
                        then(Step::KeepsAnswer(index), &|next| {
                            next.row = answered(Stored::Done);
                            next.calls[index] = Call::Ended(Answer::Done);
                        });
                    } else {
                        then(Step::CannotKeepAnswer(index), &end(Answer::Failed));
                    }
                    if self.refusals == Refusals::AlsoAfterACommit {
                        then(Step::CommandRefusesAfterItsCommit(index), &|next| {
                            next.calls[index] = Call::Refusing;
                        });
                    }
                    then(Step::CommandFailsAfterItsCommit(index), &|next| {
                        next.calls[index] = Call::Failing;
                    });
                    then(Step::Stops(index), &end(Answer::Lost));
                }
                Call::Refusing | Call::Failing => {
                    let failed = *call == Call::Failing;
                    let answer = if failed {
                        Answer::Failed
                    } else {
                        Answer::Refused
                    };
                    if failed && self.after_a_failure == AfterAFailure::KeepsTheId {
                        let stands = self.reservation_stands(world);
                        then(Step::KeepsOutcomeUnknown(index), &|next| {
                            if stands {
                                next.row = answered(Stored::OutcomeUnknown);
                            }
                            next.calls[index] = Call::Ended(answer);
                        });
                    } else {
                        then(Step::GivesBack(index), &|next| {
                            if next.row.is_some_and(|row| row.answer.is_none()) {
                                next.row = None;
                            }
                            next.calls[index] = Call::Ended(answer);
                        });
                    }
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
    after_a_failure: AfterAFailure::KeepsTheId,
    refusals: Refusals::OnlyBeforeACommit,
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

/// The bug the model found: the dispatcher gave a reservation back on any
/// error, on the claim that a failed command made nothing. A command that
/// committed and then failed (a read-back that couldn't reach the database)
/// was run again by its retry.
#[test]
fn giving_back_a_failed_calls_id_ran_it_twice() {
    let as_it_was = Receipts {
        after_a_failure: AfterAFailure::GivesTheIdBack,
        ..TODAY
    };
    let path = written_twice(as_it_was).expect("an error after a commit frees the id");
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

/// The retry of a call whose command failed is told at once that the
/// outcome isn't known, whether or not the command had committed: however
/// the command failed, an answer is kept under its id before the caller
/// hears of it, so no later call finds the id unanswered and waits.
#[test]
fn a_failed_calls_retry_is_told_the_outcome_is_unknown() {
    let explored = explore(&TODAY, STATE_LIMIT);
    let failed = |world: &World| world.calls.contains(&Call::Ended(Answer::Failed));
    let left_unanswered = explored
        .reaches(|world| failed(world) && !world.row.is_some_and(|row| row.answer.is_some()));
    never(left_unanswered);
    let retry_told = explored.reaches(|world| {
        world.calls[0] == Call::Ended(Answer::Failed)
            && world.calls[1] == Call::Ended(Answer::OutcomeUnknown)
            && world.row.is_some_and(|row| row.age == Age::Fresh)
    });
    assert!(retry_told.is_some(), "a failed call's retry waited");
    let retry_ran = explored.reaches(|world| {
        world.calls[0] == Call::Ended(Answer::Failed)
            && matches!(world.calls[1], Call::Reserved | Call::Wrote)
    });
    never(retry_ran);
}

/// By contract, not checked: a command refuses before it writes. One that
/// commits and then refuses (a read-back that finds the thing gone) has its
/// id freed, and its retry runs it again.
#[test]
fn a_command_that_refuses_after_its_commit_is_run_again() {
    let receipts = Receipts {
        refusals: Refusals::AlsoAfterACommit,
        ..TODAY
    };
    assert!(written_twice(receipts).is_some());
}

/// The promise holds for the day an id is kept (the v2 contract's window),
/// not after it.
#[test]
fn past_its_day_a_lost_call_can_run_again() {
    // Two calls, so both wrote and the first was lost after its write.
    let receipts = Receipts {
        calls: 2,
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
