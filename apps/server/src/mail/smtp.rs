//! SMTP, the first mail provider: a small client for submitting one message
//! to the operator's mail server (their provider's submission port, or a
//! relay on this host). TLS is required unless the server is on this
//! machine; credentials are only ever sent inside TLS or to a loopback
//! relay. Nothing here logs a reply, a credential or a message.

use std::{net::IpAddr, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, pki_types::ServerName},
};

use super::MailError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest reply we read; a well-behaved server's are far shorter.
const MAX_REPLY_BYTES: usize = 16 * 1024;

/// How the connection is protected.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    /// Plain connection upgraded with STARTTLS (usually port 587). The
    /// upgrade is required: a server that doesn't offer it is refused.
    Starttls,
    /// TLS from the first byte (usually port 465).
    Tls,
    /// No TLS; only for a relay on this machine.
    None,
}

/// The provider's settings (stored as `provider_config`). The password is
/// a workspace secret, passed separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: Option<String>,
}

impl Settings {
    /// Check the shape of what an owner typed.
    pub fn validate(&self) -> Result<(), &'static str> {
        let host = self.host.trim();
        if host.is_empty()
            || host.len() > 253
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        {
            return Err("host");
        }
        if self.port == 0 {
            return Err("port");
        }
        if self.security == Security::None && !is_loopback(host) {
            return Err("security");
        }
        if let Some(username) = &self.username
            && (username.is_empty()
                || username.len() > 254
                || username.chars().any(char::is_control))
        {
            return Err("username");
        }
        Ok(())
    }
}

fn is_loopback(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// One message to submit.
pub struct Envelope<'a> {
    pub from: &'a str,
    pub to: &'a str,
    /// The whole RFC 5322 message, lines ending in CRLF.
    pub data: &'a [u8],
    /// The name we greet the server with.
    pub hello: &'a str,
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

struct Connection {
    stream: Box<dyn Stream>,
    buffer: Vec<u8>,
}

/// A server reply: its code and text (lines joined with spaces).
#[derive(Debug)]
struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    fn text(&self) -> String {
        let joined = self.lines.join(" ");
        let cleaned: String = joined
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        format!("{} {}", self.code, cleaned.trim())
    }

    fn positive(&self) -> bool {
        (200..400).contains(&self.code)
    }

    fn transient(&self) -> bool {
        (400..500).contains(&self.code)
    }
}

