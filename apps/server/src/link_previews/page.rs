//! Fetching a page (or its picture) through the guard, and reading its Open
//! Graph tags. The HTML is only scanned for `<meta>` and `<title>`; nothing
//! from it is ever rendered as HTML.

use std::time::Duration;

use url::Url;

use super::guard::Policy;

/// Redirects followed, each checked like the first request.
const MAX_REDIRECTS: usize = 3;
/// The most of a page read; the tags we want are in its head.
const MAX_PAGE_BYTES: usize = 512 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// The whole fetch, redirects included.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

const MAX_TITLE_CHARS: usize = 300;
const MAX_DESCRIPTION_CHARS: usize = 600;
const MAX_SITE_NAME_CHARS: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Page {
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) site_name: Option<String>,
    pub(crate) image_url: Option<String>,
}

pub(crate) async fn fetch(policy: &Policy, url: &str) -> Result<Page, &'static str> {
    let (final_url, bytes) = get(
        policy,
        url,
        "text/html,application/xhtml+xml",
        MAX_PAGE_BYTES,
        |content_type| {
            content_type.starts_with("text/html")
                || content_type.starts_with("application/xhtml+xml")
        },
        true,
    )
    .await?;
    let html = String::from_utf8_lossy(&bytes);
    parse(&html, &final_url).ok_or("the page has no title")
}

pub(crate) async fn fetch_image(policy: &Policy, url: &str) -> Result<Vec<u8>, &'static str> {
    let (_, bytes) = get(
        policy,
        url,
        "image/png,image/jpeg,image/gif,image/webp",
        MAX_IMAGE_BYTES,
        |content_type| content_type.starts_with("image/"),
        false,
    )
    .await?;
    Ok(bytes)
}

async fn get(
    policy: &Policy,
    url: &str,
    accept: &'static str,
    max_bytes: usize,
    wanted: fn(&str) -> bool,
    keep_partial: bool,
) -> Result<(Url, Vec<u8>), &'static str> {
    tokio::time::timeout(TOTAL_TIMEOUT, async {
        let mut url = Url::parse(url).map_err(|_| "the link isn't a valid address")?;
        for _ in 0..=MAX_REDIRECTS {
            let client = policy.client_for(&url).await?;
            let mut response = client
                .get(url.clone())
                .header(reqwest::header::ACCEPT, accept)
                .send()
                .await
                .map_err(|_| "the page couldn't be reached")?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or("the page redirected nowhere")?;
                url = url
                    .join(location)
                    .map_err(|_| "the page redirected to an invalid address")?;
                continue;
            }
            if !response.status().is_success() {
                return Err("the page answered with an error");
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !wanted(&content_type) {
                return Err("the link isn't a page");
            }
            if response
                .content_length()
                .is_some_and(|length| !keep_partial && length > max_bytes as u64)
            {
                return Err("the file is too large");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "the page stopped sending")?
            {
                if bytes.len() + chunk.len() > max_bytes {
                    if keep_partial {
                        bytes.extend_from_slice(&chunk[..max_bytes - bytes.len()]);
                        break;
                    }
                    return Err("the file is too large");
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok((url, bytes));
        }
        Err("the page redirected too many times")
    })
    .await
    .map_err(|_| "the page took too long")?
}

/// The preview a page's head describes, or None when it has no title.
pub(crate) fn parse(html: &str, base: &Url) -> Option<Page> {
    let lower = html.to_ascii_lowercase();
    let head_end = lower.find("</head").unwrap_or(lower.len());
    let head = &html[..head_end];
    let lower_head = &lower[..head_end];

    let mut og_title = None;
    let mut og_description = None;
    let mut og_site = None;
    let mut og_image = None;
    let mut twitter_title = None;
    let mut twitter_description = None;
    let mut twitter_image = None;
    let mut description = None;
    let mut position = 0;
    while let Some(found) = lower_head[position..].find("<meta") {
        let start = position + found + 5;
        let Some(length) = head[start..].find('>') else {
            break;
        };
        let attributes = attributes(&head[start..start + length]);
        position = start + length;
        let key = attributes
            .iter()
            .find(|(name, _)| name == "property" || name == "name")
            .map(|(_, value)| value.to_ascii_lowercase());
        let content = attributes
            .iter()
            .find(|(name, _)| name == "content")
            .map(|(_, value)| decode_entities(value));
        let (Some(key), Some(content)) = (key, content) else {
            continue;
        };
        let slot = match key.as_str() {
            "og:title" => &mut og_title,
            "og:description" => &mut og_description,
            "og:site_name" => &mut og_site,
            "og:image" | "og:image:url" | "og:image:secure_url" => &mut og_image,
            "twitter:title" => &mut twitter_title,
            "twitter:description" => &mut twitter_description,
            "twitter:image" | "twitter:image:src" => &mut twitter_image,
            "description" => &mut description,
            _ => continue,
        };
        if slot.is_none() && !content.trim().is_empty() {
            *slot = Some(content);
        }
    }
    let title_tag = lower_head.find("<title").and_then(|start| {
        let open_end = start + head[start..].find('>')? + 1;
        let close = open_end + lower_head[open_end..].find("</title")?;
        Some(decode_entities(&head[open_end..close]))
    });

    let title = clean(og_title.or(twitter_title).or(title_tag)?, MAX_TITLE_CHARS)?;
    let description = og_description
        .or(twitter_description)
        .or(description)
        .and_then(|text| clean(text, MAX_DESCRIPTION_CHARS));
    let site_name = og_site.and_then(|text| clean(text, MAX_SITE_NAME_CHARS));
    let image_url = og_image
        .or(twitter_image)
        .and_then(|image| base.join(image.trim()).ok())
        .filter(|image| matches!(image.scheme(), "http" | "https"))
        .map(|image| image.to_string())
        .filter(|image| image.len() <= 2048);
    Some(Page {
        title,
        description,
        site_name,
        image_url,
    })
}

/// The attributes of a tag's inside (`name="value"`, `name='value'`,
/// `name=value`), names lower-cased.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let bytes = tag.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/')
        {
            index += 1;
        }
        if name_start == index {
            index += 1;
            continue;
        }
        let name = tag[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            found.push((name, String::new()));
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = match bytes.get(index) {
            Some(quote @ (b'"' | b'\'')) => {
                let value_start = index + 1;
                let end = tag[value_start..]
                    .find(char::from(*quote))
                    .map_or(tag.len(), |end| value_start + end);
                index = end + 1;
                &tag[value_start..end]
            }
            _ => {
                let value_start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                &tag[value_start..index]
            }
        };
        found.push((name, value.to_owned()));
    }
    found
}

