//! Handing a secret to a harness process without its environment or
//! argument list (H6, H7): the value goes in an anonymous in-memory file the
//! child sees as file descriptor 3. Nothing is written to disk, nothing shows
//! in `/proc/<pid>/environ` or `ps`, and the child (or a helper it starts,
//! such as Claude Code's `apiKeyHelper`) can read it as often as it needs.

use std::{io::Write, os::fd::OwnedFd};

use command_fds::{CommandFdExt, FdMapping};
use zeroize::Zeroizing;

use super::adapter::HarnessError;

/// The descriptor a harness reads its secrets from.
pub const SECRET_FD: i32 = 3;

/// An anonymous file holding `bytes`, positioned at its start.
pub fn secret_file(bytes: &[u8]) -> Result<OwnedFd, HarnessError> {
    let fd = anonymous_file()?;
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes).map_err(transport)?;
    file.flush().map_err(transport)?;
    rustix::fs::seek(&file, rustix::fs::SeekFrom::Start(0)).map_err(transport)?;
    Ok(OwnedFd::from(file))
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
fn anonymous_file() -> Result<OwnedFd, HarnessError> {
    rustix::fs::memfd_create("agent-workspace-secret", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(transport)
}

/// Elsewhere (macOS development), an unlinked private file: it has no name
/// on disk once this returns.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
fn anonymous_file() -> Result<OwnedFd, HarnessError> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!("aw-secret-{}", uuid::Uuid::now_v7()));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(transport)?;
    std::fs::remove_file(&path).map_err(transport)?;
    Ok(OwnedFd::from(file))
}

/// Gives the child `fd` as descriptor 3.
pub fn attach(command: &mut tokio::process::Command, fd: OwnedFd) -> Result<(), HarnessError> {
    command
        .fd_mappings(vec![FdMapping {
            parent_fd: fd,
            child_fd: SECRET_FD,
        }])
        .map(|_| ())
        .map_err(|_| HarnessError::InvalidConfiguration)
}

/// A JSON object of secret names to values, for a command-line harness.
pub fn secrets_json(secrets: &[(String, crate::secrets::SecretValue)]) -> Zeroizing<Vec<u8>> {
    let map = secrets
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                serde_json::Value::String(value.expose().to_owned()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let mut bytes = Zeroizing::new(Vec::new());
    // Serializing a map of strings can't fail.
    let _ = serde_json::to_writer(&mut *bytes, &map);
    bytes
}

fn transport(error: impl std::error::Error + Send + Sync + 'static) -> HarnessError {
    HarnessError::Transport(Box::new(error))
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn the_secret_file_reads_back_from_the_start() {
        let fd = secret_file(b"value-123").expect("secret file");
        let mut file = std::fs::File::from(fd);
        let mut text = String::new();
        file.read_to_string(&mut text).expect("read");
        assert_eq!(text, "value-123");
    }

    #[tokio::test]
    async fn a_child_reads_the_secret_on_fd_3_and_not_from_its_environment() {
        let fd = secret_file(b"top-secret-value").expect("secret file");
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("cat /dev/fd/3; echo; cat /dev/fd/3; echo; env")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdout(std::process::Stdio::piped());
        attach(&mut command, fd).expect("attach");
        let output = command.output().await.expect("run");
        let text = String::from_utf8(output.stdout).expect("utf8");
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("top-secret-value"));
        assert_eq!(lines.next(), Some("top-secret-value"), "readable twice");
        assert!(lines.all(|line| !line.contains("top-secret-value")));
    }
}