impl Connection {
    async fn read_line(&mut self) -> Result<String, MailError> {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|pair| pair == b"\r\n") {
                let line: Vec<u8> = self.buffer.drain(..end + 2).collect();
                return Ok(String::from_utf8_lossy(&line[..end]).into_owned());
            }
            if self.buffer.len() > MAX_REPLY_BYTES {
                return Err(MailError::Permanent(
                    "the mail server sent a reply that was too long".into(),
                ));
            }
            let mut chunk = [0_u8; 1024];
            let read = timeout(STEP_TIMEOUT, self.stream.read(&mut chunk))
                .await
                .map_err(|_| MailError::Retry("the mail server stopped answering".into()))?
                .map_err(|_| MailError::Retry("the connection to the mail server broke".into()))?;
            if read == 0 {
                return Err(MailError::Retry(
                    "the mail server closed the connection".into(),
                ));
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    async fn reply(&mut self) -> Result<Reply, MailError> {
        let mut lines = Vec::new();
        let mut total = 0;
        loop {
            let line = self.read_line().await?;
            total += line.len();
            if total > MAX_REPLY_BYTES || line.len() < 3 {
                return Err(MailError::Permanent(
                    "the mail server's reply didn't make sense".into(),
                ));
            }
            // `get`, not slicing: a reply with a multi-byte character in
            // its first three bytes must not panic.
            let code = line
                .get(..3)
                .and_then(|code| code.parse::<u16>().ok())
                .ok_or_else(|| {
                    MailError::Permanent("the mail server's reply didn't make sense".into())
                })?;
            let more = line.as_bytes().get(3) == Some(&b'-');
            lines.push(line.get(4..).unwrap_or_default().to_owned());
            if !more {
                return Ok(Reply { code, lines });
            }
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), MailError> {
        timeout(STEP_TIMEOUT, async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| MailError::Retry("the mail server stopped answering".into()))?
        .map_err(|_| MailError::Retry("the connection to the mail server broke".into()))
    }

    /// Send a command and read the reply.
    async fn command(&mut self, line: &str) -> Result<Reply, MailError> {
        self.write(format!("{line}\r\n").as_bytes()).await?;
        self.reply().await
    }

    /// Send a command that must succeed; `what` names the step for errors.
    async fn expect(&mut self, line: &str, what: &str) -> Result<Reply, MailError> {
        let reply = self.command(line).await?;
        check(reply, what)
    }
}

fn check(reply: Reply, what: &str) -> Result<Reply, MailError> {
    if reply.positive() {
        Ok(reply)
    } else if reply.transient() {
        Err(MailError::Retry(format!(
            "the mail server refused {what} for now ({})",
            reply.text()
        )))
    } else {
        Err(MailError::Permanent(format!(
            "the mail server refused {what} ({})",
            reply.text()
        )))
    }
}

fn tls_connector() -> TlsConnector {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map(|builder| builder.with_root_certificates(roots).with_no_client_auth());
    // The ring provider supports the safe defaults, so this can't fail;
    // fall back to an empty config only to stay panic-free.
    TlsConnector::from(Arc::new(config.unwrap_or_else(|_| {
        ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth()
    })))
}

async fn upgrade(stream: Box<dyn Stream>, host: &str) -> Result<Box<dyn Stream>, MailError> {
    let name = ServerName::try_from(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
    )
    .map_err(|_| MailError::Permanent("the mail server's name isn't valid".into()))?;
    let tls = timeout(STEP_TIMEOUT, tls_connector().connect(name, stream))
        .await
        .map_err(|_| MailError::Retry("the mail server didn't finish TLS".into()))?
        .map_err(|_| {
            MailError::Permanent(
                "TLS with the mail server failed (check the host name, port and security)".into(),
            )
        })?;
    Ok(Box::new(tls))
}

/// The EHLO extensions, upper case.
fn extensions(reply: &Reply) -> Vec<String> {
    reply
        .lines
        .iter()
        .skip(1)
        .map(|line| line.trim().to_ascii_uppercase())
        .collect()
}

fn auth_mechanisms(extensions: &[String]) -> Vec<String> {
    extensions
        .iter()
        .filter_map(|line| {
            line.strip_prefix("AUTH ")
                .or_else(|| line.strip_prefix("AUTH="))
        })
        .flat_map(|mechanisms| mechanisms.split_whitespace().map(str::to_owned))
        .collect()
}

/// `.` at the start of a line is doubled (RFC 5321 4.5.2), and the data
/// ends with CRLF.CRLF.
pub fn dot_stuff(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len() + 16);
    let mut line_start = true;
    for &byte in data {
        if line_start && byte == b'.' {
            output.push(b'.');
        }
        output.push(byte);
        line_start = byte == b'\n';
    }
    if !output.ends_with(b"\r\n") {
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(b".\r\n");
    output
}

/// Submit one message.
pub async fn send(
    settings: &Settings,
    password: Option<&str>,
    envelope: &Envelope<'_>,
) -> Result<(), MailError> {
    settings
        .validate()
        .map_err(|_| MailError::Permanent("the mail settings aren't valid".into()))?;
    let host = settings.host.trim();
    let address = (
        host.trim_start_matches('[').trim_end_matches(']'),
        settings.port,
    );
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| MailError::Retry("couldn't reach the mail server in time".into()))?
        .map_err(|_| {
            MailError::Retry("couldn't connect to the mail server (check the host and port)".into())
        })?;
    let stream: Box<dyn Stream> = Box::new(tcp);
    let stream = if settings.security == Security::Tls {
        upgrade(stream, host).await?
    } else {
        stream
    };
    let mut connection = Connection {
        stream,
        buffer: Vec::new(),
    };
    check(connection.reply().await?, "the connection")?;
    let hello = format!("EHLO {}", envelope.hello);
    let mut greeting = connection.expect(&hello, "the greeting").await?;
    if settings.security == Security::Starttls {
        if !extensions(&greeting).iter().any(|line| line == "STARTTLS") {
            return Err(MailError::Permanent(
                "the mail server doesn't offer STARTTLS; choose TLS or another port".into(),
            ));
        }
        connection.expect("STARTTLS", "STARTTLS").await?;
        // Anything the server sent before TLS could be an injected reply.
        if !connection.buffer.is_empty() {
            return Err(MailError::Permanent(
                "the mail server sent data before TLS started".into(),
            ));
        }
        let Connection { stream, .. } = connection;
        connection = Connection {
            stream: upgrade(stream, host).await?,
            buffer: Vec::new(),
        };
        greeting = connection.expect(&hello, "the greeting").await?;
    }
    if let Some(username) = &settings.username {
        let password = password.unwrap_or_default();
        let mechanisms = auth_mechanisms(&extensions(&greeting));
        if mechanisms.iter().any(|mechanism| mechanism == "PLAIN") {
            let token = zeroize::Zeroizing::new(
                STANDARD.encode(format!("\0{username}\0{password}").as_bytes()),
            );
            let reply = connection
                .command(&format!("AUTH PLAIN {}", token.as_str()))
                .await?;
            check_auth(reply)?;
        } else if mechanisms.iter().any(|mechanism| mechanism == "LOGIN") {
            let reply = connection.command("AUTH LOGIN").await?;
            if reply.code != 334 {
                return check_auth(reply).map(|_| ());
            }
            let reply = connection.command(&STANDARD.encode(username)).await?;
            if reply.code != 334 {
                return check_auth(reply).map(|_| ());
            }
            let encoded = zeroize::Zeroizing::new(STANDARD.encode(password));
            check_auth(connection.command(encoded.as_str()).await?)?;
        } else {
            return Err(MailError::Permanent(
                "the mail server doesn't offer a sign-in this app supports (PLAIN or LOGIN)".into(),
            ));
        }
    }
    connection
        .expect(&format!("MAIL FROM:<{}>", envelope.from), "the sender")
        .await?;
    connection
        .expect(&format!("RCPT TO:<{}>", envelope.to), "the recipient")
        .await?;
    let reply = connection.command("DATA").await?;
    if reply.code != 354 {
        return check(reply, "the message").map(|_| ());
    }
    connection.write(&dot_stuff(envelope.data)).await?;
    check(connection.reply().await?, "the message")?;
    // The message is accepted; a failed goodbye changes nothing.
    let _ = connection.command("QUIT").await;
    Ok(())
}

fn check_auth(reply: Reply) -> Result<Reply, MailError> {
    if reply.positive() {
        Ok(reply)
    } else if reply.transient() {
        Err(MailError::Retry(format!(
            "the mail server couldn't sign in right now ({})",
            reply.text()
        )))
    } else {
        Err(MailError::Permanent(format!(
            "the mail server refused the username or password ({})",
            reply.text()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_at_line_starts_are_doubled_and_data_is_terminated() {
        assert_eq!(dot_stuff(b"a\r\n.b\r\n..c"), b"a\r\n..b\r\n...c\r\n.\r\n");
        assert_eq!(dot_stuff(b".\r\n"), b"..\r\n.\r\n");
    }

    #[test]
    fn plain_connections_are_only_for_this_machine() {
        let mut settings = Settings {
            host: "smtp.example.com".into(),
            port: 587,
            security: Security::Starttls,
            username: Some("me".into()),
        };
        assert!(settings.validate().is_ok());
        settings.security = Security::None;
        assert_eq!(settings.validate(), Err("security"));
        for host in ["127.0.0.1", "localhost", "[::1]"] {
            settings.host = host.into();
            assert!(settings.validate().is_ok(), "{host}");
        }
        settings.host = "bad host\r\n".into();
        assert_eq!(settings.validate(), Err("host"));
    }

    #[test]
    fn reads_auth_mechanisms_from_either_spelling() {
        let reply = Reply {
            code: 250,
            lines: vec![
                "mail.example".into(),
                "AUTH LOGIN PLAIN".into(),
                "AUTH=XOAUTH2".into(),
                "STARTTLS".into(),
            ],
        };
        let found = auth_mechanisms(&extensions(&reply));
        assert_eq!(found, vec!["LOGIN", "PLAIN", "XOAUTH2"]);
    }
}
