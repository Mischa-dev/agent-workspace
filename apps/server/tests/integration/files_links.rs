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
    /// The SHA-256 (base64) each object was put with, when it was.
    checksums: Arc<Mutex<BTreeMap<String, String>>>,
    /// Bytes the server itself put (header-signed requests).
    server_bytes: Arc<Mutex<u64>>,
    /// Objects put through a presigned link.
    direct_puts: Arc<Mutex<u32>>,
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
            use base64::Engine as _;
            let actual = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&body));
            if presigned {
                // As S3 does: every header the link signed must come, the
                // length must be the body's, and a checksum must match.
                let signed = query
                    .get("X-Amz-SignedHeaders")
                    .cloned()
                    .unwrap_or_default();
                for name in signed.split(';').filter(|name| *name != "host") {
                    if !headers.contains_key(name) {
                        return (StatusCode::FORBIDDEN, "SignatureDoesNotMatch").into_response();
                    }
                }
                if headers
                    .get(header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<usize>().ok())
                    != Some(body.len())
                {
                    return (StatusCode::BAD_REQUEST, "IncompleteBody").into_response();
                }
            }
            if let Some(declared) = headers
                .get("x-amz-checksum-sha256")
                .and_then(|value| value.to_str().ok())
            {
                if declared != actual {
                    return (StatusCode::BAD_REQUEST, "BadDigest").into_response();
                }
                fake.checksums
                    .lock()
                    .expect("lock")
                    .insert(key.clone(), actual);
            }
            if presigned {
                *fake.direct_puts.lock().expect("lock") += 1;
            } else {
                *fake.server_bytes.lock().expect("lock") += body.len() as u64;
            }
            fake.objects
                .lock()
                .expect("lock")
                .insert(key, body.to_vec());
            StatusCode::OK.into_response()
        }
        (Method::HEAD, None, None) => match fake.objects.lock().expect("lock").get(&key) {
            Some(bytes) => {
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, bytes.len());
                if headers.contains_key("x-amz-checksum-mode")
                    && let Some(checksum) = fake.checksums.lock().expect("lock").get(&key)
                {
                    response = response.header("x-amz-checksum-sha256", checksum.clone());
                }
                response.body(Body::empty()).expect("head")
            }
            None => StatusCode::NOT_FOUND.into_response(),
        },
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
                let range = headers
                    .get(header::RANGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.strip_prefix("bytes=0-"))
                    .and_then(|end| end.parse::<usize>().ok());
                let bytes = match range {
                    Some(end) => bytes[..bytes.len().min(end + 1)].to_vec(),
                    None => bytes.clone(),
                };
                let mut response = bytes.into_response();
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

