//! The host protocol's conformance suite (A1.23), runnable both ways:
//!
//! - **Against a host** ([`check_host`]): [`FakeWorkspace`] serves the
//!   host's side of the protocol on a local port, as a workspace would,
//!   and drives a real host through enrolment, describe, heartbeats, a
//!   start, a start whose acknowledgement is lost and offered again, a
//!   cancel, a steer, an answer, an inspect, its actions and notices. It
//!   says what the host lacks: a host that starts a re-offered start a
//!   second time fails "start not idempotent". A host author's CI can use
//!   it as a stand-in workspace.
//! - **Against a server** ([`check_server`]): it acts as a host on a real
//!   server with a one-time code and checks the server's answers.

mod fake;
mod server;

pub use fake::{FakeWorkspace, check_host};
pub use server::check_server;

use std::fmt;

/// How one check came out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Pass,
    Fail,
    /// Not checked: the host doesn't offer what it needs.
    Skip,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
    pub detail: String,
}

/// Every check, in the order they ran.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    fn record(&mut self, name: &'static str, passed: bool, detail: impl Into<String>) -> bool {
        self.checks.push(Check {
            name,
            outcome: if passed { Outcome::Pass } else { Outcome::Fail },
            detail: detail.into(),
        });
        passed
    }

    fn skip(&mut self, name: &'static str, detail: impl Into<String>) {
        self.checks.push(Check {
            name,
            outcome: Outcome::Skip,
            detail: detail.into(),
        });
    }

    /// No check failed.
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.outcome != Outcome::Fail)
    }

    /// The check called `name`, if it ran.
    pub fn check(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|check| check.name == name)
    }

    /// What failed, by name.
    pub fn failures(&self) -> Vec<&'static str> {
        self.checks
            .iter()
            .filter(|check| check.outcome == Outcome::Fail)
            .map(|check| check.name)
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for check in &self.checks {
            let mark = match check.outcome {
                Outcome::Pass => "pass",
                Outcome::Fail => "FAIL",
                Outcome::Skip => "skip",
            };
            write!(formatter, "{mark}  {}", check.name)?;
            if !check.detail.is_empty() {
                write!(formatter, ": {}", check.detail)?;
            }
            writeln!(formatter)?;
        }
        let failed = self.failures().len();
        if failed == 0 {
            writeln!(formatter, "\nEvery check passed.")
        } else {
            writeln!(formatter, "\n{failed} check(s) failed.")
        }
    }
}
