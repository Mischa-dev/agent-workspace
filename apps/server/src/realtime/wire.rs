//! Protocol 2 (K5.9), on the same address with `?v=2`: every frame is an
//! object with a `type`; an event carries its position (as a string, since
//! positions can pass JavaScript's safe integers); a connection that says
//! where it stopped (`hello` with `resumeFrom`) is sent what it missed and
//! then `ready` with `resumed: true`, so it needn't refetch everything.
//! Protocol 1's bare envelopes stay until the web client has moved (K5.15).
//!
//! `tests/fixtures/realtime_wire_v2.json` holds an example of every frame,
//! which the server's tests and the web's parse alike.

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::subscriptions::SubscriptionSet;

/// How far back a connection may resume from; further back, it refetches.
pub(crate) const RESUME_LIMIT: i64 = 5_000;
/// How long a protocol 2 connection has to say `hello` before it is served
/// without resuming.
pub(crate) const HELLO_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The protocol a connection asked for in its address.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireQuery {
    pub v: Option<u8>,
}

impl WireQuery {
    pub(crate) fn protocol(self) -> u8 {
        if self.v == Some(2) { 2 } else { 1 }
    }
}

/// A frame from the client. Kinds this server doesn't know yet are ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClientFrame {
    Hello { resume_from: Option<i64> },
    Other,
}

impl ClientFrame {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        match value.get("type")?.as_str()? {
            "hello" => Some(Self::Hello {
                resume_from: value
                    .get("resumeFrom")
                    .and_then(|position| match position {
                        Value::String(text) => text.parse::<i64>().ok(),
                        Value::Number(number) => number.as_i64(),
                        _ => None,
                    })
                    .filter(|position| *position >= 0),
            }),
            _ => Some(Self::Other),
        }
    }
}

pub(crate) fn ready(position: i64, resumed: bool, replayed: usize) -> String {
    json!({
        "type": "ready",
        "protocol": 2,
        "position": position.to_string(),
        "resumed": resumed,
        "replayed": replayed,
    })
    .to_string()
}

/// An event, wrapping protocol 1's envelope.
pub(crate) fn event(position: i64, envelope: &str) -> String {
    format!("{{\"type\":\"event\",\"position\":\"{position}\",\"event\":{envelope}}}")
}

pub(crate) fn resync(reason: &str, position: i64) -> String {
    json!({ "type": "resync", "reason": reason, "position": position.to_string() }).to_string()
}

pub(crate) fn reconnect(after_ms: u64, reason: &str) -> String {
    json!({ "type": "reconnect", "afterMs": after_ms, "reason": reason }).to_string()
}

/// A streamed reply's next words: protocol 1's `message.delta`.
pub(crate) fn stream_delta(conversation: Option<Uuid>, payload: &Value) -> String {
    let field = |snake: &str, camel: &str| {
        payload
            .get(camel)
            .or_else(|| payload.get(snake))
            .cloned()
            .unwrap_or(Value::Null)
    };
    json!({
        "type": "ephemeral",
        "kind": "stream.delta",
        "conversationId": conversation,
        "messageId": field("message_id", "messageId"),
        "runId": field("run_id", "runId"),
        "delta": field("delta", "delta"),
    })
    .to_string()
}

