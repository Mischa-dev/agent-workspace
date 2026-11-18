//! Why an account is told about an event (A2 3.3), from facts the read
//! query works out per event and recipient.

use serde::Serialize;

/// How strongly an event concerns the account; a subscription's
/// `min_reason` is the weakest it wants.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Visible,
    Following,
    Addressed,
}

impl Level {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "visible" => Some(Self::Visible),
            "following" => Some(Self::Following),
            "addressed" => Some(Self::Addressed),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Following => "following",
            Self::Addressed => "addressed",
        }
    }
}

/// The `reason` on a delivered event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A required reply request for this account.
    ReplyRequired,
    /// An optional one (a group, a channel tag).
    ReplyWelcome,
    Mentioned,
    Assigned,
    /// A request it may answer was raised.
    RequestForYou,
    /// A request it raised was settled.
    RequestAnswered,
    /// A run it does was asked to stop.
    HostCommand,
    /// About the account or its connection: a follow-up due, pushes off, a
    /// stopped reply request.
    Lifecycle,
    /// The account caused it; only for subscriptions that ask for echoes.
    #[serde(rename = "self")]
    Own,
    /// It follows the thing: a task it watches or is assigned, a run it
    /// does or asked for.
    Following,
    Visible,
}

impl Reason {
    pub const fn key(self) -> &'static str {
        match self {
            Self::ReplyRequired => "reply_required",
            Self::ReplyWelcome => "reply_welcome",
            Self::Mentioned => "mentioned",
            Self::Assigned => "assigned",
            Self::RequestForYou => "request_for_you",
            Self::RequestAnswered => "request_answered",
            Self::HostCommand => "host_command",
            Self::Lifecycle => "lifecycle",
            Self::Own => "self",
            Self::Following => "following",
            Self::Visible => "visible",
        }
    }

    /// The reason a stored key names.
    pub fn parse(key: &str) -> Option<Self> {
        [
            Self::ReplyRequired,
            Self::ReplyWelcome,
            Self::Mentioned,
            Self::Assigned,
            Self::RequestForYou,
            Self::RequestAnswered,
            Self::HostCommand,
            Self::Lifecycle,
            Self::Own,
            Self::Following,
            Self::Visible,
        ]
        .into_iter()
        .find(|reason| reason.key() == key)
    }

    pub const fn level(self) -> Level {
        match self {
            Self::Following => Level::Following,
            Self::Visible => Level::Visible,
            _ => Level::Addressed,
        }
    }
}

/// What the read query found about one event for one recipient.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    /// The event names the recipient as its audience.
    pub addressed: bool,
    /// The recipient acted.
    pub own: bool,
    pub following: bool,
    /// The event is about a request the recipient raised.
    pub raiser: bool,
    /// A request the recipient may answer was raised.
    pub for_you: bool,
    /// A reply request the event wakes for was required.
    pub required: bool,
}

/// Why the recipient hears an event of type `kind`, before any
/// subscription decides whether it wants it.
pub fn decide(kind: &str, facts: Facts) -> Reason {
    if facts.addressed {
        return match kind {
            "reply.requested" | "reply.followup" if facts.required => Reason::ReplyRequired,
            "reply.requested" | "reply.followup" => Reason::ReplyWelcome,
            "message.mentioned" | "comment.mentioned" => Reason::Mentioned,
            "task.assigned" | "task.delegated" | "run.start" => Reason::Assigned,
            "run.cancel" => Reason::HostCommand,
            _ => Reason::Lifecycle,
        };
    }
    if kind == "request.raised" && facts.for_you && !facts.own {
        return Reason::RequestForYou;
    }
    if facts.raiser
        && matches!(
            kind,
            "request.answered" | "request.cancelled" | "request.expired"
        )
    {
        return Reason::RequestAnswered;
    }
    if facts.own {
        return Reason::Own;
    }
    if facts.following {
        return Reason::Following;
    }
    Reason::Visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressed_events_say_why() {
        let addressed = Facts {
            addressed: true,
            ..Facts::default()
        };
        assert_eq!(
            decide(
                "reply.requested",
                Facts {
                    required: true,
                    ..addressed
                }
            ),
            Reason::ReplyRequired
        );
        assert_eq!(decide("reply.followup", addressed), Reason::ReplyWelcome);
        assert_eq!(decide("task.assigned", addressed), Reason::Assigned);
        assert_eq!(decide("schedule.due", addressed), Reason::Lifecycle);
        assert_eq!(decide("run.cancel", addressed).level(), Level::Addressed);
    }

    #[test]
    fn requests_reach_who_answers_and_who_asked() {
        let for_you = Facts {
            for_you: true,
            ..Facts::default()
        };
        assert_eq!(decide("request.raised", for_you), Reason::RequestForYou);
        let raiser = Facts {
            raiser: true,
            own: false,
            ..Facts::default()
        };
        assert_eq!(decide("request.answered", raiser), Reason::RequestAnswered);
        // Raising your own request is your own doing.
        let own = Facts {
            raiser: true,
            own: true,
            ..Facts::default()
        };
        assert_eq!(decide("request.raised", own), Reason::Own);
    }

    #[test]
    fn levels_order_from_visible_to_addressed() {
        assert!(Level::Visible < Level::Following);
        assert!(Level::Following < Level::Addressed);
        assert_eq!(
            decide(
                "task.updated",
                Facts {
                    following: true,
                    ..Facts::default()
                }
            ),
            Reason::Following
        );
        assert_eq!(decide("task.updated", Facts::default()), Reason::Visible);
    }
}
