//! K5.12: files in an S3-compatible store, and signed download links.
//!
//! The store here is a small S3 stand-in the test starts: it keeps objects
//! in memory, speaks the parts of the S3 API the server uses (put, get,
//! delete, list, multipart, presigned gets) and refuses requests that
//! aren't signed with the configured key. The signing itself is checked
//! against AWS's published examples in `files::object`'s unit tests.

use std::{
    collections::{BTreeMap, HashMap},
    os::unix::fs::PermissionsExt as _,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    AppState, config::AppConfig, files, provider::SupervisorRegistry, rate_limit::RateLimiters,
    realtime::RealtimeHub, router, secrets,
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::{create_channel, text},
};

const ACCESS_KEY: &str = "TESTKEY";
const BUCKET: &str = "files";

/// Multipart uploads in progress: id to parts by number.
type Uploads = HashMap<String, BTreeMap<u32, Vec<u8>>>;

/// The S3 stand-in's memory.
#[derive(Clone, Default)]
struct Fake {
    objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    uploads: Arc<Mutex<Uploads>>,
    /// Multipart uploads completed.
    completed: Arc<Mutex<u32>>,
}

async fn s3(
    State(fake): State<Fake>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let query: HashMap<String, String> = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect();
    let header_signed = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.starts_with(&format!("AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/"))
        });
    let presigned = query.contains_key("X-Amz-Signature")
        && query
            .get("X-Amz-Credential")
            .is_some_and(|credential| credential.starts_with(&format!("{ACCESS_KEY}/")));
    if !(header_signed || presigned) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if header_signed {
        let declared = headers
            .get("x-amz-content-sha256")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if declared != hex::encode(Sha256::digest(&body)) {
            return (StatusCode::BAD_REQUEST, "XAmzContentSHA256Mismatch").into_response();
        }
    }
    let Some(key) = uri.path().strip_prefix(&format!("/{BUCKET}/")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let key = decode(key);
    match (method, query.get("uploadId"), query.get("partNumber")) {
        (Method::PUT, Some(upload), Some(number)) => {
            let number = number.parse::<u32>().expect("part number");
            match fake.uploads.lock().expect("lock").get_mut(upload) {
                Some(parts) => {
                    parts.insert(number, body.to_vec());
                    ([(header::ETAG, format!("\"part-{number}\""))], "").into_response()
                }
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        (Method::PUT, None, None) => {
            fake.objects
                .lock()
                .expect("lock")
                .insert(key, body.to_vec());
            StatusCode::OK.into_response()
        }
        (Method::POST, None, None) if query.contains_key("uploads") => {
            let id = Uuid::now_v7().simple().to_string();
            fake.uploads
                .lock()
                .expect("value")
                .insert(id.clone(), BTreeMap::new());
            format!(
                "<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
            )
            .into_response()
        }
        (Method::POST, Some(upload), None) => {
            let Some(parts) = fake.uploads.lock().expect("lock").remove(upload) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let listed = String::from_utf8_lossy(&body).matches("<Part>").count();
            assert_eq!(listed, parts.len(), "every part is listed");
            let bytes = parts.into_values().flatten().collect::<Vec<_>>();
            fake.objects.lock().expect("lock").insert(key, bytes);
            *fake.completed.lock().expect("lock") += 1;
            "<CompleteMultipartUploadResult/>".into_response()
        }
        (Method::DELETE, Some(upload), None) => {
            fake.uploads.lock().expect("lock").remove(upload);
            StatusCode::NO_CONTENT.into_response()
        }
        (Method::DELETE, None, None) => {
            fake.objects.lock().expect("lock").remove(&key);
            StatusCode::NO_CONTENT.into_response()
        }
        (Method::GET, None, None) => match fake.objects.lock().expect("lock").get(&key) {
            Some(bytes) => {
                let mut response = bytes.clone().into_response();
                for (parameter, name) in [
                    ("response-content-type", header::CONTENT_TYPE),
                    ("response-content-disposition", header::CONTENT_DISPOSITION),
                ] {
                    if let Some(value) = query.get(parameter) {
                        response
                            .headers_mut()
                            .insert(name, value.parse().expect("header"));
                    }
                }
                response
            }
            None => StatusCode::NOT_FOUND.into_response(),
        },
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

/// Listing a prefix, the one bucket-level request the server makes.
async fn list(State(fake): State<Fake>, uri: Uri) -> Response {
    let prefix = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("prefix="))
        .map(decode)
        .unwrap_or_default();
    let keys = fake
        .objects
        .lock()
        .expect("value")
        .keys()
        .filter(|key| key.starts_with(&prefix))
        .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
        .collect::<String>();
    format!("<ListBucketResult><IsTruncated>false</IsTruncated>{keys}</ListBucketResult>")
        .into_response()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).expect("hex");
            out.push(u8::from_str_radix(hex, 16).expect("hex"));
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).expect("utf-8")
}

async fn start_fake() -> (Fake, String) {
    let fake = Fake::default();
    let app = Router::new()
        .route(&format!("/{BUCKET}/"), axum::routing::get(list))
        .fallback(s3)
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the S3 stand-in");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve the S3 stand-in");
    });
    (fake, format!("http://{address}/"))
}

