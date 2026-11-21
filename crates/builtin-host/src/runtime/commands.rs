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
/// After this many starts, or [`BETWEEN_AGAIN`], a pass that is still busy
/// gives what runs between passes (admitting runs) its turn.
pub const PER_PASS: usize = 4 * IN_FLIGHT;
const BETWEEN_AGAIN: Duration = Duration::from_secs(1);

/// Carry out the commands waiting for this host, up to [`IN_FLIGHT`] at
/// once, until none is waiting or in flight. Commands for one run are
/// carried out one at a time, in order. Returns how many.
///
/// The host keeps looking for new commands the whole time, so one that
/// waits to be picked up (an account question) is, however slow the ones
/// in flight are. While the pass is busy, `between` runs every
/// [`PER_PASS`] starts or [`BETWEEN_AGAIN`] (the server admits runs
/// there); if it fails, no more commands start, the ones in flight
/// finish, and the pass ends with its error.
pub async fn pump<H, B, F, E>(host: &H, client: &HostClient, mut between: B) -> Result<usize, E>
where
    H: CommandHost,
    B: FnMut() -> F,
    F: Future<Output = Result<(), E>>,
    E: From<RuntimeError>,
{
    let mut handled = 0;
    let mut running = FuturesUnordered::new();
    let mut busy_runs = HashSet::new();
    // A command whose acknowledgement is overdue is listed again as
    // uncertain; one still in flight here isn't carried out twice.
    let mut in_flight = HashSet::new();
    let mut started_since = 0;
    let mut since = tokio::time::Instant::now();
    let mut stopped = None;
    // Commands that finished while the pump waited on something else.
    let mut finished = Vec::new();
    loop {
        // Admission (and the lease check in it) comes before anything new is
        // fetched: once it fails, nothing more starts.
        if stopped.is_none() && (started_since >= PER_PASS || since.elapsed() >= BETWEEN_AGAIN) {
            // The commands in flight keep going meanwhile: they may hold what
            // admission waits for (a database connection).
            if let Err(error) = alongside(&mut running, &mut finished, between()).await {
                stopped = Some(error);
            }
            started_since = 0;
            since = tokio::time::Instant::now();
        }
        if stopped.is_none() && running.len() < IN_FLIGHT {
            match alongside(&mut running, &mut finished, client.commands(0, 0)).await {
                Ok(listed) => {
                    for summary in listed.commands {
                        // Commands finishing while others are fetched free
                        // slots; admission still gets its turn on time.
                        if running.len() >= IN_FLIGHT
                            || started_since >= PER_PASS
                            || since.elapsed() >= BETWEEN_AGAIN
                        {
                            break;
                        }
                        if in_flight.contains(&summary.command_id)
                            || summary.run_id.is_some_and(|run| busy_runs.contains(&run))
                        {
                            continue;
                        }
                        let fetched = alongside(
                            &mut running,
                            &mut finished,
                            client.command(summary.command_id),
                        )
                        .await;
                        let Ok(command) = fetched else {
                            // Closed since it was listed: nothing to do.
                            continue;
                        };
                        if let Some(run) = command.run_id {
                            busy_runs.insert(run);
                        }
                        in_flight.insert(command.command_id);
                        started_since += 1;
                        running.push(async move {
                            let ack = carry_out(host, client, &command).await;
                            let _ = client.ack(command.command_id, &ack).await;
                            (command.command_id, command.run_id)
                        });
                    }
                }
                Err(_) if running.is_empty() => {
                    return Err(RuntimeError::Internal("read the built-in host's commands").into());
                }
                // The ones in flight still finish; the list is read again.
                Err(_) => {}
            }
        }
        for (command_id, run) in finished.drain(..) {
            handled += 1;
            in_flight.remove(&command_id);
            if let Some(run) = run {
                busy_runs.remove(&run);
            }
        }
        if running.is_empty() {
            return match stopped {
                Some(error) => Err(error),
                None => Ok(handled),
            };
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

/// Wait for `work` while the commands in flight keep being carried out;
/// those that finish meanwhile are kept in `finished`.
async fn alongside<R: Future, T>(
    running: &mut FuturesUnordered<R>,
    finished: &mut Vec<R::Output>,
    work: impl Future<Output = T>,
) -> T {
    let mut work = std::pin::pin!(work);
    loop {
        tokio::select! {
            out = &mut work => return out,
            Some(done) = running.next(), if !running.is_empty() => finished.push(done),
        }
    }
}

/// Nothing to do between commands: for a host with nothing else to run.
pub async fn nothing_between() -> Result<(), RuntimeError> {
    Ok(())
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
        let handled = tokio::time::timeout(
            Duration::from_secs(10),
            pump(&host, &client, nothing_between),
        )
        .await
        .expect("the slow command waits only for the quick one")
        .expect("the pump");
        assert_eq!(handled, 2);
        assert_eq!(*server.acked.lock().expect("acked"), vec![QUICK, SLOW]);
    }

    /// A server with one slow command waiting (if `slow`), and `limit` quick
    /// ones, one more each time it is listed.
    #[derive(Debug)]
    struct Queue {
        slow: bool,
        limit: u128,
        listed: Mutex<u128>,
        slow_listed: Mutex<bool>,
    }

    impl Queue {
        fn new(slow: bool, limit: u128) -> Self {
            Self {
                slow,
                limit,
                listed: Mutex::new(0),
                slow_listed: Mutex::new(false),
            }
        }
    }

    impl Transport for Queue {
        fn send(&self, request: HttpRequest) -> TransportFuture<'_> {
            Box::pin(async move {
                if request.path.starts_with("/api/v1/host/commands?") {
                    let mut commands = Vec::new();
                    let mut slow_listed = self.slow_listed.lock().expect("slow listed");
                    if self.slow && !*slow_listed {
                        *slow_listed = true;
                        commands.push(SLOW);
                    }
                    let mut listed = self.listed.lock().expect("listed");
                    if *listed < self.limit {
                        *listed += 1;
                        commands.push(Uuid::from_u128(1000 + *listed));
                    }
                    let commands = commands
                        .into_iter()
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
                let command_id = request
                    .path
                    .trim_start_matches("/api/v1/host/commands/")
                    .trim_end_matches("/ack")
                    .parse::<Uuid>()
                    .expect("a command id");
                if request.method == "POST" {
                    return ok(&AckReply {
                        command_id,
                        state: "acked".to_owned(),
                        run_state: None,
                    });
                }
                ok(&command(command_id))
            })
        }
    }

    /// The slow command finishes only once `wait_for` quick ones have.
    struct Waits {
        wait_for: usize,
        quick: std::sync::atomic::AtomicUsize,
        enough: Notify,
    }

    impl CommandHost for Waits {
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
                self.enough.notified().await;
            } else {
                let done = self.quick.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if done == self.wait_for {
                    self.enough.notify_one();
                }
            }
            let mut ack = Ack::new(AckOutcome::Accepted);
            ack.result = Some(json!({}));
            ack
        }
    }

    #[tokio::test]
    async fn new_commands_are_picked_up_past_a_pass_while_a_slow_one_waits() {
        let wait_for = 2 * PER_PASS;
        let client = HostClient::with_transport(
            "http://localhost",
            "token",
            Arc::new(Queue::new(true, u128::try_from(wait_for).expect("small"))),
        );
        let host = Waits {
            wait_for,
            quick: 0.into(),
            enough: Notify::new(),
        };
        let handled = tokio::time::timeout(
            Duration::from_secs(30),
            pump(&host, &client, nothing_between),
        )
        .await
        .expect("the slow command waits only for the quick ones")
        .expect("the pump");
        assert_eq!(handled, wait_for + 1);
    }

    /// A server listing all of its quick commands at once.
    #[derive(Debug, Default)]
    struct Burst {
        fetched: Mutex<Vec<Uuid>>,
    }

    const BURST: u128 = 50;

    impl Transport for Burst {
        fn send(&self, request: HttpRequest) -> TransportFuture<'_> {
            Box::pin(async move {
                if request.path.starts_with("/api/v1/host/commands?") {
                    let fetched = self.fetched.lock().expect("fetched").clone();
                    let commands = (1..=BURST)
                        .map(|n| Uuid::from_u128(1000 + n))
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
                let command_id = request
                    .path
                    .trim_start_matches("/api/v1/host/commands/")
                    .trim_end_matches("/ack")
                    .parse::<Uuid>()
                    .expect("a command id");
                if request.method == "POST" {
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

    #[tokio::test]
    async fn one_long_list_still_stops_for_admission() {
        let server = Arc::new(Burst::default());
        let client = HostClient::with_transport("http://localhost", "token", server.clone());
        let host = Waits {
            wait_for: usize::MAX,
            quick: 0.into(),
            enough: Notify::new(),
        };
        let seen_at_admission = Mutex::new(Vec::new());
        let between = || async {
            seen_at_admission
                .lock()
                .expect("seen")
                .push(server.fetched.lock().expect("fetched").len());
            Ok::<_, RuntimeError>(())
        };
        let handled = tokio::time::timeout(Duration::from_secs(30), pump(&host, &client, between))
            .await
            .expect("the pass ends")
            .expect("the pump");
        assert_eq!(handled, usize::try_from(BURST).expect("small"));
        let seen = seen_at_admission.lock().expect("seen").clone();
        assert!(!seen.is_empty());
        let mut before = 0;
        for at in seen {
            assert!(
                at - before <= PER_PASS,
                "{} started between admissions",
                at - before
            );
            before = at;
        }
    }

    /// Each command holds the one database connection for a moment.
    struct OneConnection {
        connection: tokio::sync::Semaphore,
    }

    impl CommandHost for OneConnection {
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
        async fn action(&self, _: &Command) -> Ack {
            let _held = self.connection.acquire().await.expect("the connection");
            // Longer than the pump waits before looking again: others queue.
            tokio::time::sleep(LOOK_AGAIN * 2).await;
            let mut ack = Ack::new(AckOutcome::Accepted);
            ack.result = Some(json!({}));
            ack
        }
    }

    #[tokio::test]
    async fn admission_gets_the_connection_a_command_in_flight_holds() {
        let client =
            HostClient::with_transport("http://localhost", "token", Arc::new(Queue::new(false, 8)));
        let host = OneConnection {
            connection: tokio::sync::Semaphore::new(1),
        };
        let admitted = std::sync::atomic::AtomicUsize::new(0);
        let between = || async {
            let _held = tokio::time::timeout(Duration::from_secs(3), host.connection.acquire())
                .await
                .map_err(|_| RuntimeError::Internal("no connection for admission"))?
                .expect("the connection");
            admitted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, RuntimeError>(())
        };
        let handled = tokio::time::timeout(Duration::from_secs(60), pump(&host, &client, between))
            .await
            .expect("the pass ends")
            .expect("admission waits only for a command to let the connection go");
        assert_eq!(handled, 8);
        assert!(admitted.load(std::sync::atomic::Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn nothing_new_is_fetched_once_admission_fails() {
        let queue = Arc::new(Queue::new(false, u128::MAX));
        let client = HostClient::with_transport("http://localhost", "token", queue.clone());
        let host = Waits {
            wait_for: usize::MAX,
            quick: 0.into(),
            enough: Notify::new(),
        };
        let listed_when_stopped = Mutex::new(None);
        let between = || async {
            *listed_when_stopped.lock().expect("listed") =
                Some(*queue.listed.lock().expect("listed"));
            Err(RuntimeError::Internal("the lease was lost"))
        };
        let ended = tokio::time::timeout(Duration::from_secs(30), pump(&host, &client, between))
            .await
            .expect("the pass ends once admission says so");
        assert!(ended.is_err());
        let stopped_at = listed_when_stopped
            .lock()
            .expect("listed")
            .expect("admission ran");
        assert_eq!(*queue.listed.lock().expect("listed"), stopped_at);
        assert_eq!(
            host.quick.load(std::sync::atomic::Ordering::SeqCst),
            usize::try_from(stopped_at).expect("small")
        );
    }

    #[tokio::test]
    async fn admission_gets_its_turn_under_steady_traffic_and_can_end_the_pass() {
        let client = HostClient::with_transport(
            "http://localhost",
            "token",
            Arc::new(Queue::new(false, u128::MAX)),
        );
        let host = Waits {
            wait_for: usize::MAX,
            quick: 0.into(),
            enough: Notify::new(),
        };
        let turns = std::sync::atomic::AtomicUsize::new(0);
        let between = || async {
            if turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 >= 3 {
                Err(RuntimeError::Internal("the lease was lost"))
            } else {
                Ok(())
            }
        };
        let ended = tokio::time::timeout(Duration::from_secs(30), pump(&host, &client, between))
            .await
            .expect("the pass ends once admission says so");
        assert!(ended.is_err());
        assert_eq!(turns.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
