//! A1.16, A1.17: the built-in host reads its commands through the public
//! host protocol, like any host (listed, fetched, acknowledged), and
//! carries each out. What carrying out means for a start, a stop, an
//! answer or an inspection is the host's own work, behind [`CommandHost`].

use std::{collections::HashSet, future::Future, time::Duration};

use futures_util::stream::{FuturesUnordered, StreamExt};

use agent_workspace_host_protocol::{Ack, AckOutcome, Command, CommandKind, HostClient};
use uuid::Uuid;

use crate::RuntimeError;

/// The host's own work for each kind of command it carries out.
pub trait CommandHost: Send + Sync {
    /// Start a run (prepare where it runs, then its first turn).
    fn start(&self, client: &HostClient, command: &Command) -> impl Future<Output = Ack> + Send;
    /// Report what the host knows about a run.
    fn inspect(&self, command: &Command) -> impl Future<Output = Ack> + Send;
    /// A person answered what a run asked.
    fn answered(&self, command: &Command) -> impl Future<Output = Ack> + Send;
    /// Stop a run.
    fn cancelled(&self, command: &Command) -> impl Future<Output = Ack> + Send;
    /// Answer a question that isn't about a run, such as one about a model
    /// account (`agent_workspace_host_protocol::account`).
    fn action(&self, command: &Command) -> impl Future<Output = Ack> + Send;
}

/// A refusal, with the reason a server keeps and the message people see.
pub fn refused(reason: &str, message: &str) -> Ack {
    let mut ack = Ack::new(AckOutcome::Refused);
    ack.reason = Some(reason.to_owned());
    ack.message = Some(message.to_owned());
    ack
}

/// How many commands the host carries out at once. A slow one (an account
/// question waits on its harness) doesn't hold up the rest.
pub const IN_FLIGHT: usize = 8;
/// How often the host looks for new commands while some are in flight.
const LOOK_AGAIN: Duration = Duration::from_millis(250);

/// Carry out every command waiting for this host, up to [`IN_FLIGHT`] at
/// once, until none is waiting or in flight. Commands for one run are
/// carried out one at a time, in order. Returns how many.
pub async fn pump<H: CommandHost>(host: &H, client: &HostClient) -> Result<usize, RuntimeError> {
    let mut handled = 0;
    let mut running = FuturesUnordered::new();
    let mut busy_runs = HashSet::new();
    // A command whose acknowledgement is overdue is listed again as
    // uncertain; one still in flight here isn't carried out twice.
    let mut in_flight = HashSet::new();
    loop {
        if running.len() < IN_FLIGHT {
            match client.commands(0, 0).await {
                Ok(listed) => {
                    for summary in listed.commands {
                        if running.len() >= IN_FLIGHT {
                            break;
                        }
                        if in_flight.contains(&summary.command_id)
                            || summary.run_id.is_some_and(|run| busy_runs.contains(&run))
                        {
                            continue;
                        }
                        let Ok(command) = client.command(summary.command_id).await else {
                            // Closed since it was listed: nothing to do.
                            continue;
                        };
                        if let Some(run) = command.run_id {
                            busy_runs.insert(run);
                        }
                        in_flight.insert(command.command_id);
                        running.push(async move {
                            let ack = carry_out(host, client, &command).await;
                            let _ = client.ack(command.command_id, &ack).await;
                            (command.command_id, command.run_id)
                        });
                    }
                }
                Err(_) if running.is_empty() => {
                    return Err(RuntimeError::Internal("read the built-in host's commands"));
                }
                // The ones in flight still finish; the list is read again.
                Err(_) => {}
            }
        }
        if running.is_empty() {
            return Ok(handled);
        }
        tokio::select! {
            Some((command_id, run)) = running.next() => {
                handled += 1;
                in_flight.remove(&command_id);
                if let Some(run) = run {
                    busy_runs.remove(&run);
                }
            }
            () = tokio::time::sleep(LOOK_AGAIN) => {}
        }
    }
}

/// Carry out at once what is waiting for one run, of these kinds: a
/// person's Stop is heard before the answer to it returns.
pub async fn hear_now<H: CommandHost>(
    host: &H,
    client: &HostClient,
    run_id: Uuid,
    kinds: &[CommandKind],
) -> Result<(), RuntimeError> {
    let listed = client
        .commands(0, 0)
        .await
        .map_err(|_| RuntimeError::Internal("read the built-in host's commands"))?;
    for summary in listed.commands {
        if summary.run_id != Some(run_id) || !kinds.contains(&summary.kind) {
            continue;
        }
        let Ok(command) = client.command(summary.command_id).await else {
            continue;
        };
        let ack = carry_out(host, client, &command).await;
        let _ = client.ack(command.command_id, &ack).await;
    }
    Ok(())
}

async fn carry_out<H: CommandHost>(host: &H, client: &HostClient, command: &Command) -> Ack {
    match command.kind {
        CommandKind::Start => host.start(client, command).await,
        CommandKind::Inspect => host.inspect(command).await,
        CommandKind::Answer => host.answered(command).await,
        CommandKind::Cancel => host.cancelled(command).await,
        CommandKind::Steer => refused(
            "unsupported",
            "The built-in host takes a new message as a new run.",
        ),
        CommandKind::Action => host.action(command).await,
    }
}

