//! Every order of steps for one job and two workers, checked against the
//! promise a job's lease makes: a handler never runs at or after the end of
//! its lease, so two workers never run the same job at once.
//!
//! Time moves in sixths of the lease, the smallest span the code uses: a
//! worker waits `renewal_wait` (a third) between renewals and gives one up
//! after `renewal_patience` (a sixth). A renewal takes anything up to that
//! long. Its statement runs at any moment in between, extending the lease
//! from then or finding it gone, or never reaches the database; its answer
//! arrives or is lost on the way back. (The statement is one step: it dates
//! the lease from when its transaction began and holds the row until it
//! commits, and a claim skips a held row, so nobody acts on the row in
//! between and a lease dated earlier is one the model tries anyway.) The
//! decision to keep running after a
//! failed renewal is the server's own `lease_still_held`. The steps around
//! it are `claim`, `confirm_lease`, `renew_lease` and `extend_lease`.
//!
//! Assumed, as the code assumes: a worker's clock and the database's run at
//! the same rate, a sleeping worker wakes on time, and dropping a handler
//! stops it at once. Not in the model: what a handler does, a slow
//! `confirm_lease`, and `finish` (which only counts while the lease token is
//! still the job's).
//! `.claude/skills/job-leases/SKILL.md` says how to read a failure.

use std::{fmt, time::Duration};

use super::{lease_still_held, renewal_patience, renewal_wait};
use crate::model_check::{Model, Path, explore, never};

const LEASE: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_secs(10);
const LEASE_TICKS: u8 = 6;
const _: () = assert!(TICK.as_secs() * LEASE_TICKS as u64 == LEASE.as_secs());

fn ticks(span: Duration) -> u8 {
    assert!(
        span.as_nanos().is_multiple_of(TICK.as_nanos()),
        "{span:?} isn't a whole number of ticks: make TICK divide every timing the code uses"
    );
    u8::try_from(span.as_secs() / TICK.as_secs()).expect("a span of a few ticks")
}

fn span(ticks: u8) -> Duration {
    TICK * u32::from(ticks)
}

#[derive(Clone, Copy, Debug)]
struct Leases {
    /// A claimed job's lease is confirmed before its handler starts
    /// (`confirm_lease`). Before 2c013368 it started on the claim alone.
    confirms_before_starting: bool,
    /// A worker that can't renew stops before the lease may have ended
    /// (`lease_still_held`). Before aace9b02 it kept trying.
    stops_when_unsure: bool,
    until: u8,
}

/// The job's row, as the database has it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Row {
    token: u8,
    expires_at: u8,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Worker {
    Idle,
    /// The claim committed; the worker has not acted on it yet.
    Claimed {
        token: u8,
    },
    Running {
        token: u8,
        renewed_at: u8,
        wakes_at: u8,
    },
    /// A renewal sent at `sent_at` is answered at `answered_at`. Its
    /// statement runs at some moment in between (`extended`: whether it
    /// found the lease still the worker's), or never.
    Renewing {
        token: u8,
        renewed_at: u8,
        sent_at: u8,
        answered_at: u8,
        extended: Option<bool>,
    },
}

