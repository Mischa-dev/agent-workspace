//! A1: where a run is, whoever runs it (A1 3.2).
//!
//! States describe the work; [`Liveness`] describes the reporter. Every write
//! to a run goes through [`reduce`], one deterministic function, so a run's
//! stored state is always what its activities and requests say it is.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::domain::RunState as LegacyRunState;

/// Where a run is.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Recorded, not yet sent to anyone.
    Queued,
    /// A start was sent; no answer yet.
    Starting,
    /// The host may or may not have started it.
    StartUncertain,
    /// The host took it; nothing reported yet.
    Accepted,
    /// Reports are arriving.
    Working,
    /// At least one request raised by the run is open.
    Waiting,
    /// Someone asked it to stop; not yet confirmed.
    CancelRequested,
    Done,
    Failed,
    Cancelled,
    /// The host said no.
    Refused,
    /// Ended without a known outcome.
    Interrupted,
}

impl RunState {
    pub const ALL: [Self; 12] = [
        Self::Queued,
        Self::Starting,
        Self::StartUncertain,
        Self::Accepted,
        Self::Working,
        Self::Waiting,
        Self::CancelRequested,
        Self::Done,
        Self::Failed,
        Self::Cancelled,
        Self::Refused,
        Self::Interrupted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::StartUncertain => "start_uncertain",
            Self::Accepted => "accepted",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::CancelRequested => "cancel_requested",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Refused => "refused",
            Self::Interrupted => "interrupted",
        }
    }

    /// Finished, one way or another.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Refused | Self::Interrupted
        )
    }

    /// Not finished: the one rule for "active" (guard G4).
    pub const fn is_active(self) -> bool {
        !self.is_terminal()
    }

    /// The states that are not finished, as stored, for SQL `= ANY($n)`.
    pub fn active_keys() -> Vec<&'static str> {
        Self::ALL
            .into_iter()
            .filter(|state| state.is_active())
            .map(Self::as_str)
            .collect()
    }

    /// The finished states, as stored.
    pub fn terminal_keys() -> Vec<&'static str> {
        Self::ALL
            .into_iter()
            .filter(|state| state.is_terminal())
            .map(Self::as_str)
            .collect()
    }

    /// Whether a run that reports for itself is expected to report: liveness
    /// applies only while the work is out with someone.
    pub const fn expects_reports(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Working | Self::Waiting | Self::CancelRequested
        )
    }

    /// The explicit edges of A1 3.2's table: what `run_update`, a host's
    /// acknowledgement, a cancel or recovery may move a run along.
    pub fn can_move_to(self, next: Self) -> bool {
        use RunState::{
            Accepted, CancelRequested, Cancelled, Done, Failed, Interrupted, Queued, Refused,
            StartUncertain, Starting, Waiting, Working,
        };
        if self == next {
            return false;
        }
        match self {
            Queued => matches!(next, Starting | Cancelled | Failed),
            Starting => matches!(
                next,
                Accepted | Working | Refused | StartUncertain | CancelRequested | Interrupted
            ),
            StartUncertain => matches!(
                next,
                Accepted | Working | Failed | CancelRequested | Interrupted
            ),
            Accepted => matches!(
                next,
                Working | Waiting | Done | Failed | CancelRequested | Interrupted
            ),
            Working => matches!(
                next,
                Waiting | Done | Failed | CancelRequested | Interrupted
            ),
            Waiting => matches!(
                next,
                Working | CancelRequested | Failed | Done | Interrupted
            ),
            CancelRequested => matches!(next, Cancelled | Done | Failed | Interrupted),
            Done | Failed | Cancelled | Refused | Interrupted => false,
        }
    }

    /// Today's built-in run state as a run state. `interrupted` is
    /// `cancelled` when the person who asked for the run pressed Stop, which
    /// `api::interrupt_run_in` records as the reason. Kept equal to the SQL
    /// function `runs_state_from_legacy` (tested).
    pub fn from_legacy(state: LegacyRunState, failure_reason: Option<&str>) -> Self {
        match state {
            LegacyRunState::Created | LegacyRunState::Queued => Self::Queued,
            LegacyRunState::Starting => Self::Starting,
            LegacyRunState::Working | LegacyRunState::Recovering => Self::Working,
            LegacyRunState::Blocked => Self::Waiting,
            LegacyRunState::Completed => Self::Done,
            LegacyRunState::Failed => Self::Failed,
            LegacyRunState::Canceled => Self::Cancelled,
            LegacyRunState::Interrupted if failure_reason == Some(STOPPED_BY_REQUESTER) => {
                Self::Cancelled
            }
            LegacyRunState::Interrupted => Self::Interrupted,
        }
    }
}

