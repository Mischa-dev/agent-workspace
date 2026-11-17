//! The suite run against a server, as an agent with a token reads it.

use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode, header};
use serde_json::Value;

use crate::{Report, check_envelope, conforms, fixtures};

/// The longest the suite waits for the stimulus's event to arrive.
const EVENT_WITHIN: Duration = Duration::from_secs(120);

struct Server {
    http: Client,
    origin: String,
    token: String,
}

impl Server {
    async fn get(&self, path: &str) -> Result<(StatusCode, Value), String> {
        let response = self
            .http
            .get(format!("{}{path}", self.origin))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// One page after `cursor`, waiting up to `wait` seconds.
    async fn page(&self, cursor: Option<&str>, wait: u64) -> Result<Value, String> {
        let mut path = format!("/api/v1/events?limit=100&wait={wait}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&encode(cursor));
        }
        match self.get(&path).await? {
            (status, body) if status.is_success() => Ok(body),
            (status, body) => Err(format!("{status} {body}")),
        }
    }

    /// The SSE events of a stream opened at `cursor` (or `Last-Event-ID`),
    /// read until one has `until` as its envelope's id or `within` passes.
    async fn stream(
        &self,
        cursor: Option<&str>,
        last_event_id: Option<&str>,
        until: &Value,
        within: Duration,
    ) -> Result<Vec<SseEvent>, String> {
        let mut path = "/api/v1/events".to_owned();
        if let Some(cursor) = cursor {
            path.push_str("?cursor=");
            path.push_str(&encode(cursor));
        }
        let mut request = self
            .http
            .get(format!("{}{path}", self.origin))
            .bearer_auth(&self.token)
            .header(header::ACCEPT, "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id);
        }
        let mut response = request.send().await.map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(response.status().to_string());
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if !content_type.starts_with("text/event-stream") {
            return Err(format!("content type {content_type}"));
        }
        let deadline = tokio::time::Instant::now() + within;
        let mut text = String::new();
        loop {
            let found = sse_events(&text);
            if found.iter().any(|(_, _, data)| {
                serde_json::from_str::<Value>(data).is_ok_and(|data| &data["id"] == until)
            }) {
                break;
            }
            match tokio::time::timeout_at(deadline, response.chunk()).await {
                Ok(Ok(Some(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Ok(None)) | Err(_) => break,
                Ok(Err(error)) => return Err(error.to_string()),
            }
        }
        Ok(sse_events(&text))
    }
}

/// A cursor for a query string (cursors are URL-safe, but be sure).
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// One SSE event: its `id`, `event` and `data`.
pub(crate) type SseEvent = (Option<String>, Option<String>, String);

/// The complete events in an SSE text, comments and keep-alives left out.
pub(crate) fn sse_events(text: &str) -> Vec<SseEvent> {
    let normalized = text.replace("\r\n", "\n");
    let mut blocks = normalized.split("\n\n").collect::<Vec<_>>();
    // The last block is complete only when the text ends with a blank line.
    if !normalized.ends_with("\n\n") {
        blocks.pop();
    }
    blocks
        .into_iter()
        .filter_map(|block| {
            let mut id = None;
            let mut event = None;
            let mut data = Vec::new();
            for line in block.lines() {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "id" => id = Some(value.to_owned()),
                    "event" => event = Some(value.to_owned()),
                    "data" => data.push(value.to_owned()),
                    _ => {}
                }
            }
            (!data.is_empty()).then(|| (id, event, data.join("\n")))
        })
        .collect()
}

/// Reads on from `cursor` to the end of what is there, returning the
/// cursor at the end and every event seen.
async fn drain(server: &Server, cursor: Option<String>) -> Result<(String, Vec<Value>), String> {
    let mut cursor = cursor;
    let mut seen = Vec::new();
    for _ in 0..50 {
        let page = server.page(cursor.as_deref(), 0).await?;
        seen.extend(page["events"].as_array().cloned().unwrap_or_default());
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        if page["hasMore"] != Value::Bool(true) {
            break;
        }
    }
    cursor
        .map(|cursor| (cursor, seen))
        .ok_or_else(|| "no nextCursor".to_owned())
}

fn check_events(events: &[Value]) -> Result<(), String> {
    events.iter().try_for_each(|event| {
        check_envelope(event).map_err(|error| format!("{}: {error}", event["type"]))
    })
}

/// Reads the server at `origin` as the account `token` signs in as. With a
/// `stimulus` (something that makes one event this account hears), it also
/// checks that a waiting long-poll gets it, that a stream carries it with
/// its cursor as the id, and that resuming from it repeats nothing.
pub async fn check_server<F>(origin: &str, token: &str, stimulus: Option<F>) -> Report
where
    F: AsyncFnOnce() -> Result<(), String>,
{
    let mut report = Report::default();
    let server = Server {
        http: Client::new(),
        origin: origin.trim_end_matches('/').to_owned(),
        token: token.to_owned(),
    };

    match server.get("/api/v1/events/catalogue").await {
        Ok((status, body)) if status.is_success() => {
            let entry = fixtures::parse(fixtures::CATALOGUE_ENTRY);
            let entries = body["events"].as_array().cloned().unwrap_or_default();
            let shaped = entries.iter().try_for_each(|found| {
                conforms(
                    &entry,
                    found,
                    &["kind", "family", "addressed", "summary"],
                    &[],
                )
            });
            report.result(
                "lists what can be heard",
                if entries.is_empty() {
                    Err("the catalogue is empty".to_owned())
                } else {
                    shaped
                },
            );
        }
        Ok((status, body)) => {
            report.record("lists what can be heard", false, format!("{status} {body}"));
        }
        Err(error) => {
            report.record("lists what can be heard", false, error);
        }
    }

    let anonymous = server
        .http
        .get(format!("{}/api/v1/events", server.origin))
        .send()
        .await;
    report.record(
        "refuses a read without a credential",
        anonymous
            .as_ref()
            .is_ok_and(|response| response.status() == StatusCode::UNAUTHORIZED),
        anonymous
            .map(|response| response.status().to_string())
            .unwrap_or_else(|error| error.to_string()),
    );

    let first = match server.page(None, 0).await {
        Ok(page) => page,
        Err(error) => {
            report.record("answers a poll", false, error);
            return report;
        }
    };
    let shaped = conforms(
        &fixtures::parse(fixtures::POLL_PAGE),
        &first,
        &["events", "nextCursor", "hasMore", "connectionId"],
        &["events"],
    );
    report.result("answers a poll", shaped);
    let (head, history) =
        match drain(&server, first["nextCursor"].as_str().map(str::to_owned)).await {
            Ok(found) => found,
            Err(error) => {
                report.record("reads on to the end", false, error);
                return report;
            }
        };
    let mut seen = first["events"].as_array().cloned().unwrap_or_default();
    seen.extend(history);
    report.result(
        "sends envelopes shaped like the fixture, without text",
        check_events(&seen),
    );

    let refused = server.get("/api/v1/events?cursor=not-a-cursor").await;
    report.record(
        "refuses a cursor it didn't issue",
        matches!(&refused, Ok((status, _)) if status.is_client_error()),
        match refused {
            Ok((status, body)) => format!("{status} {body}"),
            Err(error) => error,
        },
    );

    let started = Instant::now();
    let waited = server.page(Some(&head), 2).await;
    let elapsed = started.elapsed();
    report.record(
        "holds a long-poll open while nothing is new",
        match &waited {
            Ok(page) => {
                page["events"]
                    .as_array()
                    .is_some_and(|events| !events.is_empty())
                    || elapsed >= Duration::from_millis(1500)
            }
            Err(_) => false,
        },
        format!("{elapsed:?}"),
    );

    let Some(stimulus) = stimulus else {
        for name in [
            "wakes a waiting long-poll",
            "streams an event with its cursor as the id",
            "resumes a stream without repeats",
            "reads on from an event's cursor without repeats",
        ] {
            report.skip(name, "no stimulus to make an event");
        }
        return report;
    };
    let waiting = {
        let server = &server;
        let head = head.clone();
        async move {
            let deadline = Instant::now() + EVENT_WITHIN;
            let mut cursor = head;
            while Instant::now() < deadline {
                let page = server.page(Some(&cursor), 25).await?;
                if let Some(event) = page["events"].as_array().and_then(|events| events.first()) {
                    return Ok(event.clone());
                }
                cursor = page["nextCursor"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or(cursor);
            }
            Err("nothing arrived".to_owned())
        }
    };
    let made = async {
        // Let the long-poll start waiting first.
        tokio::time::sleep(Duration::from_millis(300)).await;
        stimulus().await
    };
    let (woken, made) = tokio::join!(waiting, made);
    if let Err(error) = made {
        report.record(
            "wakes a waiting long-poll",
            false,
            format!("stimulus: {error}"),
        );
        return report;
    }
    let event = match woken {
        Ok(event) => {
            report.result(
                "wakes a waiting long-poll",
                check_events(std::slice::from_ref(&event)),
            );
            event
        }
        Err(error) => {
            report.record("wakes a waiting long-poll", false, error);
            return report;
        }
    };
    let cursor = event["cursor"].as_str().unwrap_or_default().to_owned();
    let id = event["id"].as_str().unwrap_or_default().to_owned();

    let streamed = server
        .stream(Some(&head), None, &event["id"], Duration::from_secs(20))
        .await;
    report.result(
        "streams an event with its cursor as the id",
        streamed.and_then(|events| {
            let (sse_id, kind, data) = events
                .iter()
                .find_map(|(sse_id, kind, data)| {
                    let data = serde_json::from_str::<Value>(data).ok()?;
                    (data["id"] == event["id"]).then_some((sse_id, kind, data))
                })
                .ok_or_else(|| format!("{id} didn't come on the stream"))?;
            check_envelope(&data)?;
            if sse_id.as_deref() != Some(cursor.as_str()) {
                return Err(format!("id {sse_id:?} isn't the event's cursor"));
            }
            if kind.as_deref() != data["type"].as_str() {
                return Err(format!("event {kind:?} isn't the type {}", data["type"]));
            }
            Ok(())
        }),
    );

    let resumed = server
        .stream(None, Some(&cursor), &event["id"], Duration::from_secs(3))
        .await;
    report.result(
        "resumes a stream without repeats",
        resumed.and_then(|events| {
            let repeated = events.iter().any(|(_, _, data)| {
                serde_json::from_str::<Value>(data).is_ok_and(|data| data["id"] == event["id"])
            });
            if repeated {
                Err(format!("{id} came again after Last-Event-ID"))
            } else {
                Ok(())
            }
        }),
    );

    let after = server.page(Some(&cursor), 0).await;
    report.result(
        "reads on from an event's cursor without repeats",
        after.and_then(|page| {
            let repeated = page["events"]
                .as_array()
                .is_some_and(|events| events.iter().any(|found| found["id"] == event["id"]));
            if repeated {
                Err(format!("{id} came again"))
            } else {
                Ok(())
            }
        }),
    );
    report
}