/// Text with HTML character references decoded (the common named ones and
/// numeric ones). Control characters are dropped later by `clean`.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semicolon) = rest.as_bytes()[..rest.len().min(12)]
            .iter()
            .position(|byte| *byte == b';')
        else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semicolon];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "rdquo" => Some('”'),
            "ldquo" => Some('“'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &rest[semicolon + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whitespace collapsed, control characters dropped, cut at `max` characters
/// with an ellipsis. None when nothing is left.
fn clean(text: String, max: usize) -> Option<String> {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= max {
        return Some(collapsed);
    }
    let mut cut = collapsed.chars().take(max - 1).collect::<String>();
    cut.push('…');
    Some(cut)
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    #[test]
    fn open_graph_tags_win_over_the_title() {
        let html = r#"<!doctype html><html><head>
            <title>Plain &amp; simple</title>
            <meta name="description" content="Fallback">
            <meta property="og:title" content="Rich &quot;title&quot;" />
            <meta content='The description' property='og:description'>
            <meta property=og:site_name content=Example>
            <meta property="og:image" content="/img/card.png">
            </head><body><meta property="og:title" content="ignored"></body></html>"#;
        let page = parse(
            html,
            &Url::parse("https://example.com/post/1").expect("test fixture"),
        )
        .expect("test fixture");
        assert_eq!(page.title, "Rich \"title\"");
        assert_eq!(page.description.as_deref(), Some("The description"));
        assert_eq!(page.site_name.as_deref(), Some("Example"));
        assert_eq!(
            page.image_url.as_deref(),
            Some("https://example.com/img/card.png")
        );
    }

    #[test]
    fn a_page_without_a_title_has_no_preview() {
        assert!(
            parse(
                "<html><head></head></html>",
                &Url::parse("https://x.example/").expect("test fixture")
            )
            .is_none()
        );
        let page = parse(
            "<title>\n  Just   a title &#8212; ok\n</title>",
            &Url::parse("https://x.example/").expect("test fixture"),
        )
        .expect("test fixture");
        assert_eq!(page.title, "Just a title — ok");
        assert_eq!(page.image_url, None);
        // Scripts in an image URL never become a link.
        let page = parse(
            r#"<title>t</title><meta property="og:image" content="javascript:alert(1)">"#,
            &Url::parse("https://x.example/").expect("test fixture"),
        )
        .expect("test fixture");
        assert_eq!(page.image_url, None);
    }

    /// A one-shot HTTP server on loopback answering each request with the
    /// next canned response.
    async fn serve(responses: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let address = listener.local_addr().expect("test fixture");
        tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 4096];
                let _ = socket.read(&mut buffer).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn pages_are_fetched_and_redirects_are_checked_again() {
        let html = "<html><head><title>Hello</title></head></html>";
        let base = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            ),
        ])
        .await;
        let page = fetch(&Policy::loopback_for_tests(), &format!("{base}/start"))
            .await
            .expect("page");
        assert_eq!(page.title, "Hello");

        // A redirect to the cloud metadata address is refused before any
        // connection is made, even though the first hop was allowed.
        let base = serve(vec![
            "HTTP/1.1 301 Moved\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
        ])
        .await;
        assert_eq!(
            fetch(&Policy::loopback_for_tests(), &base).await,
            Err("the link points at a private address")
        );

        // The real policy refuses this machine outright.
        assert!(fetch(&Policy::public(), &base).await.is_err());
    }

    #[tokio::test]
    async fn non_pages_and_oversized_images_are_refused() {
        let base = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: 2\r\nConnection: close\r\n\r\nPK"
                .to_owned(),
        ])
        .await;
        assert_eq!(
            fetch(&Policy::loopback_for_tests(), &base).await,
            Err("the link isn't a page")
        );
        let base = serve(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_IMAGE_BYTES + 1
        )])
        .await;
        assert_eq!(
            fetch_image(&Policy::loopback_for_tests(), &base).await,
            Err("the file is too large")
        );
    }
}