impl Worker {
    fn handler_token(self) -> Option<u8> {
        match self {
            Self::Running { token, .. } | Self::Renewing { token, .. } => Some(token),
            Self::Idle | Self::Claimed { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct World {
    now: u8,
    row: Option<Row>,
    workers: [Worker; 2],
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Step {
    Claims(usize),
    Starts(usize),
    FindsItsLeaseGone(usize),
    SendsRenewal(usize, u8),
    RenewalRuns(usize, bool),
    Renews(usize),
    RenewalRefused(usize),
    RenewalFails {
        worker: usize,
        /// What its statement did, if it ran.
        extended: Option<bool>,
        stops: bool,
    },
    TimePasses(u8),
}

impl fmt::Display for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let worker = |index: &usize| index + 1;
        match self {
            Self::Claims(index) => write!(formatter, "worker {} claims the job", worker(index)),
            Self::Starts(index) => write!(formatter, "worker {} starts the handler", worker(index)),
            Self::FindsItsLeaseGone(index) => write!(
                formatter,
                "worker {} finds its lease gone and never starts",
                worker(index)
            ),
            Self::SendsRenewal(index, takes) => write!(
                formatter,
                "worker {} sends a renewal (answered {takes} sixths later)",
                worker(index)
            ),
            Self::RenewalRuns(index, extended) => write!(
                formatter,
                "worker {}'s renewal reaches the database: {}",
                worker(index),
                if *extended {
                    "the lease runs a whole lease from now"
                } else {
                    "the lease is no longer its own"
                }
            ),
            Self::Renews(index) => write!(
                formatter,
                "worker {} hears its lease was renewed",
                worker(index)
            ),
            Self::RenewalRefused(index) => write!(
                formatter,
                "worker {} is told the lease is no longer its own and stops the handler",
                worker(index)
            ),
            Self::RenewalFails {
                worker: index,
                extended,
                stops,
            } => write!(
                formatter,
                "worker {}'s renewal {}: it {}",
                worker(index),
                match extended {
                    Some(true) => "extended the lease but its answer is lost",
                    Some(false) => "was refused but its answer is lost",
                    None => "can't reach the database",
                },
                if *stops {
                    "can't be sure of its lease and stops the handler"
                } else {
                    "keeps the handler running and will try again"
                }
            ),
            Self::TimePasses(now) => write!(formatter, "time passes: {now} sixths of a lease"),
        }
    }
}

impl Leases {
    fn held(row: Option<Row>, token: u8, now: u8) -> bool {
        row.is_some_and(|row| row.token == token && row.expires_at > now)
    }

    fn renewed(token: u8, now: u8) -> Option<Row> {
        Some(Row {
            token,
            expires_at: now + LEASE_TICKS,
        })
    }

    /// The earliest moment something in a worker's own loop is due: time
    /// can't pass it (a sleeping worker wakes on time).
    fn next_due(world: &World) -> Option<u8> {
        world
            .workers
            .iter()
            .filter_map(|worker| match worker {
                Worker::Running { wakes_at, .. } => Some(*wakes_at),
                Worker::Renewing { answered_at, .. } => Some(*answered_at),
                Worker::Idle | Worker::Claimed { .. } => None,
            })
            .min()
    }
}

impl Model for Leases {
    type State = World;
    type Step = Step;

    fn start(&self) -> World {
        World {
            now: 0,
            row: None,
            workers: [Worker::Idle; 2],
        }
    }

    fn steps(&self, world: &World) -> Vec<(Step, World)> {
        let mut steps = Vec::new();
        let mut then = |step: Step, change: &dyn Fn(&mut World)| {
            let mut next = world.clone();
            change(&mut next);
            steps.push((step, next));
        };
        let now = world.now;
        let wait = ticks(renewal_wait(LEASE));
        let patience = ticks(renewal_patience(LEASE));

        for (index, worker) in world.workers.iter().enumerate() {
            match *worker {
                Worker::Idle => {
                    let claimable = world.row.is_none_or(|row| row.expires_at < now);
                    if claimable {
                        let token = world.row.map_or(1, |row| row.token + 1);
                        then(Step::Claims(index), &|next| {
                            next.row = Self::renewed(token, now);
                            next.workers[index] = Worker::Claimed { token };
                        });
                    }
                }
                Worker::Claimed { token } => {
                    let running = Worker::Running {
                        token,
                        renewed_at: now,
                        wakes_at: now + wait,
                    };
                    if !self.confirms_before_starting {
                        then(Step::Starts(index), &|next| next.workers[index] = running);
                    } else if Self::held(world.row, token, now) {
                        then(Step::Starts(index), &|next| {
                            next.row = Self::renewed(token, now);
                            next.workers[index] = running;
                        });
                    } else {
                        then(Step::FindsItsLeaseGone(index), &|next| {
                            next.workers[index] = Worker::Idle;
                        });
                    }
                }
                Worker::Running {
                    token,
                    renewed_at,
                    wakes_at,
                } if wakes_at == now => {
                    for takes in 0..=patience {
                        then(Step::SendsRenewal(index, takes), &|next| {
                            next.workers[index] = Worker::Renewing {
                                token,
                                renewed_at,
                                sent_at: now,
                                answered_at: now + takes,
                                extended: None,
                            };
                        });
                    }
                }
                Worker::Running { .. } => {}
                Worker::Renewing {
                    token,
                    renewed_at,
                    sent_at,
                    answered_at,
                    extended,
                } => {
                    // `extend_lease`'s statement dates the lease from when
                    // it runs, which is any moment before its answer.
                    if extended.is_none() {
                        let held = Self::held(world.row, token, now);
                        then(Step::RenewalRuns(index, held), &|next| {
                            if held {
                                next.row = Self::renewed(token, now);
                            }
                            next.workers[index] = Worker::Renewing {
                                token,
                                renewed_at,
                                sent_at,
                                answered_at,
                                extended: Some(held),
                            };
                        });
                    }
                    if answered_at == now {
                        match extended {
                            Some(true) => then(Step::Renews(index), &|next| {
                                next.workers[index] = Worker::Running {
                                    token,
                                    renewed_at: sent_at,
                                    wakes_at: now + wait,
                                };
                            }),
                            Some(false) => then(Step::RenewalRefused(index), &|next| {
                                next.workers[index] = Worker::Idle;
                            }),
                            None => {}
                        }
                        let sure = lease_still_held(span(now - renewed_at), LEASE);
                        let stops = self.stops_when_unsure && !sure;
                        then(
                            Step::RenewalFails {
                                worker: index,
                                extended,
                                stops,
                            },
                            &|next| {
                                next.workers[index] = if stops {
                                    Worker::Idle
                                } else {
                                    Worker::Running {
                                        token,
                                        renewed_at,
                                        wakes_at: now + wait,
                                    }
                                };
                            },
                        );
                    }
                }
            }
        }

        let nothing_due = Self::next_due(world).is_none_or(|due| due > now);
        if nothing_due && now < self.until {
            then(Step::TimePasses(now + 1), &|next| next.now = now + 1);
        }
        steps
    }
}

const STATE_LIMIT: usize = 500_000;

const TODAY: Leases = Leases {
    confirms_before_starting: true,
    stops_when_unsure: true,
    until: 16,
};

/// A handler running at or after the end of its lease, or under a lease
/// that is another worker's by now.
fn runs_without_its_lease(world: &World) -> bool {
    world.workers.iter().any(|worker| {
        worker
            .handler_token()
            .is_some_and(|token| !Leases::held(world.row, token, world.now))
    })
}

fn two_handlers(world: &World) -> bool {
    world
        .workers
        .iter()
        .all(|worker| worker.handler_token().is_some())
}

fn told(path: &Path<Leases>) -> Vec<String> {
    path.steps.iter().map(ToString::to_string).collect()
}

#[test]
fn a_handler_never_runs_at_or_after_the_end_of_its_lease() {
    let explored = explore(&TODAY, STATE_LIMIT);
    never(explored.reaches(runs_without_its_lease));
    never(explored.reaches(two_handlers));
}

/// The same steps, from a state already reached.
struct After(Leases, World);

impl Model for After {
    type State = World;
    type Step = Step;

    fn start(&self) -> World {
        self.1.clone()
    }

    fn steps(&self, world: &World) -> Vec<(Step, World)> {
        self.0.steps(world)
    }
}

/// A handler that started and then lost its lease doesn't hold the job for
/// good: another worker runs it.
#[test]
fn a_job_whose_handler_stopped_is_run_again() {
    let started = explore(&TODAY, STATE_LIMIT)
        .reaches(|world| world.workers[0].handler_token() == Some(1))
        .expect("a handler starts");
    let taken_over = explore(&After(TODAY, started.end), STATE_LIMIT)
        .reaches(|world| world.workers[1].handler_token() == Some(2));
    assert!(taken_over.is_some(), "a lost lease was never taken over");
}

/// A failed renewal doesn't always cost the job its run: when it and the
/// renewal before it were quick, the worker tries again while its lease is
/// sure to hold. (When the two took a sixth of the lease or more between
/// them, the handler stops: a second try couldn't be sure to end in time.)
#[test]
fn a_blip_does_not_stop_the_handler() {
    let wait = ticks(renewal_wait(LEASE));
    let patience = ticks(renewal_patience(LEASE));
    let survived = explore(&TODAY, STATE_LIMIT).reaches(|world| {
        matches!(
            world.workers[0],
            Worker::Running { renewed_at, wakes_at, .. } if wakes_at - renewed_at > wait + patience
        )
    });
    assert!(
        survived.is_some(),
        "a worker stopped at its first failed renewal"
    );
}

/// The bug 2c013368 fixed: a claim that reached its worker late started its
/// handler on the claim alone, as if the lease had just begun. The lease ran
/// out under it and another worker took the job beside it.
#[test]
fn a_late_claim_started_a_second_handler() {
    let before = Leases {
        confirms_before_starting: false,
        ..TODAY
    };
    let path = explore(&before, STATE_LIMIT)
        .reaches(two_handlers)
        .expect("a late claim starts a second handler");
    assert_eq!(
        told(&path),
        [
            "worker 1 claims the job",
            "time passes: 1 sixths of a lease",
            "time passes: 2 sixths of a lease",
            "time passes: 3 sixths of a lease",
            "time passes: 4 sixths of a lease",
            "time passes: 5 sixths of a lease",
            "worker 1 starts the handler",
            "time passes: 6 sixths of a lease",
            "time passes: 7 sixths of a lease",
            "worker 2 claims the job",
            "worker 2 starts the handler",
        ]
    );
}

/// The bug aace9b02 fixed: a worker whose renewals kept failing went on
/// running while its lease ran out and another worker took the job.
#[test]
fn a_worker_that_kept_trying_ran_beside_its_successor() {
    let before = Leases {
        stops_when_unsure: false,
        ..TODAY
    };
    assert!(
        explore(&before, STATE_LIMIT)
            .reaches(two_handlers)
            .is_some()
    );
}