/// The reason `api::interrupt_run_in` gives a built-in run a person stopped.
pub const STOPPED_BY_REQUESTER: &str = "Interrupted by the requesting Actor.";

impl fmt::Display for RunState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownRunState;

impl FromStr for RunState {
    type Err = UnknownRunState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or(UnknownRunState)
    }
}

/// Whether the reporter is keeping up. Separate from the state, so a run can
/// be "working but silent" without losing what it was doing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    Ok,
    /// No report for one interval.
    Quiet,
    /// No report for three intervals.
    NotResponding,
}

impl Liveness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Quiet => "quiet",
            Self::NotResponding => "not_responding",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(Self::Ok),
            "quiet" => Some(Self::Quiet),
            "not_responding" => Some(Self::NotResponding),
            _ => None,
        }
    }

    /// What a run is after `silent` seconds without a report, when it was
    /// asked to report every `interval` seconds.
    pub fn after_silence(silent: i64, interval: i64) -> Self {
        let interval = interval.max(1);
        if silent >= interval * 3 {
            Self::NotResponding
        } else if silent >= interval {
            Self::Quiet
        } else {
            Self::Ok
        }
    }
}

/// How a run finished, as a `run_finished` activity says.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Cancelled,
}

impl Outcome {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "cancelled" | "canceled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub const fn state(self) -> RunState {
        match self {
            Self::Done => RunState::Done,
            Self::Failed => RunState::Failed,
            Self::Cancelled => RunState::Cancelled,
        }
    }
}

/// What happened to a run, for [`reduce`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunEvent {
    /// An activity was stored. `finished` carries a `run_finished`
    /// outcome; `lifecycle` is `run_started`.
    Activity(ActivityEffect),
    /// A request raised by the run opened.
    RequestOpened,
    /// A request raised by the run closed; `still_open` are left.
    RequestClosed { still_open: i64 },
    /// Someone asked it to stop. `has_runner`: a host or the agent must
    /// confirm; without one (still queued) it is cancelled at once.
    CancelRequested { has_runner: bool },
    /// An explicit move (`run_update`, a host's acknowledgement, recovery).
    Update(RunState),
    /// The host was lost or unlinked, or the agent lost access.
    HostLost,
}

/// What an activity does to the state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityEffect {
    /// `run_started`, or any other report: the run is being worked on.
    Report,
    /// `run_finished` with its outcome.
    Finished(Outcome),
    /// `interrupt`: the run raised a request and waits on it.
    Interrupt,
    /// `interrupt_resolved`: one of its requests was answered.
    Resolved { still_open: i64 },
}

/// A move the edges don't allow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefusedMove {
    pub from: RunState,
    pub to: RunState,
}

/// The one reducer every write to a run goes through (A1 3.2). Reports never
/// fail: a report on a finished run is kept and changes nothing. Explicit
/// moves follow [`RunState::can_move_to`] and refuse anything else.
pub fn reduce(state: RunState, event: RunEvent) -> Result<RunState, RefusedMove> {
    use RunState::{
        Accepted, CancelRequested, Cancelled, Interrupted, Queued, StartUncertain, Starting,
        Waiting, Working,
    };
    Ok(match event {
        RunEvent::Activity(_) | RunEvent::RequestOpened | RunEvent::RequestClosed { .. }
            if state.is_terminal() =>
        {
            state
        }
        RunEvent::Activity(ActivityEffect::Finished(outcome)) => outcome.state(),
        RunEvent::Activity(ActivityEffect::Report) => match state {
            Queued | Starting | StartUncertain | Accepted => Working,
            other => other,
        },
        RunEvent::Activity(ActivityEffect::Interrupt) | RunEvent::RequestOpened => match state {
            CancelRequested => CancelRequested,
            _ => Waiting,
        },
        RunEvent::Activity(ActivityEffect::Resolved { still_open })
        | RunEvent::RequestClosed { still_open } => match state {
            Waiting if still_open <= 0 => Working,
            other => other,
        },
        RunEvent::CancelRequested { .. } if state.is_terminal() || state == CancelRequested => {
            state
        }
        // Still queued, or nobody runs it who could confirm: stopped at once.
        RunEvent::CancelRequested { has_runner } if state == Queued || !has_runner => Cancelled,
        RunEvent::CancelRequested { .. } => CancelRequested,
        RunEvent::HostLost if state.is_terminal() => state,
        RunEvent::HostLost => Interrupted,
        RunEvent::Update(next) if next == state => state,
        RunEvent::Update(next) => {
            if state.can_move_to(next) {
                next
            } else {
                return Err(RefusedMove {
                    from: state,
                    to: next,
                });
            }
        }
    })
}