/// K5.13: files on disk move to the object store a batch at a time; a move
/// stopped after one batch resumes where it was; every moved file has the
/// same bytes and is served from its new store; a file uploaded meanwhile
/// goes to the new store; the old copies stay until pruned.
#[tokio::test]
async fn files_move_between_stores_and_resume() {
    let Some(setup) = setup("files_move").await else {
        return;
    };
    let (fake, endpoint) = start_fake().await;
    let root = tempfile::tempdir().expect("data root");
    let keys = tempfile::tempdir().expect("keys");
    let common = vec![
        (
            "AGENT_WORKSPACE_DATA_ROOT",
            root.path().to_string_lossy().into_owned(),
        ),
        ("AGENT_WORKSPACE_S3_ENDPOINT", endpoint),
        ("AGENT_WORKSPACE_S3_BUCKET", BUCKET.to_owned()),
        ("AGENT_WORKSPACE_S3_PATH_STYLE", "true".to_owned()),
        (
            "AGENT_WORKSPACE_S3_ACCESS_KEY_ID_PATH",
            secret_file(keys.path(), "access", ACCESS_KEY),
        ),
        (
            "AGENT_WORKSPACE_S3_SECRET_ACCESS_KEY_PATH",
            secret_file(keys.path(), "secret", "test-secret-key"),
        ),
    ];
    // First on disk...
    let (on_disk, _) = app_with(&setup, &common);
    let mut moving = common.clone();
    moving.push(("AGENT_WORKSPACE_FILE_STORE", "object".to_owned()));
    // ...then the operator makes the object store the primary and moves.
    let (on_object, state) = app_with(&setup, &moving);
    let setup = Setup {
        app: on_disk,
        ..setup
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let channel = create_channel(&setup, &work, "general").await;
    let owner = &setup.browser;
    // One upload, then 249 more like it written straight to disk and the
    // database (uploading them would hit the rate limit).
    let (status, file) = upload(
        &setup.app,
        owner,
        &format!("/api/v1/conversations/{channel}/files?name=f0.txt"),
        Body::from("file number 0"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    let template = file["id"].as_str().expect("file").to_owned();
    let mut ids = vec![template.clone()];
    for index in 1..250 {
        let id = Uuid::now_v7();
        let bytes = format!("file number {index}");
        let key = format!("{}/{}", setup.organization_id.simple(), id.simple());
        std::fs::write(root.path().join("files").join(&key), &bytes).expect("bytes");
        sqlx::query(
            r#"
            INSERT INTO files (
                id, organization_id, project_id, conversation_id, uploaded_by_actor_id,
                name, content_type, byte_size, sha256, storage_key, store_id
            )
            SELECT $2, organization_id, project_id, conversation_id, uploaded_by_actor_id,
                   $3, content_type, $4, $5, $6, store_id
            FROM files WHERE id = $1::uuid
            "#,
        )
        .bind(&template)
        .bind(id)
        .bind(format!("f{index}.txt"))
        .bind(i64::try_from(bytes.len()).expect("size"))
        .bind(hex::encode(Sha256::digest(bytes.as_bytes())))
        .bind(&key)
        .execute(&setup.pool)
        .await
        .expect("file row");
        ids.push(id.to_string());
    }
    let folder = root
        .path()
        .join("files")
        .join(setup.organization_id.simple().to_string());
    let on_disk_count = || std::fs::read_dir(&folder).expect("folder").count();
    assert_eq!(on_disk_count(), 250);
    // Another move (a workspace's, say) holds one file: this one passes it
    // by and writes nothing of it.
    let held = ids[5].parse::<Uuid>().expect("id");
    let held_key = format!("{}/{}", setup.organization_id.simple(), held.simple());
    sqlx::query(
        "UPDATE files SET move_lease = $2, move_lease_until = now() + interval '1 hour' \
         WHERE id = $1",
    )
    .bind(held)
    .bind(Uuid::now_v7())
    .execute(&setup.pool)
    .await
    .expect("hold");

    let mut payload = files::migrate::MovePayload {
        to: "object".to_owned(),
        workspace: None,
        after: None,
        pass: 0,
    };
    let first = files::migrate::move_batch(&state, &payload)
        .await
        .expect("first batch");
    assert_eq!((first.moved, first.failed, first.skipped), (99, 0, 1));
    assert!(
        !fake.objects.lock().expect("lock").contains_key(&held_key),
        "nothing written for a held file"
    );
    // Stopped here; a file uploaded now goes to the new store.
    let (status, during) = upload(
        &on_object,
        owner,
        &format!("/api/v1/conversations/{channel}/files?name=during.txt"),
        Body::from("uploaded during the move"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{during}");
    let during = during["id"].as_str().expect("file").to_owned();
    // Resumed, it finishes in two more batches.
    payload.after = first.next;
    let mut batches = 1;
    loop {
        let moved = files::migrate::move_batch(&state, &payload)
            .await
            .expect("batch");
        assert_eq!(moved.failed, 0);
        batches += 1;
        match moved.next {
            Some(next) => payload.after = Some(next),
            None => break,
        }
    }
    assert_eq!(batches, 3);
    // The walk ended with a file held, so another pass comes.
    let mut again = files::migrate::next_pass(&state, &payload)
        .await
        .expect("next pass")
        .expect("a pass for the held file");
    assert_eq!((again.after, again.pass), (None, 1));
    // Let go, it moves on that pass; the rest are already there.
    sqlx::query("UPDATE files SET move_lease = NULL, move_lease_until = NULL WHERE id = $1")
        .bind(held)
        .execute(&setup.pool)
        .await
        .expect("let go");
    let mut moved_again = 0;
    loop {
        let moved = files::migrate::move_batch(&state, &again)
            .await
            .expect("again");
        moved_again += moved.moved;
        match moved.next {
            Some(next) => again.after = Some(next),
            None => break,
        }
    }
    assert_eq!(moved_again, 1);
    assert!(
        files::migrate::next_pass(&state, &again)
            .await
            .expect("after the pass")
            .is_none(),
        "nothing held, no further pass"
    );
    let stores: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT storage_key, store_id, moved_from_store_id FROM files ORDER BY id")
            .fetch_all(&setup.pool)
            .await
            .expect("rows");
    assert_eq!(stores.len(), 251);
    assert!(stores.iter().all(|(_, store, _)| store == "object"));
    assert_eq!(
        stores
            .iter()
            .filter(|(_, _, from)| from.as_deref() == Some("local"))
            .count(),
        250
    );
    let digests: Vec<(String, String)> = sqlx::query_as("SELECT storage_key, sha256 FROM files")
        .fetch_all(&setup.pool)
        .await
        .expect("digests");
    {
        let objects = fake.objects.lock().expect("lock");
        for (key, sha256) in &digests {
            let bytes = objects.get(key).expect("in the bucket");
            assert_eq!(&hex::encode(Sha256::digest(bytes)), sha256, "{key}");
        }
    }
    // Served from the new store, by either app.
    for (app, id, text) in [
        (&on_object, &ids[7], "file number 7"),
        (&setup.app, &ids[249], "file number 249"),
        (&on_object, &during, "uploaded during the move"),
    ] {
        let response = get(app, Some(owner), &format!("/api/v1/files/{id}/content")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(bytes_of(response).await, text.as_bytes());
    }
    // The old copies stay until pruned.
    assert_eq!(on_disk_count(), 250);
    let pruned = files::migrate::prune_sources(&setup.pool, &state.config)
        .await
        .expect("prune");
    assert_eq!(pruned, 250);
    assert_eq!(on_disk_count(), 0);
    let response = get(
        &on_object,
        Some(owner),
        &format!("/api/v1/files/{}/content", ids[0]),
    )
    .await;
    assert_eq!(bytes_of(response).await, b"file number 0");
}

/// A 1x1 PNG.
const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

async fn op(setup: &Setup, name: &str, input: Value) -> (StatusCode, Value) {
    let path = format!("/api/v2/w/{}/ops/{name}", setup.organization_id);
    op_at(setup, &path, input).await
}

/// An operation on a DM or group, which is in no workspace: its target says
/// where it acts.
async fn direct_op(setup: &Setup, name: &str, input: Value) -> (StatusCode, Value) {
    op_at(setup, &format!("/api/v2/ops/{name}"), input).await
}

async fn op_at(setup: &Setup, path: &str, input: Value) -> (StatusCode, Value) {
    let key = Uuid::now_v7().to_string();
    let (status, body) = send(
        &setup.app,
        setup.browser.request("POST", path, Some(&key), input),
    )
    .await;
    if body["outcome"] == "ok" {
        (status, body["value"].clone())
    } else {
        (status, body)
    }
}

/// Sends bytes to a direct upload link as a client would.
async fn put_direct(started: &Value, bytes: Vec<u8>) -> reqwest::StatusCode {
    let mut request = reqwest::Client::new()
        .put(
            started["url"]
                .as_str()
                .unwrap_or_else(|| panic!("a link to send to: {started}")),
        )
        .body(bytes);
    for (name, value) in started["headers"].as_object().expect("headers") {
        request = request.header(name, value.as_str().expect("header"));
    }
    request.send().await.expect("put").status()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The key a direct upload link writes to, from its address.
fn key_of(started: &Value) -> String {
    let url = url::Url::parse(started["url"].as_str().expect("url")).expect("url");
    decode(
        url.path()
            .strip_prefix(&format!("/{BUCKET}/"))
            .expect("bucket"),
    )
}

fn object_values(
    root: &std::path::Path,
    keys: &std::path::Path,
    endpoint: String,
) -> Vec<(&'static str, String)> {
    vec![
        (
            "AGENT_WORKSPACE_DATA_ROOT",
            root.to_string_lossy().into_owned(),
        ),
        ("AGENT_WORKSPACE_FILE_STORE", "object".to_owned()),
        ("AGENT_WORKSPACE_S3_ENDPOINT", endpoint),
        ("AGENT_WORKSPACE_S3_BUCKET", BUCKET.to_owned()),
        ("AGENT_WORKSPACE_S3_PATH_STYLE", "true".to_owned()),
        (
            "AGENT_WORKSPACE_S3_ACCESS_KEY_ID_PATH",
            secret_file(keys, "access", ACCESS_KEY),
        ),
        (
            "AGENT_WORKSPACE_S3_SECRET_ACCESS_KEY_PATH",
            secret_file(keys, "secret", "test-secret-key"),
        ),
        ("AGENT_WORKSPACE_SECRETS_KEY_PATH", keyring(keys)),
        // The workspace's own bucket below is a local stand-in.
        ("AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES", "true".to_owned()),
    ]
}

/// K5.20: a client uploads straight to the store; the server only signs
/// the link and checks what arrived. A mismatch is refused and deleted, and
/// a picture is served only once its stripped copy exists.
#[tokio::test]
async fn direct_uploads_skip_the_server() {
    let Some(setup) = setup("files_direct").await else {
        return;
    };
    let (fake, endpoint) = start_fake().await;
    let root = tempfile::tempdir().expect("data root");
    let keys = tempfile::tempdir().expect("keys");
    let (app, state) = app_with(&setup, &object_values(root.path(), keys.path(), endpoint));
    let setup = Setup { app, ..setup };
    let work = create_project(&setup, "WORK", "Work").await;
    let channel = create_channel(&setup, &work, "general").await;
    sqlx::query(
        r#"
        INSERT INTO settings (id, organization_id, scope, key, value)
        VALUES ($1, $2, 'workspace', 'files.max_upload_mb', '100'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(setup.organization_id)
    .execute(&setup.pool)
    .await
    .expect("setting");

    // 20 MiB: the server would have sent it in parts; it carries none of it.
    let (_, big) = chunks(20, 1024 * 1024);
    let (status, started) = op(
        &setup,
        "file_upload_start",
        json!({"conversationId": channel, "name": "big.bin", "size": big.len(),
               "sha256": sha256_hex(&big)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["method"], "PUT");
    let upload_id = started["uploadId"].as_str().expect("upload").to_owned();
    // Nothing is a file yet.
    let response = get(
        &setup.app,
        Some(&setup.browser),
        &format!("/api/v1/files/{upload_id}/content"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // Bytes that aren't the ones declared are refused by the store.
    assert_eq!(
        put_direct(&started, vec![b'x'; big.len()]).await,
        reqwest::StatusCode::BAD_REQUEST
    );
    let (status, early) = op(&setup, "file_upload_finish", json!({"uploadId": upload_id})).await;
    assert_eq!(status, StatusCode::CONFLICT, "nothing arrived yet: {early}");
    assert_eq!(
        put_direct(&started, big.clone()).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(*fake.direct_puts.lock().expect("lock"), 1);
    assert!(
        *fake.server_bytes.lock().expect("lock") < 1024 * 1024,
        "the server sent the store under 1 MiB itself"
    );
    assert_eq!(
        *fake.completed.lock().expect("lock"),
        0,
        "no multipart upload"
    );
    let (status, file) = op(&setup, "file_upload_finish", json!({"uploadId": upload_id})).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    let file_id = file["id"].as_str().expect("file").to_owned();
    assert_eq!(file["size"], big.len());
    let response = get(
        &setup.app,
        Some(&setup.browser),
        &format!("/api/v1/files/{file_id}/content"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(bytes_of(response).await, big);
    // Finishing again answers with the same file.
    let (status, again) = op(&setup, "file_upload_finish", json!({"uploadId": upload_id})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], file_id.as_str());

    // A store that took the wrong length: refused at the finish, and the
    // bytes deleted.
    let declared = b"ten bytes!".to_vec();
    let (_, short) = op(
        &setup,
        "file_upload_start",
        json!({"conversationId": channel, "name": "short.txt", "size": declared.len(),
               "sha256": sha256_hex(&declared)}),
    )
    .await;
    let short_key = key_of(&short);
    fake.objects
        .lock()
        .expect("lock")
        .insert(short_key.clone(), b"eleven byte".to_vec());
    let (status, refused) = op(
        &setup,
        "file_upload_finish",
        json!({"uploadId": short["uploadId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["field"], "size", "{refused}");
    assert!(!fake.objects.lock().expect("lock").contains_key(&short_key));
    let (status, _) = op(
        &setup,
        "file_upload_finish",
        json!({"uploadId": short["uploadId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the upload is gone");

    // A picture: the uploaded bytes are never served; the stripped copy is
    // a new object, and the upload's own bytes are deleted.
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG)
        .expect("png");
    let (_, picture) = op(
        &setup,
        "file_upload_start",
        json!({"conversationId": channel, "name": "pic.png", "contentType": "image/png",
               "size": png.len(), "sha256": sha256_hex(&png)}),
    )
    .await;
    assert_eq!(put_direct(&picture, png).await, reqwest::StatusCode::OK);
    let picture_key = key_of(&picture);
    let (status, image) = op(
        &setup,
        "file_upload_finish",
        json!({"uploadId": picture["uploadId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{image}");
    assert_eq!(image["width"], 1);
    assert_ne!(image["id"], picture["uploadId"]);
    let objects = fake.objects.lock().expect("lock").clone();
    assert!(
        !objects.contains_key(&picture_key),
        "the upload's bytes went"
    );
    let image_id = Uuid::parse_str(image["id"].as_str().expect("id")).expect("id");
    assert!(
        objects
            .keys()
            .any(|key| key.ends_with(&image_id.simple().to_string()))
    );

    // Unfinished uploads hold their size: with room for 30 more bytes, a
    // 20-byte start goes and a second one is refused until the first ends.
    let (_, usage) = op(&setup, "workspace_usage_get", json!({})).await;
    let stored = usage["meters"]
        .as_array()
        .expect("meters")
        .iter()
        .find(|meter| meter["meter"] == "storage_bytes")
        .and_then(|meter| meter["used"].as_i64())
        .expect("stored bytes");
    sqlx::query(
        "INSERT INTO spend_limits (id, workspace_id, scope_kind, scope_id, meter, period, \
         hard_limit, set_by_actor_id) \
         SELECT $1, $2, 'workspace', $2, 'storage_bytes', 'month', $3, actor_id \
         FROM users WHERE login_name = 'owner'",
    )
    .bind(Uuid::now_v7())
    .bind(setup.organization_id)
    .bind(stored + 30)
    .execute(&setup.pool)
    .await
    .expect("storage limit");
    let twenty = vec![b'r'; 20];
    let start_twenty = |name: &'static str| {
        op(
            &setup,
            "file_upload_start",
            json!({"conversationId": channel, "name": name, "size": twenty.len(),
                   "sha256": sha256_hex(&twenty)}),
        )
    };
    let (status, first) = start_twenty("first.bin").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, second) = start_twenty("second.bin").await;
    assert_eq!(second["outcome"], "limit_reached", "{status} {second}");

    // A day on, the unfinished upload and its bytes go; the finished one's
    // bytes stay with its file.
    assert_eq!(
        put_direct(&first, twenty.clone()).await,
        reqwest::StatusCode::OK
    );
    let first_key = key_of(&first);
    let big_key = key_of(&started);
    sqlx::query("UPDATE file_uploads SET created_at = now() - interval '2 days'")
        .execute(&setup.pool)
        .await
        .expect("a day on");
    files::cleanup(&state).await.expect("cleanup");
    let objects = fake.objects.lock().expect("lock").clone();
    assert!(
        !objects.contains_key(&first_key),
        "the unfinished bytes went"
    );
    assert!(
        objects.contains_key(&big_key),
        "a finished upload keeps its bytes"
    );
    let left =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM file_uploads WHERE finished_at IS NULL")
            .fetch_one(&setup.pool)
            .await
            .expect("uploads");
    assert_eq!(left, 0);
    let pending = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM file_objects_pending")
        .fetch_one(&setup.pool)
        .await
        .expect("pending");
    assert_eq!(pending, 0, "deleted bytes leave no pending entry");
}

/// K5.20: a workspace's own bucket takes its new uploads, through the
/// server or straight, once a probe there passed; its keys never appear in
/// an answer or in a row as written.
#[tokio::test]
async fn a_workspace_bucket_takes_new_uploads() {
    let Some(setup) = setup("files_own_bucket").await else {
        return;
    };
    let (fake, endpoint) = start_fake().await;
    let (own, own_endpoint) = start_fake().await;
    let root = tempfile::tempdir().expect("data root");
    let keys = tempfile::tempdir().expect("keys");
    let (app, state) = app_with(&setup, &object_values(root.path(), keys.path(), endpoint));
    let setup = Setup { app, ..setup };
    let work = create_project(&setup, "WORK", "Work").await;
    let channel = create_channel(&setup, &work, "general").await;
    let secret = "own-bucket-secret-value";

    // Keys the bucket refuses: the probe fails and nothing is recorded.
    let (status, refused) = op(
        &setup,
        "workspace_store_set",
        json!({"endpoint": own_endpoint, "bucket": BUCKET, "region": "us-east-1",
               "pathStyle": true, "accessKeyId": "WRONG", "secretAccessKey": secret}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(!refused.to_string().contains(secret));
    let (status, none) = op(&setup, "workspace_store_get", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{none}");
    assert!(none.get("store").is_none(), "{none}");

    let (status, set) = op(
        &setup,
        "workspace_store_set",
        json!({"endpoint": own_endpoint, "bucket": BUCKET, "region": "us-east-1",
               "pathStyle": true, "accessKeyId": ACCESS_KEY, "secretAccessKey": secret}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert!(!set.to_string().contains(secret), "{set}");
    let store_id = set["store"]["storeId"].as_str().expect("store").to_owned();
    assert!(store_id.starts_with("ws-"));
    assert!(
        own.objects.lock().expect("lock").is_empty(),
        "the probe cleaned up"
    );

    // Through the server: into the workspace's bucket.
    let (status, small) = upload(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/conversations/{channel}/files?name=small.txt"),
        Body::from("in the workspace's bucket"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{small}");
    let small = small["id"].as_str().expect("file").to_owned();
    let stored: String = sqlx::query_scalar("SELECT store_id FROM files WHERE id = $1::uuid")
        .bind(&small)
        .fetch_one(&setup.pool)
        .await
        .expect("row");
    assert_eq!(stored, store_id);
    assert_eq!(own.objects.lock().expect("lock").len(), 1);
    assert!(fake.objects.lock().expect("lock").is_empty());
    let response = get(
        &setup.app,
        Some(&setup.browser),
        &format!("/api/v1/files/{small}/content"),
    )
    .await;
    assert_eq!(bytes_of(response).await, b"in the workspace's bucket");

    // Straight: the link points at the workspace's bucket.
    let bytes = b"direct to the workspace".to_vec();
    let (_, started) = op(
        &setup,
        "file_upload_start",
        json!({"conversationId": channel, "name": "d.txt", "size": bytes.len(),
               "sha256": sha256_hex(&bytes)}),
    )
    .await;
    assert_eq!(
        put_direct(&started, bytes.clone()).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(*own.direct_puts.lock().expect("lock"), 1);
    let (status, file) = op(
        &setup,
        "file_upload_finish",
        json!({"uploadId": started["uploadId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");

    // The keys are in no row as written, nor in the audit trail.
    for table in ["workspace_stores", "secrets", "audit_events", "events"] {
        let found: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} row WHERE row_to_json(row)::text LIKE '%' || $1 || '%'"
        ))
        .bind(secret)
        .fetch_one(&setup.pool)
        .await
        .expect("search");
        assert_eq!(found, 0, "the secret is in {table}");
    }

    // A group's file is for its members: it stays in the server's store,
    // whose keys the workspace's managers don't hold, by either path.
    let space = space(&setup).await;
    let (rook, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, rook, "member").await;
    let (status, group) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("group-rook"),
            json!({"name": "plans", "memberIds": [rook]}),
        ),
    )
    .await;
    assert!(status.is_success(), "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let own_before = own.objects.lock().expect("lock").len();
    let (status, private) = upload(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/files?name=private.txt"),
        Body::from("for the group only"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{private}");
    let private = private["id"].as_str().expect("file").to_owned();
    let (stored, members_only): (String, bool) =
        sqlx::query_as("SELECT store_id, members_only FROM files WHERE id = $1::uuid")
            .bind(&private)
            .fetch_one(&setup.pool)
            .await
            .expect("row");
    assert_eq!((stored.as_str(), members_only), ("object", true));
    let bytes = b"straight, for the group".to_vec();
    let (_, started) = direct_op(
        &setup,
        "file_upload_start",
        json!({"conversationId": group, "name": "g.txt", "size": bytes.len(),
               "sha256": sha256_hex(&bytes)}),
    )
    .await;
    assert_eq!(
        put_direct(&started, bytes.clone()).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(own.objects.lock().expect("lock").len(), own_before);
    assert_eq!(fake.objects.lock().expect("lock").len(), 2);
    // Moving the workspace's files into its bucket passes the group's by.
    let moved = files::migrate::move_batch(
        &state,
        &files::migrate::MovePayload {
            to: store_id.clone(),
            workspace: Some(setup.organization_id),
            after: None,
            pass: 0,
        },
    )
    .await
    .expect("move");
    assert_eq!(moved.failed, 0);
    let stored: String = sqlx::query_scalar("SELECT store_id FROM files WHERE id = $1::uuid")
        .bind(&private)
        .fetch_one(&setup.pool)
        .await
        .expect("row");
    assert_eq!(stored, "object");
    let (status, _) = direct_op(
        &setup,
        "file_upload_finish",
        json!({"uploadId": started["uploadId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Stopping: new uploads go to the server's store; old files still read.
    let (status, cleared) = op(&setup, "workspace_store_revoke", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    let (status, later) = upload(
        &setup.app,
        &setup.browser,
        &format!("/api/v1/conversations/{channel}/files?name=later.txt"),
        Body::from("back on the server's store"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{later}");
    assert_eq!(fake.objects.lock().expect("lock").len(), 3);
    let response = get(
        &setup.app,
        Some(&setup.browser),
        &format!("/api/v1/files/{small}/content"),
    )
    .await;
    assert_eq!(bytes_of(response).await, b"in the workspace's bucket");

    // Deleting the workspace's files reaches its own bucket too.
    for store in crate::support::as_job(
        setup.organization_id,
        files::workspace_store::all_of(&state, setup.organization_id),
    )
    .await
    .expect("stores")
    {
        store
            .delete_workspace(setup.organization_id)
            .await
            .expect("delete");
    }
    assert!(own.objects.lock().expect("lock").is_empty());
}
