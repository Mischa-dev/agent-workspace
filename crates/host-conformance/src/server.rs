//! The suite run against a server: it acts as a host with a one-time code
//! and checks what the server answers.

use agent_workspace_host_protocol::{
    Ack, AckOutcome, ClientError, DescribedAgent, Description, EnrolRequest, Heartbeat, HostClient,
    HostSoftware, Notice, NoticeKind, PROTOCOL_HEADER, PROTOCOL_VERSION,
};
use serde_json::json;
use uuid::Uuid;

use crate::Report;

fn refused_with(result: &Result<impl Sized, ClientError>, statuses: &[u16]) -> bool {
    matches!(result, Err(ClientError::Refused(status, _)) if statuses.contains(status))
}

fn described<T: std::fmt::Debug>(result: &Result<T, ClientError>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(ClientError::Refused(status, body)) => format!("{status} {body}"),
        Err(ClientError::Http(error)) => error.to_string(),
    }
}

/// Act as a host on the server at `base` with the one-time `code`.
pub async fn check_server(base: &str, code: &str) -> Report {
    let mut report = Report::default();
    let request = EnrolRequest {
        code: code.to_owned(),
        host: HostSoftware {
            name: "Conformance host".to_owned(),
            software: Some("host-conformance".to_owned()),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        },
        protocol_versions: vec![PROTOCOL_VERSION.to_owned()],
        connection: None,
        push: None,
    };
    let enrolled = HostClient::enrol(base, &request).await;
    let token = match &enrolled {
        Ok(enrolled) if enrolled.protocol == PROTOCOL_VERSION => {
            report.record("takes a one-time code", true, "");
            enrolled.credential.token.clone()
        }
        _ => {
            report.record("takes a one-time code", false, described(&enrolled));
            return report;
        }
    };
    let again = HostClient::enrol(base, &request).await;
    report.record(
        "takes a code only once",
        refused_with(&again, &[400, 401, 403, 404, 409, 410, 422]),
        described(&again),
    );

    let Ok(client) = HostClient::new(base, token.clone()) else {
        report.record("builds a client", false, "");
        return report;
    };
    let http = reqwest::Client::new();
    let unspoken = http
        .post(format!("{}/api/v1/host/heartbeat", client.base()))
        .bearer_auth(&token)
        .header(PROTOCOL_HEADER, "1999-01")
        .json(&json!({}))
        .send()
        .await;
    report.record(
        "refuses a protocol version it doesn't speak",
        unspoken
            .as_ref()
            .is_ok_and(|response| response.status().is_client_error()),
        unspoken
            .map(|response| response.status().to_string())
            .unwrap_or_else(|error| error.to_string()),
    );
    let stranger = http
        .post(format!("{}/api/v1/host/heartbeat", client.base()))
        .bearer_auth("not-a-host-token")
        .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
        .json(&json!({}))
        .send()
        .await;
    report.record(
        "refuses a token it didn't issue",
        stranger
            .as_ref()
            .is_ok_and(|response| response.status().as_u16() == 401),
        stranger
            .map(|response| response.status().to_string())
            .unwrap_or_else(|error| error.to_string()),
    );

    let description = Description {
        agents: vec![DescribedAgent {
            host_agent_ref: "conformance-agent".to_owned(),
            name: Some("Conformance agent".to_owned()),
            actions: Vec::new(),
        }],
        ..Description::default()
    };
    let answer = client.describe(&description).await;
    report.record(
        "takes a description",
        answer
            .as_ref()
            .is_ok_and(|answer| answer["protocol"] == PROTOCOL_VERSION),
        described(&answer),
    );
    let beat = client.heartbeat(&Heartbeat::default()).await;
    report.record(
        "takes a heartbeat",
        beat.as_ref()
            .is_ok_and(|answer| answer["protocol"] == PROTOCOL_VERSION),
        described(&beat),
    );
    let listed = client.commands(0, 1).await;
    report.record(
        "lists commands by long-poll",
        listed.is_ok(),
        described(&listed),
    );
    let unknown = Uuid::now_v7();
    let fetched = client.command(unknown).await;
    report.record(
        "answers an unknown command with not found",
        refused_with(&fetched, &[404]),
        described(&fetched),
    );
    let acked = client.ack(unknown, &Ack::new(AckOutcome::Accepted)).await;
    report.record(
        "refuses to acknowledge an unknown command",
        refused_with(&acked, &[404]),
        described(&acked),
    );
    // Not bound to any account yet: a notice about it has nothing to apply to.
    let notice = client
        .notice(&Notice::new(NoticeKind::AgentRetired, "conformance-agent"))
        .await;
    report.record(
        "refuses a notice about an agent nobody bound",
        refused_with(&notice, &[400, 404, 409, 422]),
        described(&notice),
    );
    report
}
