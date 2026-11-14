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

/// How text that is a credential usually starts.
const SECRET_PREFIXES: &[&str] = &[
    "-----BEGIN",
    "Bearer ",
    "Basic ",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
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

fn private_field(name: &str) -> bool {
    let folded = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    PRIVATE_FIELDS.contains(&folded.as_str())
}

/// One run of 32 or more letters and digits mixing upper case, lower case
/// and digits, as generated tokens do (ids are lower-case hex and pass).
fn looks_generated(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .any(|run| {
            run.len() >= 32
                && run.chars().any(|c| c.is_ascii_uppercase())
                && run.chars().any(|c| c.is_ascii_lowercase())
                && run.chars().any(|c| c.is_ascii_digit())
        })
}

fn secret_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    SECRET_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
        || looks_generated(text)
        || url_with_credentials(text)
}

/// A URL with a user, password or a query (webhook URLs often carry their
/// secret in the query).
fn url_with_credentials(text: &str) -> bool {
    let Some(rest) = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    authority.contains('@') || rest.contains('?')
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
