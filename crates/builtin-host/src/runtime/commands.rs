//! A1.16, A1.17: the built-in host reads its commands through the public
//! host protocol, like any host (listed, fetched, acknowledged), and
//! carries each out. What carrying out means for a start, a stop, an
//! answer or an inspection is the host's own work, behind [`CommandHost`].

use std::future::Future;

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

/// Carry out every command waiting for this host. Returns how many.
pub async fn pump<H: CommandHost>(host: &H, client: &HostClient) -> Result<usize, RuntimeError> {
    let mut handled = 0;
    loop {
        let listed = client
            .commands(0, 0)
            .await
            .map_err(|_| RuntimeError::Internal("read the built-in host's commands"))?;
        if listed.commands.is_empty() {
            return Ok(handled);
        }
        for summary in listed.commands {
            let Ok(command) = client.command(summary.command_id).await else {
                // Closed since it was listed: nothing to do.
                continue;
            };
            let ack = carry_out(host, client, &command).await;
            let _ = client.ack(command.command_id, &ack).await;
            handled += 1;
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
pub async fn carry_out_one<H: CommandHost>(
    host: &H,
    client: &HostClient,
    command_id: Uuid,
) -> Result<Ack, RuntimeError> {
    let command = client
        .command(command_id)
        .await
        .map_err(|_| RuntimeError::Internal("read the built-in host's command"))?;
    let ack = carry_out(host, client, &command).await;
    client
        .ack(command.command_id, &ack)
        .await
        .map_err(|_| RuntimeError::Internal("acknowledge the built-in host's command"))?;
    Ok(ack)
}