/// The app on `setup`'s database with this test's configuration, and its
/// state for running jobs directly.
fn app_with(setup: &Setup, values: &[(&str, String)]) -> (Router, AppState) {
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let mut all = BTreeMap::from([("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url)]);
    for (key, value) in values {
        all.insert((*key).to_owned(), value.clone());
    }
    let mut config = AppConfig::from_values(&all).expect("configuration");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let state = AppState {
        supervisors: SupervisorRegistry::new(&config).expect("supervisors"),
        config: Arc::new(config),
        pool: setup.pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
    };
    (router(state.clone()), state)
}

fn secret_file(folder: &std::path::Path, name: &str, value: &str) -> String {
    let path = folder.join(name);
    std::fs::write(&path, value).expect("secret file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("owner only");
    path.to_string_lossy().into_owned()
}

fn keyring(folder: &std::path::Path) -> String {
    let path = folder.join("secrets.keyring");
    secrets::write_new_keyring(&path, &secrets::FileKeyring::generate()).expect("keyring");
    path.to_string_lossy().into_owned()
}

async fn upload(app: &Router, browser: &Browser, uri: &str, body: Body) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("origin", "http://127.0.0.1:3100")
        .header("cookie", &browser.cookie)
        .header("x-csrf-token", &browser.csrf)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(body)
        .expect("upload");
    send(app, request).await
}

async fn get(app: &Router, browser: Option<&Browser>, uri: &str) -> Response {
    let mut request = Request::builder().method("GET").uri(uri);
    if let Some(browser) = browser {
        request = request.header("cookie", &browser.cookie);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn bytes_of(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec()
}

fn chunks(count: usize, size: usize) -> (Body, Vec<u8>) {
    let parts = (0..count)
        .map(|index| {
            let mut part = format!("{index:08}\n").into_bytes();
            part.resize(size, b'a' + (index % 26) as u8);
            part
        })
        .collect::<Vec<_>>();
    let all = parts.concat();
    let body = Body::from_stream(futures_util::stream::iter(
        parts
            .into_iter()
            .map(|part| Ok::<_, std::io::Error>(Bytes::from(part))),
    ));
    (body, all)
}

/// Files go to the object store when it is the primary, stream there
/// (a large one in parts), come back through the app or straight from the
/// store, and are deleted from it when removed.
#[tokio::test]
async fn files_live_in_an_object_store() {
    let Some(setup) = setup("files_object").await else {
        return;
    };
    let (fake, endpoint) = start_fake().await;
    let root = tempfile::tempdir().expect("data root");
    let keys = tempfile::tempdir().expect("keys");
    let (app, state) = app_with(
        &setup,
        &[
            (
                "AGENT_WORKSPACE_DATA_ROOT",
                root.path().to_string_lossy().into_owned(),
            ),
            ("AGENT_WORKSPACE_FILE_STORE", "object".to_owned()),
            ("AGENT_WORKSPACE_S3_ENDPOINT", endpoint),
            ("AGENT_WORKSPACE_S3_BUCKET", BUCKET.to_owned()),
            ("AGENT_WORKSPACE_S3_PATH_STYLE", "true".to_owned()),
            ("AGENT_WORKSPACE_S3_PREFIX", "aw".to_owned()),
            (
                "AGENT_WORKSPACE_S3_ACCESS_KEY_ID_PATH",
                secret_file(keys.path(), "access", ACCESS_KEY),
            ),
            (
                "AGENT_WORKSPACE_S3_SECRET_ACCESS_KEY_PATH",
                secret_file(keys.path(), "secret", "test-secret-key"),
            ),
        ],
    );
    let setup = Setup { app, ..setup };
    let work = create_project(&setup, "WORK", "Work").await;
    let channel = create_channel(&setup, &work, "general").await;
    let owner = &setup.browser;
    let prefix = format!("aw/{}/", setup.organization_id.simple());

    // 18 MiB in 1 MiB chunks: three parts of a multipart upload.
    let (body, sent) = chunks(18, 1024 * 1024);
    let (status, big) = upload(
        &setup.app,
        owner,
        &format!("/api/v1/conversations/{channel}/files?name=big.txt"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{big}");
    let big = big["id"].as_str().expect("file").to_owned();
    assert_eq!(
        *fake.completed.lock().expect("lock"),
        1,
        "one multipart upload"
    );
    assert!(
        fake.uploads.lock().expect("lock").is_empty(),
        "none left open"
    );
    let stored = fake
        .objects
        .lock()
        .expect("value")
        .get(&format!(
            "{prefix}{}",
            Uuid::parse_str(&big).expect("value").simple()
        ))
        .cloned()
        .expect("in the bucket under the prefix");
    assert_eq!(stored, sent);
    let store_id: String = sqlx::query_scalar("SELECT store_id FROM files WHERE id = $1::uuid")
        .bind(&big)
        .fetch_one(&setup.pool)
        .await
        .expect("row");
    assert_eq!(store_id, "object");
    let response = get(
        &setup.app,
        Some(owner),
        &format!("/api/v1/files/{big}/content"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        bytes_of(response).await,
        sent,
        "streamed back through the app"
    );

    // A small one is one put.
    let (status, small) = upload(
        &setup.app,
        owner,
        &format!("/api/v1/conversations/{channel}/files?name=small.txt"),
        Body::from("hello from the bucket"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{small}");
    let small = small["id"].as_str().expect("file").to_owned();
    assert_eq!(*fake.completed.lock().expect("lock"), 1);

    // With downloads sent straight from storage, a download redirects to a
    // presigned link the store serves; nothing else changes.
    sqlx::query(
        r#"
        INSERT INTO settings (id, organization_id, scope, key, value)
        VALUES ($1, $2, 'workspace', 'files.redirect_downloads', 'true'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(setup.organization_id)
    .execute(&setup.pool)
    .await
    .expect("setting");
    let response = get(
        &setup.app,
        Some(owner),
        &format!("/api/v1/files/{small}/content?download=1"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION]
        .to_str()
        .expect("location")
        .to_owned();
    assert!(location.contains("X-Amz-Signature="), "{location}");
    let fetched = reqwest::get(&location).await.expect("presigned get");
    assert_eq!(fetched.status(), 200);
    assert!(
        fetched.headers()["content-disposition"]
            .to_str()
            .expect("value")
            .starts_with("attachment"),
    );
    assert_eq!(
        fetched.text().await.expect("value"),
        "hello from the bucket"
    );
    // Someone outside the channel's project gets no link.
    let response = get(
        &setup.app,
        None,
        &format!("/api/v1/files/{small}/content?download=1"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Removed (never sent), its bytes leave the bucket at the next cleanup.
    let (status, removed) = send(
        &setup.app,
        owner.request(
            "DELETE",
            &format!("/api/v1/files/{small}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{removed}");
    files::cleanup(&state).await.expect("cleanup");
    let left = fake
        .objects
        .lock()
        .expect("value")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        left,
        [format!(
            "{prefix}{}",
            Uuid::parse_str(&big).expect("value").simple()
        )]
    );
}

/// A signed link opens the file for whoever holds it, until the file is
/// removed, its reader loses access, or anyone changes the link.
#[tokio::test]
async fn links_open_only_while_the_reader_still_could() {
    let Some(setup) = setup("files_links").await else {
        return;
    };
    let root = tempfile::tempdir().expect("data root");
    let keys = tempfile::tempdir().expect("keys");
    let (app, _state) = app_with(
        &setup,
        &[
            (
                "AGENT_WORKSPACE_DATA_ROOT",
                root.path().to_string_lossy().into_owned(),
            ),
            ("AGENT_WORKSPACE_SECRETS_KEY_PATH", keyring(keys.path())),
        ],
    );
    let setup = Setup { app, ..setup };
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (rook, token) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, rook, "member").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let channel = create_channel(&setup, &work, "general").await;
    let owner = &setup.browser;

    // Two files too large to read inline, sent in the channel.
    let mut sent = Vec::new();
    let mut ids = Vec::new();
    for name in ["one", "two"] {
        let (body, bytes) = chunks(5, 1024 * 1024);
        let (status, file) = upload(
            &setup.app,
            owner,
            &format!("/api/v1/conversations/{channel}/files?name={name}.txt"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{file}");
        let id = file["id"].as_str().expect("file").to_owned();
        let (status, message) = send(
            &setup.app,
            owner.request(
                "POST",
                &format!("/api/v1/conversations/{channel}/messages"),
                Some(&format!("send-{name}")),
                json!({"content": text(name), "attachments": [id]}),
            ),
        )
        .await;
        assert!(status.is_success(), "{message}");
        sent.push(bytes);
        ids.push(id);
    }
    let link_for = |id: String| {
        let (app, token, session) = (setup.app.clone(), token.clone(), session.clone());
        async move {
            let read = tool(&app, &token, &session, "file_read", json!({"fileId": id})).await;
            let url =
                find(&read, "downloadUrl").unwrap_or_else(|| panic!("a download link in {read}"));
            assert!(find(&read, "downloadExpiresAt").is_some(), "{read}");
            url.split_once("/api/")
                .map(|(_, path)| format!("/api/{path}"))
                .expect("a link to this server")
        }
    };
    let one = link_for(ids[0].clone()).await;
    let two = link_for(ids[1].clone()).await;

    // Anyone holding it can download it, with no session, as a download.
    let response = get(&setup.app, None, &one).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .expect("value")
            .starts_with("attachment")
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(bytes_of(response).await, sent[0]);

    // Changed anywhere, it opens nothing.
    let token_at = one.rfind('/').expect("token") + 1;
    for index in [token_at + 3, one.len() - 2] {
        let mut tampered = one.clone().into_bytes();
        tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).expect("value");
        let response = get(&setup.app, None, &tampered).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{tampered}");
    }

    // The file removed, its link stops.
    sqlx::query("UPDATE files SET removed_at = now(), removed_by_actor_id = uploaded_by_actor_id WHERE id = $1::uuid")
        .bind(&ids[0])
        .execute(&setup.pool)
        .await
        .expect("remove");
    assert_eq!(
        get(&setup.app, None, &one).await.status(),
        StatusCode::NOT_FOUND
    );

    // Rook's access ended, Rook's link to the other file stops too.
    assert_eq!(get(&setup.app, None, &two).await.status(), StatusCode::OK);
    sqlx::query("UPDATE project_memberships SET enabled = false WHERE actor_id = $1")
        .bind(rook)
        .execute(&setup.pool)
        .await
        .expect("disable");
    assert_eq!(
        get(&setup.app, None, &two).await.status(),
        StatusCode::NOT_FOUND
    );
}

fn find(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| map.values().find_map(|value| find(value, key))),
        Value::Array(items) => items.iter().find_map(|value| find(value, key)),
        _ => None,
    }
}
