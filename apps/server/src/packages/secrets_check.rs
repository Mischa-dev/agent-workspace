//! The no-secrets check (M1 section 3.6): a package never holds a secret,
//! credential, token, signing secret or member list. Export builds parts
//! from their portable forms, which leave these out; this check is the net
//! under that, run over the whole document before it leaves the server.

use serde_json::Value;

/// Field names that only ever hold something private.
const PRIVATE_FIELDS: &[&str] = &[
    "token",
    "tokenhint",
    "tokendigest",
    "secret",
    "secretid",
    "signingsecret",
    "password",
    "passphrase",
    "apikey",
    "accesskey",
    "privatekey",
    "credential",
    "credentials",
    "authorization",
    "cookie",
    "members",
    "memberids",
    "email",
    "emails",
];

/// How a credential usually starts, wherever it stands in the text: a word
/// beginning with one of these and long enough to be a key.
const SECRET_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "AKIA",
    "awp_",
    "awt_",
];

/// Characters after a prefix that make a word a key rather than a name.
const MIN_KEY_TAIL: usize = 16;

/// Words that introduce a credential in a header value.
const SCHEMES: &[&str] = &["bearer", "basic"];

fn private_field(name: &str) -> bool {
    let folded = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    PRIVATE_FIELDS.contains(&folded.as_str())
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// One run of 32 or more letters and digits mixing upper case, lower case
/// and digits, as generated tokens do (ids are lower-case hex and pass).
fn looks_generated(text: &str) -> bool {
    text.split(|c: char| !token_char(c)).any(|run| {
        run.len() >= 32
            && run.chars().any(|c| c.is_ascii_uppercase())
            && run.chars().any(|c| c.is_ascii_lowercase())
            && run.chars().any(|c| c.is_ascii_digit())
    })
}

/// A word anywhere in the text that starts like a known key.
fn prefixed_key(text: &str) -> bool {
    text.split(|c: char| !token_char(c)).any(|run| {
        SECRET_PREFIXES
            .iter()
            .any(|prefix| run.starts_with(prefix) && run.len() >= prefix.len() + MIN_KEY_TAIL)
    })
}

/// "Bearer <credential>" or "Basic <credential>" anywhere, in any case; the
/// credential has letters and digits, not hyphenated words.
fn scheme_credential(text: &str) -> bool {
    let words = text.split_whitespace().collect::<Vec<_>>();
    words.windows(2).any(|pair| {
        let scheme = pair[0].trim_end_matches(':').to_ascii_lowercase();
        let value = pair[1].trim_matches(|c: char| !(token_char(c) || "+/=.".contains(c)));
        SCHEMES.contains(&scheme.as_str())
            && value.len() >= MIN_KEY_TAIL
            && value.chars().all(|c| token_char(c) || "+/=.".contains(c))
            && value.chars().any(|c| c.is_ascii_digit())
            && value.chars().any(|c| c.is_ascii_alphabetic())
            && value.matches('-').count() < 2
    })
}

fn secret_text(text: &str) -> bool {
    text.contains("-----BEGIN")
        || prefixed_key(text)
        || scheme_credential(text)
        || looks_generated(text)
        || urls(text).any(url_with_credentials)
}

/// Every http(s) URL in the text, up to where it ends.
fn urls(text: &str) -> impl Iterator<Item = &str> {
    let lower = text.to_ascii_lowercase();
    let starts = lower
        .match_indices("http")
        .map(|(at, _)| at)
        .filter(|&at| lower[at..].starts_with("http://") || lower[at..].starts_with("https://"))
        .collect::<Vec<_>>();
    starts.into_iter().map(move |at| {
        let rest = &text[at..];
        let end = rest
            .find(|c: char| c.is_whitespace() || "\"'<>`".contains(c))
            .unwrap_or(rest.len());
        &rest[..end]
    })
}

/// A URL with a user, password, a query or a fragment carrying a value
/// (webhook URLs often carry their secret there).
fn url_with_credentials(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let fragment_value = rest
        .split_once('#')
        .is_some_and(|(_, fragment)| fragment.contains('='));
    authority.contains('@') || rest.contains('?') || fragment_value
}

/// Where in the document something private was found, as JSON paths.
pub fn check(document: &Value) -> Result<(), Vec<String>> {
    fn walk(value: &Value, path: &str, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    let at = format!("{path}.{key}");
                    let empty = match item {
                        Value::Null => true,
                        Value::String(text) => text.is_empty(),
                        Value::Array(items) => items.is_empty(),
                        _ => false,
                    };
                    if private_field(key) && !empty {
                        found.push(at.clone());
                    }
                    walk(item, &at, found);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{index}]"), found);
                }
            }
            Value::String(text) if secret_text(text) => found.push(path.to_owned()),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(document, "$", &mut found);
    if found.is_empty() { Ok(()) } else { Err(found) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::check;

    #[test]
    fn private_fields_and_credential_text_are_found() {
        let document = json!({
            "contents": [
                {"body": {"name": "Feedback", "token": "abc"}},
                {"body": {"url": "https://hooks.example.org/x?key=1"}},
                {"body": {"note": "ghp_0123456789abcdefABCDEF0123456789abcd"}},
                {"body": {"note": "Use aB3dE5gH7jK9mN1pQ3sT5vX7zA9cE1gI3k here"}},
                {"body": {"signingSecret": "s"}},
                {"body": {"members": ["mara"]}},
            ]
        });
        let found = check(&document).expect_err("found");
        assert_eq!(found.len(), 6, "{found:?}");
    }

    #[test]
    fn credentials_inside_text_are_found() {
        for text in [
            "Use ghp_0123456789abcdefABCDEF0123456789abcd here",
            "Webhook: https://hooks.example.org/run?token=abc123",
            "Post to (HTTPS://hooks.example.org/run?key=1) when done",
            "Sign in at https://user:pw@example.org/",
            "Callback https://example.org/cb#access_token=abc",
            "Header: Authorization: Bearer abcdefghijklmnop0123",
            "Key: AKIAABCDEFGHIJKLMNOP",
            "slack xoxb-1234567890-abcdefghijkl",
            "-----BEGIN OPENSSH PRIVATE KEY----- in the middle",
        ] {
            let document = json!({"contents": [{"body": {"note": text}}]});
            assert!(check(&document).is_err(), "{text}");
        }
    }

    #[test]
    fn words_that_only_look_alike_pass() {
        for text in [
            "Ask the sk-team about basic setup",
            "Bearer of bad news",
            "Basic onboarding-checklist-for-2024 steps",
            "See https://example.org/docs/page#setup and http://example.org.",
            "The task-key AWD-12 and ghp_ prefixes",
        ] {
            let document = json!({"contents": [{"body": {"note": text}}]});
            assert_eq!(check(&document), Ok(()), "{text}");
        }
    }

    #[test]
    fn ordinary_setup_passes() {
        let document = json!({
            "id": "studio.game_studio",
            "contents": [
                {"body": {
                    "name": "Feedback",
                    "id": "0190f5a2-7b1c-7d3e-8f00-123456789abc",
                    "url": "https://example.org/docs/page",
                    "members": [],
                    "text": "Release notes for {{task.key}}",
                }},
            ],
        });
        assert_eq!(check(&document), Ok(()));
    }
}
