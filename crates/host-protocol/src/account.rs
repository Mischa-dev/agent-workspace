//! A1.17: what a server asks the built-in host about one of its model
//! accounts, and what the host answers. The server writes the question as
//! an `action` command (`{"action": "account", "input": AccountAsk}`), the
//! host carries it out with the account's harness and answers in the
//! acknowledgement's `result`. A refusal's `reason` says what went wrong in
//! a word: [`REFUSED_BUSY`], [`REFUSED_UNSUPPORTED`], or what failed (such
//! as `transport` or `circuit_open`; [`REFUSED_FAILED`] when nothing more
//! is known).
//!
//! Nothing secret travels in a command: an API key to sign in with is
//! sealed beside its sign-in (`ApiKeyLogin` names the sign-in), and the host
//! takes it from there once.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::harness::manifest::{LoginKind, ModelEntry};

/// The `action` an account question is written as.
pub const ACCOUNT_ACTION: &str = "account";

/// Every control session of the account's harness is busy: ask again soon.
pub const REFUSED_BUSY: &str = "busy";
/// The account's harness can't do what was asked.
pub const REFUSED_UNSUPPORTED: &str = "unsupported";
/// The harness was asked and failed, or couldn't be reached.
pub const REFUSED_FAILED: &str = "failed";

/// A question about one account, with what the host needs to reach it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAsk {
    pub account_id: Uuid,
    /// The account's harness kind.
    pub harness: String,
    /// The account's private home on the host.
    pub home: String,
    pub max_concurrent_runs: u16,
    pub question: AccountQuestion,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum AccountQuestion {
    /// Whether the account is signed in, as whom, and its usage windows.
    Status,
    /// Start a browser or device-code sign-in.
    Login { kind: LoginKind },
    /// Sign in with the API key sealed beside this sign-in.
    #[serde(rename_all = "camelCase")]
    ApiKeyLogin { ceremony_id: Uuid },
    /// Stop a sign-in the harness started.
    #[serde(rename_all = "camelCase")]
    CancelLogin { login_id: String },
    /// Sign the account out of its harness.
    Logout,
    /// Read the account's usage and report its windows the way any host
    /// does (`POST /api/v1/host/windows`); answered `Done`.
    Usage,
    /// Spend one of the account's banked resets.
    #[serde(rename_all = "camelCase")]
    SpendReset {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credit_id: Option<String>,
        /// The same key for a retry of the same attempt.
        attempt_key: String,
    },
    /// The models the account can run, as the harness lists them now.
    Models,
    /// Whether the harness can reach a cloud environment by this id or
    /// label with the account. Refused as unsupported by a harness that
    /// has no environments of its own.
    CheckEnvironment { environment: String },
}

/// The host's answer, in the acknowledgement's `result`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum AccountAnswer {
    #[serde(rename_all = "camelCase")]
    Status {
        authenticated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rate_limits: Option<RateLimits>,
    },
    /// A sign-in started (or, for an API key, finished).
    LoginStarted {
        start: LoginStarted,
    },
    /// Done, with nothing to say.
    Done,
    ResetSpent {
        outcome: ResetSpent,
    },
    Models {
        models: Vec<ModelEntry>,
    },
    /// Whether a cloud environment was found for the account.
    Environment {
        found: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoginStarted {
    #[serde(rename_all = "camelCase")]
    Browser {
        login_id: String,
        auth_url: String,
    },
    #[serde(rename_all = "camelCase")]
    DeviceCode {
        login_id: String,
        verification_url: String,
        user_code: String,
    },
    ApiKey,
}

/// The two usage windows a status reports, when the harness has them.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<RateLimitWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<RateLimitWindow>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_duration_minutes: Option<i64>,
}

/// What spending a banked reset did.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetSpent {
    Reset,
    NothingToReset,
    NoCredit,
    /// The same attempt already went through.
    AlreadyRedeemed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn questions_and_answers_read_as_written() {
        let ask = AccountAsk {
            account_id: Uuid::nil(),
            harness: "demo".into(),
            home: "/accounts/a".into(),
            max_concurrent_runs: 2,
            question: AccountQuestion::SpendReset {
                credit_id: None,
                attempt_key: "k".into(),
            },
        };
        let written = serde_json::to_value(&ask).expect("write a question");
        assert_eq!(
            written["question"],
            json!({"ask": "spend_reset", "attemptKey": "k"})
        );
        assert_eq!(
            serde_json::from_value::<AccountAsk>(written).expect("read a question"),
            ask
        );

        let answer = AccountAnswer::LoginStarted {
            start: LoginStarted::DeviceCode {
                login_id: "l".into(),
                verification_url: "https://example.test/device".into(),
                user_code: "ABCD".into(),
            },
        };
        let written = serde_json::to_value(&answer).expect("write an answer");
        assert_eq!(written["answer"], "login_started");
        assert_eq!(written["start"]["kind"], "device_code");
        assert_eq!(
            serde_json::from_value::<AccountAnswer>(written).expect("read an answer"),
            answer
        );
    }
}