/// Carry out one command this process claimed when it wrote it (the
/// server asking its own built-in host, in process), and acknowledge it.
/// `handed` is what the asker hands over in memory beside the command
/// (such as an API key to sign in with), never written anywhere.
pub async fn carry_out_one<H: CommandHost>(
    host: &H,
    client: &HostClient,
    command_id: Uuid,
    handed: serde_json::Map<String, serde_json::Value>,
) -> Result<Ack, RuntimeError> {
    let mut command = client
        .command(command_id)
        .await
        .map_err(|_| RuntimeError::Internal("read the built-in host's command"))?;
    command.detail.extend(handed);
    let ack = carry_out(host, client, &command).await;
    client
        .ack(command.command_id, &ack)
        .await
        .map_err(|_| RuntimeError::Internal("acknowledge the built-in host's command"))?;
    Ok(ack)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use agent_workspace_host_protocol::{
        AckReply, ClientError, CommandList, CommandSummary, HttpRequest, HttpResponse, Transport,
        TransportFuture,
    };
    use serde_json::json;
    use tokio::sync::Notify;

    use super::*;

    /// A server holding two `action` commands, the first carried out only
    /// once the second has been.
    #[derive(Debug, Default)]
    struct TwoCommands {
        fetched: Mutex<Vec<Uuid>>,
        acked: Mutex<Vec<Uuid>>,
    }

    const SLOW: Uuid = Uuid::from_u128(1);
    const QUICK: Uuid = Uuid::from_u128(2);

    fn command(command_id: Uuid) -> Command {
        Command {
            command_id,
            seq: i64::try_from(command_id.as_u128()).unwrap_or_default(),
            kind: CommandKind::Action,
            run_id: None,
            request_id: None,
            start_key: None,
            issued_at: String::new(),
            expires_at: String::new(),
            workspace_credential: None,
            detail: serde_json::Map::new(),
        }
    }

    fn ok(value: &impl serde::Serialize) -> Result<HttpResponse, ClientError> {
        Ok(HttpResponse {
            status: 200,
            body: serde_json::to_vec(value).expect("a body"),
            headers: Vec::new(),
        })
    }

    impl Transport for TwoCommands {
        fn send(&self, request: HttpRequest) -> TransportFuture<'_> {
            Box::pin(async move {
                let id = |path: &str| {
                    path.trim_start_matches("/api/v1/host/commands/")
                        .trim_end_matches("/ack")
                        .parse::<Uuid>()
                        .expect("a command id")
                };
                if request.path.starts_with("/api/v1/host/commands?") {
                    let fetched = self.fetched.lock().expect("fetched").clone();
                    let commands = [SLOW, QUICK]
                        .into_iter()
                        .filter(|command_id| !fetched.contains(command_id))
                        .map(|command_id| CommandSummary {
                            command_id,
                            seq: 0,
                            kind: CommandKind::Action,
                            run_id: None,
                            offer: 1,
                        })
                        .collect();
                    return ok(&CommandList { commands, next: 0 });
                }
                let command_id = id(&request.path);
                if request.method == "POST" {
                    self.acked.lock().expect("acked").push(command_id);
                    return ok(&AckReply {
                        command_id,
                        state: "acked".to_owned(),
                        run_state: None,
                    });
                }
                self.fetched.lock().expect("fetched").push(command_id);
                ok(&command(command_id))
            })
        }
    }

    struct Host {
        quick_done: Notify,
    }

    impl CommandHost for Host {
        async fn start(&self, _: &HostClient, _: &Command) -> Ack {
            refused("unused", "unused")
        }
        async fn inspect(&self, _: &Command) -> Ack {
            refused("unused", "unused")
        }
        async fn answered(&self, _: &Command) -> Ack {
            refused("unused", "unused")
        }
        async fn cancelled(&self, _: &Command) -> Ack {
            refused("unused", "unused")
        }
        async fn action(&self, command: &Command) -> Ack {
            if command.command_id == SLOW {
                self.quick_done.notified().await;
            } else {
                self.quick_done.notify_one();
            }
            let mut ack = Ack::new(AckOutcome::Accepted);
            ack.result = Some(json!({}));
            ack
        }
    }

    #[tokio::test]
    async fn a_slow_command_does_not_hold_up_the_next() {
        let server = Arc::new(TwoCommands::default());
        let client = HostClient::with_transport("http://localhost", "token", server.clone());
        let host = Host {
            quick_done: Notify::new(),
        };
        let handled = tokio::time::timeout(Duration::from_secs(10), pump(&host, &client))
            .await
            .expect("the slow command waits only for the quick one")
            .expect("the pump");
        assert_eq!(handled, 2);
        assert_eq!(*server.acked.lock().expect("acked"), vec![QUICK, SLOW]);
    }
}
