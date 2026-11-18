//! Text a run reports can carry anything a command printed, keys included:
//! [`redact`] removes what looks like a secret, and [`cap`] keeps the start
//! and end of long text. [`SecretValue`] is a secret's plaintext, which is
//! never printed and is wiped when dropped. The server and the built-in host
//! both use these, so they agree on what a secret looks like.

use std::fmt;

use serde_json::Value;
use zeroize::Zeroizing;

/// A secret's plaintext. Its `Debug` is redacted, it isn't `Serialize`, and
/// its memory is wiped when dropped.
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// The plaintext, for the one call that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The last four characters people see, or nothing for short values
    /// (where four characters would give too much away).
    pub fn last_four(&self) -> String {
        let count = self.0.chars().count();
        if count < 12 {
            return String::new();
        }
        self.0.chars().skip(count - 4).collect()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

pub const MAX_OUTPUT_CHARS: usize = 8_000;
/// The most one note keeps.
pub const MAX_NOTE_CHARS: usize = 4_000;
/// The most one run event payload may hold (the database checks it too).
pub const MAX_PAYLOAD_BYTES: usize = 48 * 1024;
/// The most changed files one step keeps.
pub const MAX_FILES_PER_STEP: usize = 200;
/// The most of one command line a step keeps.
pub const MAX_COMMAND_CHARS: usize = 2_000;
const REDACTED: &str = "[redacted]";

// ---------------------------------------------------------------------------
// Redaction

/// Words that, followed by `=` or `:`, introduce a secret value.
const SECRET_KEYS: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "private_key",
];

/// Object keys whose value is a secret, compared lower-cased with `_`,
/// `-` and other separators removed, by how the key ends: `password`,
/// `client_secret`, `X-Api-Key`, `accessToken`, `Set-Cookie`. Counts such
/// as `inputTokens` don't end in one and are kept.
const SECRET_FIELD_ENDINGS: [&str; 16] = [
    "password",
    "passwords",
    "passwd",
    "secret",
    "secrets",
    "token",
    "apikey",
    "accesskey",
    "privatekey",
    "authorization",
    "cookie",
    "cookies",
    "credential",
    "credentials",
    "sessionkey",
    "signingkey",
];

/// Whether a JSON object key names a secret, so its whole value goes.
pub fn secret_field(key: &str) -> bool {
    let compact = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    SECRET_FIELD_ENDINGS
        .iter()
        .any(|ending| compact.ends_with(ending))
}

/// What a redacted value is stored as.
pub fn redacted() -> Value {
    Value::String(REDACTED.to_owned())
}

/// Prefixes of well-known credential formats; the rest of the word goes.
const SECRET_PREFIXES: [(&str, usize); 12] = [
    ("sk-", 16),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("github_pat_", 20),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("glpat-", 16),
    ("AKIA", 16),
    ("AIza", 30),
];

fn token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/' | '+' | '=')
}