/// The state a stored log of activities leads to from `start`, replaying
/// [`reduce`]: what guard G7 checks the stored state against.
pub fn replay(start: RunState, effects: impl IntoIterator<Item = ActivityEffect>) -> RunState {
    effects.into_iter().fold(start, |state, effect| {
        reduce(state, RunEvent::Activity(effect)).unwrap_or(state)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ActivityEffect, Liveness, Outcome, RunEvent, RunState, STOPPED_BY_REQUESTER, reduce, replay,
    };
    use crate::domain::RunState as Legacy;

    #[test]
    fn states_round_trip_and_split_into_active_and_terminal() {
        for state in RunState::ALL {
            assert_eq!(state.as_str().parse::<RunState>(), Ok(state));
            assert_eq!(state.is_active(), !state.is_terminal());
        }
        assert_eq!(
            RunState::terminal_keys(),
            ["done", "failed", "cancelled", "refused", "interrupted"]
        );
        assert_eq!(RunState::active_keys().len(), 7);
    }

    /// Every edge of A1 3.2's table, and nothing else.
    #[test]
    fn explicit_moves_follow_the_table() {
        use RunState::*;
        let allowed = [
            (Queued, Starting),
            (Queued, Cancelled),
            (Queued, Failed),
            (Starting, Accepted),
            (Starting, Working),
            (Starting, Refused),
            (Starting, StartUncertain),
            (Starting, CancelRequested),
            (Starting, Interrupted),
            (StartUncertain, Accepted),
            (StartUncertain, Working),
            (StartUncertain, Failed),
            (StartUncertain, CancelRequested),
            (StartUncertain, Interrupted),
            (Accepted, Working),
            (Accepted, Waiting),
            (Accepted, Done),
            (Accepted, Failed),
            (Accepted, CancelRequested),
            (Accepted, Interrupted),
            (Working, Waiting),
            (Working, Done),
            (Working, Failed),
            (Working, CancelRequested),
            (Working, Interrupted),
            (Waiting, Working),
            (Waiting, CancelRequested),
            (Waiting, Failed),
            (Waiting, Done),
            (Waiting, Interrupted),
            (CancelRequested, Cancelled),
            (CancelRequested, Done),
            (CancelRequested, Failed),
            (CancelRequested, Interrupted),
        ];
        for from in RunState::ALL {
            for to in RunState::ALL {
                let expected = allowed.contains(&(from, to));
                assert_eq!(from.can_move_to(to), expected, "{from} -> {to}");
                let reduced = reduce(from, RunEvent::Update(to));
                if from == to {
                    assert_eq!(reduced, Ok(from));
                } else if expected {
                    assert_eq!(reduced, Ok(to), "{from} -> {to}");
                } else {
                    assert!(reduced.is_err(), "{from} -> {to} should be refused");
                }
            }
        }
    }

    #[test]
    fn activities_drive_the_state() {
        use RunState::*;
        let report = RunEvent::Activity(ActivityEffect::Report);
        for start in [Queued, Starting, StartUncertain, Accepted] {
            assert_eq!(reduce(start, report), Ok(Working));
        }
        assert_eq!(reduce(Waiting, report), Ok(Waiting));
        assert_eq!(reduce(CancelRequested, report), Ok(CancelRequested));
        let interrupt = RunEvent::Activity(ActivityEffect::Interrupt);
        assert_eq!(reduce(Working, interrupt), Ok(Waiting));
        assert_eq!(reduce(CancelRequested, interrupt), Ok(CancelRequested));
        let resolved = |open| RunEvent::Activity(ActivityEffect::Resolved { still_open: open });
        assert_eq!(reduce(Waiting, resolved(1)), Ok(Waiting));
        assert_eq!(reduce(Waiting, resolved(0)), Ok(Working));
        for outcome in [Outcome::Done, Outcome::Failed, Outcome::Cancelled] {
            let finished = RunEvent::Activity(ActivityEffect::Finished(outcome));
            assert_eq!(reduce(Working, finished), Ok(outcome.state()));
            assert_eq!(reduce(CancelRequested, finished), Ok(outcome.state()));
            // A late report on a finished run is kept and changes nothing.
            assert_eq!(reduce(Done, finished), Ok(Done));
        }
        assert_eq!(reduce(Done, report), Ok(Done));
    }

    #[test]
    fn cancel_waits_for_a_runner_and_host_loss_interrupts() {
        use RunState::*;
        assert_eq!(
            reduce(Queued, RunEvent::CancelRequested { has_runner: true }),
            Ok(Cancelled)
        );
        assert_eq!(
            reduce(Working, RunEvent::CancelRequested { has_runner: true }),
            Ok(CancelRequested)
        );
        assert_eq!(
            reduce(Working, RunEvent::CancelRequested { has_runner: false }),
            Ok(Cancelled)
        );
        assert_eq!(
            reduce(Done, RunEvent::CancelRequested { has_runner: true }),
            Ok(Done)
        );
        assert_eq!(reduce(Waiting, RunEvent::HostLost), Ok(Interrupted));
        assert_eq!(reduce(Failed, RunEvent::HostLost), Ok(Failed));
    }

    /// G7, in memory: a random sequence of activities applied one at a time
    /// reaches the same state as replaying the whole log.
    #[test]
    fn state_follows_activities_for_random_sequences() {
        let mut seed: u64 = 0x5eed_a1a1;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..500 {
            let start =
                [RunState::Queued, RunState::Accepted, RunState::Working][(next() % 3) as usize];
            let mut state = start;
            let mut open = 0i64;
            let mut log = Vec::new();
            for _ in 0..(next() % 12) {
                let effect = match next() % 5 {
                    0 => ActivityEffect::Interrupt,
                    1 if open > 0 => ActivityEffect::Resolved {
                        still_open: open - 1,
                    },
                    2 if next() % 4 == 0 => ActivityEffect::Finished(
                        [Outcome::Done, Outcome::Failed, Outcome::Cancelled][(next() % 3) as usize],
                    ),
                    _ => ActivityEffect::Report,
                };
                match effect {
                    ActivityEffect::Interrupt => open += 1,
                    ActivityEffect::Resolved { .. } => open -= 1,
                    _ => {}
                }
                state = reduce(state, RunEvent::Activity(effect)).expect("activities never fail");
                log.push(effect);
            }
            assert_eq!(replay(start, log), state);
        }
    }

    #[test]
    fn legacy_states_map_as_the_plan_says() {
        let cases = [
            (Legacy::Created, None, RunState::Queued),
            (Legacy::Queued, None, RunState::Queued),
            (Legacy::Starting, None, RunState::Starting),
            (Legacy::Working, None, RunState::Working),
            (Legacy::Blocked, None, RunState::Waiting),
            (Legacy::Recovering, None, RunState::Working),
            (Legacy::Completed, None, RunState::Done),
            (Legacy::Failed, Some("boom"), RunState::Failed),
            (Legacy::Canceled, None, RunState::Cancelled),
            (Legacy::Interrupted, Some("restart"), RunState::Interrupted),
            (
                Legacy::Interrupted,
                Some(STOPPED_BY_REQUESTER),
                RunState::Cancelled,
            ),
        ];
        for (legacy, reason, expected) in cases {
            assert_eq!(RunState::from_legacy(legacy, reason), expected, "{legacy}");
        }
    }

    #[test]
    fn silence_turns_quiet_then_not_responding() {
        assert_eq!(Liveness::after_silence(10, 60), Liveness::Ok);
        assert_eq!(Liveness::after_silence(60, 60), Liveness::Quiet);
        assert_eq!(Liveness::after_silence(179, 60), Liveness::Quiet);
        assert_eq!(Liveness::after_silence(180, 60), Liveness::NotResponding);
    }
}