/// What a connection gained and lost when its set was worked out again;
/// None when nothing changed.
pub(crate) fn access(before: &SubscriptionSet, after: &SubscriptionSet) -> Option<String> {
    fn sorted(set: impl Iterator<Item = Uuid>) -> Vec<String> {
        let mut ids = set.map(|id| id.to_string()).collect::<Vec<_>>();
        ids.sort();
        ids
    }
    let added_spaces = sorted(after.spaces.difference(&before.spaces).copied());
    let removed_spaces = sorted(before.spaces.difference(&after.spaces).copied());
    let added_conversations = sorted(after.restricted.difference(&before.restricted).copied());
    let removed_conversations = sorted(before.restricted.difference(&after.restricted).copied());
    let added_workspaces = sorted(after.workspaces.difference(&before.workspaces).copied());
    let removed_workspaces = sorted(before.workspaces.difference(&after.workspaces).copied());
    if added_spaces.is_empty()
        && removed_spaces.is_empty()
        && added_conversations.is_empty()
        && removed_conversations.is_empty()
        && added_workspaces.is_empty()
        && removed_workspaces.is_empty()
    {
        return None;
    }
    Some(
        json!({
            "type": "access",
            "added": {
                "workspaces": added_workspaces,
                "spaces": added_spaces,
                "conversations": added_conversations,
            },
            "removed": {
                "workspaces": removed_workspaces,
                "spaces": removed_spaces,
                "conversations": removed_conversations,
            },
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../tests/fixtures/realtime_wire_v2.json"))
            .expect("fixture")
    }

    /// Every server frame this module writes has the shape the shared
    /// fixture shows, key for key.
    #[test]
    fn server_frames_match_the_shared_fixture() {
        let fixture = fixture();
        let keys = |value: &Value| {
            let mut keys = value
                .as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            keys
        };
        let example = |kind: &str| {
            fixture["server"]
                .as_array()
                .expect("server frames")
                .iter()
                .find(|frame| {
                    let ty = frame["type"].as_str().unwrap_or_default();
                    ty == kind || frame["kind"].as_str() == Some(kind)
                })
                .unwrap_or_else(|| panic!("no {kind} frame in the fixture"))
                .clone()
        };
        let parse = |text: String| serde_json::from_str::<Value>(&text).expect("frame");
        let envelope = example("event")["event"].to_string();
        let written = [
            ("ready", parse(ready(18_350, true, 8))),
            ("event", parse(event(18_343, &envelope))),
            ("resync", parse(resync("too_far_behind", 40_112))),
            ("reconnect", parse(reconnect(1_800, "server_draining"))),
            (
                "stream.delta",
                parse(stream_delta(
                    Some(Uuid::now_v7()),
                    &json!({"message_id": Uuid::now_v7(), "delta": "next"}),
                )),
            ),
        ];
        for (kind, frame) in written {
            assert_eq!(keys(&frame), keys(&example(kind)), "{kind}");
        }
        let gained = SubscriptionSet {
            spaces: HashSet::from([Uuid::now_v7()]),
            ..SubscriptionSet::default()
        };
        let gained_frame = parse(access(&SubscriptionSet::default(), &gained).expect("changed"));
        assert_eq!(keys(&gained_frame), keys(&example("access")));
        assert_eq!(
            keys(&gained_frame["added"]),
            keys(&example("access")["added"]),
            "access lists"
        );
        assert!(access(&gained, &gained).is_none());
        // Positions are strings.
        assert!(parse(ready(1, false, 0))["position"].is_string());
    }

    #[test]
    fn client_frames_in_the_fixture_parse() {
        let fixture = fixture();
        let hello = fixture["client"]
            .as_array()
            .expect("client frames")
            .iter()
            .find(|frame| frame["type"] == "hello")
            .expect("hello")
            .to_string();
        assert_eq!(
            ClientFrame::parse(&hello),
            Some(ClientFrame::Hello {
                resume_from: Some(18_342)
            })
        );
        assert_eq!(
            ClientFrame::parse(r#"{"type":"hello","protocol":2}"#),
            Some(ClientFrame::Hello { resume_from: None })
        );
        assert_eq!(
            ClientFrame::parse(r#"{"type":"hello","resumeFrom":"-4"}"#),
            Some(ClientFrame::Hello { resume_from: None })
        );
        assert_eq!(
            ClientFrame::parse(r#"{"type":"typing","conversationId":"x"}"#),
            Some(ClientFrame::Other)
        );
        assert_eq!(ClientFrame::parse("not json"), None);
    }
}