/// Removes credentials from text a harness reported: private key blocks,
/// bearer tokens, JSON web tokens, well-known key formats and `key=value`
/// pairs whose key names a secret. Over-redacting is fine; this text is
/// for people reviewing a run, not for replaying it.
pub fn redact(text: &str) -> String {
    let text = redact_private_keys(text);
    let mut out = String::with_capacity(text.len());
    let characters = text.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let (offset, character) = characters[index];
        let rest = &text[offset..];
        let at_word_start = index == 0 || !token_char(characters[index - 1].1);
        if at_word_start {
            // Well-known formats.
            if let Some((prefix, minimum)) = SECRET_PREFIXES
                .iter()
                .find(|(prefix, _)| rest.starts_with(prefix))
            {
                let length = rest.chars().take_while(|c| token_char(*c)).count();
                if length >= prefix.len() + minimum {
                    out.push_str(REDACTED);
                    index += length;
                    continue;
                }
            }
            // A JSON web token: three base64url parts, the first `eyJ`.
            if rest.starts_with("eyJ") {
                let word = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    .collect::<String>();
                if word.split('.').filter(|part| !part.is_empty()).count() >= 3 && word.len() > 30 {
                    out.push_str(REDACTED);
                    index += word.chars().count();
                    continue;
                }
            }
            // `Bearer <token>` and `Basic <token>`.
            for scheme in ["Bearer ", "bearer ", "Basic ", "basic "] {
                if let Some(after) = rest.strip_prefix(scheme) {
                    let length = after.chars().take_while(|c| token_char(*c)).count();
                    if length >= 8 {
                        out.push_str(scheme);
                        out.push_str(REDACTED);
                        index += scheme.chars().count() + length;
                        break;
                    }
                }
            }
            if index < characters.len() && characters[index].0 != offset {
                continue;
            }
            // `token=…`, `"password": "…"`, `API_KEY: …`, and on a command
            // line `--token …` and `PASSWORD='with spaces'`.
            let lower = rest
                .chars()
                .take(32)
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some(key_length) = secret_key_length(&lower) {
                let key = rest.chars().take(key_length).collect::<String>();
                let after = &rest[key.len()..];
                let separator = after
                    .chars()
                    .take_while(|c| matches!(c, '"' | '\'' | ' ' | ':' | '='))
                    .collect::<String>();
                // A flag (`--token abc`) takes its value after a space.
                let flag_value = key.starts_with('-')
                    && separator.starts_with(' ')
                    && !separator.contains(['=', ':']);
                if separator.contains(['=', ':']) || flag_value {
                    // A quoted value runs to its closing quote, spaces and
                    // all; a bare one to the next space or delimiter.
                    let quote = separator.chars().last().filter(|c| matches!(c, '"' | '\''));
                    let value = after[separator.len()..]
                        .chars()
                        .take_while(|c| match quote {
                            Some(quote) => *c != quote,
                            None => !c.is_whitespace() && !matches!(c, '"' | '\'' | ',' | '}'),
                        })
                        .collect::<String>();
                    if value.len() >= 4 {
                        out.push_str(&key);
                        out.push_str(&separator);
                        out.push_str(REDACTED);
                        index +=
                            key.chars().count() + separator.chars().count() + value.chars().count();
                        continue;
                    }
                }
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// The length of a secret-naming key at the start of `lower` (already
/// lowercased), allowing a prefix such as `github_` or `x-api-`.
fn secret_key_length(lower: &str) -> Option<usize> {
    let word = lower
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect::<String>();
    SECRET_KEYS
        .iter()
        .any(|key| word.ends_with(key))
        .then_some(word.chars().count())
}

fn redact_private_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let header_end = rest[start..]
            .find("-----\n")
            .or_else(|| rest[start + 11..].find("-----").map(|end| end + 11));
        let Some(header_end) = header_end else { break };
        let header = &rest[start..start + header_end];
        if !header.contains("PRIVATE KEY") {
            out.push_str(&rest[..start + header_end]);
            rest = &rest[start + header_end..];
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str("[redacted private key]");
        match rest[start..].find("-----END ") {
            Some(end) => {
                let tail = &rest[start + end + 9..];
                let close = tail.find("-----").map_or(tail.len(), |close| close + 5);
                rest = &tail[close..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Keeps the start and end of long text, with a note of what was cut.
pub fn cap(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let head = max_chars / 4;
    let tail = max_chars - head;
    let start = text.chars().take(head).collect::<String>();
    let end = text.chars().skip(count - tail).collect::<String>();
    format!(
        "{start}\n… {} characters left out …\n{end}",
        count - head - tail
    )
}

/// Text kept in a log: control characters other than newlines and tabs
/// gone, secrets redacted, capped at `max_chars`.
pub fn clean(text: &str, max_chars: usize) -> String {
    // Control characters other than newlines and tabs only garble a log.
    let text = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect::<String>();
    cap(&redact(&text), max_chars)
}
