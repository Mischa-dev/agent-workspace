//! H1.18: OpenID Connect sign-in against a stub issuer on loopback. The
//! silent, costly failures: an email the issuer didn't verify taking over
//! someone's account, a sign-in's state or nonce working twice, and a
//! token not signed by the issuer accepted.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::support::{app::app, db::db};

/// What the stub issuer will say about the next code it is given.
#[derive(Clone)]
struct Grant {
    challenge: String,
    claims: Value,
}

#[derive(Clone)]
struct Issuer {
    base: String,
    key: Arc<EncodingKey>,
    jwk: Value,
    grants: Arc<Mutex<HashMap<String, Grant>>>,
}

async fn discovery(State(issuer): State<Issuer>) -> Json<Value> {
    Json(json!({
        "issuer": issuer.base,
        "authorization_endpoint": format!("{}/authorize", issuer.base),
        "token_endpoint": format!("{}/token", issuer.base),
        "jwks_uri": format!("{}/jwks", issuer.base),
    }))
}

async fn keys(State(issuer): State<Issuer>) -> Json<Value> {
    Json(json!({ "keys": [issuer.jwk] }))
}

async fn token(
    State(issuer): State<Issuer>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let grant = issuer
        .grants
        .lock()
        .expect("grants")
        .remove(form.get("code").ok_or(StatusCode::BAD_REQUEST)?)
        .ok_or(StatusCode::BAD_REQUEST)?;
    // PKCE: the verifier must be the one the challenge was made from.
    let verifier = form.get("code_verifier").ok_or(StatusCode::BAD_REQUEST)?;
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != grant.challenge
        || form.get("client_secret").map(String::as_str) != Some("stub-client-secret")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("stub-key".to_owned());
    let id_token = jsonwebtoken::encode(&header, &grant.claims, &issuer.key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        json!({ "id_token": id_token, "token_type": "Bearer", "access_token": "unused" }),
    ))
}

async fn stub_issuer() -> Issuer {
    let random = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).expect("key");
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
        .expect("pair");
    let point = pair.public_key().as_ref();
    let jwk = json!({
        "kty": "EC", "crv": "P-256", "kid": "stub-key", "alg": "ES256", "use": "sig",
        "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
        "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("issuer");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let issuer = Issuer {
        base,
        key: Arc::new(EncodingKey::from_ec_der(pkcs8.as_ref())),
        jwk,
        grants: Arc::default(),
    };
    let routes = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(keys))
        .route("/token", post(token))
        .with_state(issuer.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, routes).await;
    });
    issuer
}

/// One sign-in, as a browser would go through it: start, the issuer's
/// page (here: the grant the test sets up, with whatever claims the issuer
/// would sign), the callback. The callback's status, where it sends the
/// browser, and whether it set a session.
struct Signed {
    location: String,
    session: bool,
    /// The callback, to try again.
    callback: Request<Body>,
}

async fn sign_in(router: &Router, issuer: &Issuer, claims: impl FnOnce(&str) -> Value) -> Signed {
    let started = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/oidc/start?returnTo=%2Fhome")
                .body(Body::empty())
                .expect("start"),
        )
        .await
        .expect("start");
    assert_eq!(started.status(), StatusCode::SEE_OTHER);
    let cookie = started
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("browser cookie")
        .to_owned();
    let to = url::Url::parse(
        started
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("to the issuer"),
    )
    .expect("issuer address");
    assert!(
        to.as_str()
            .starts_with(&format!("{}/authorize", issuer.base)),
        "{to}"
    );
    let query = to.query_pairs().into_owned().collect::<HashMap<_, _>>();
    assert_eq!(query["code_challenge_method"], "S256");
    let code = Uuid::now_v7().to_string();
    issuer.grants.lock().expect("grants").insert(
        code.clone(),
        Grant {
            challenge: query["code_challenge"].clone(),
            claims: claims(&query["nonce"]),
        },
    );
    let callback = || {
        Request::builder()
            .uri(format!(
                "/auth/oidc/callback?code={code}&state={}",
                query["state"]
            ))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .expect("callback")
    };
    let finished = router.clone().oneshot(callback()).await.expect("callback");
    let location = finished
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let session = finished
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with("agent_workspace_session=") && !value.contains("Max-Age=0"));
    let _ = to_bytes(finished.into_body(), 1_000_000).await;
    Signed {
        location,
        session,
        callback: callback(),
    }
}

fn claims(issuer: &Issuer, sub: &str, email: &str, verified: bool, nonce: &str) -> Value {
    json!({
        "iss": issuer.base,
        "aud": "stub-client",
        "sub": sub,
        "exp": chrono::Utc::now().timestamp() + 300,
        "iat": chrono::Utc::now().timestamp(),
        "nonce": nonce,
        "email": email,
        "email_verified": verified,
        "name": "Signed In",
    })
}

async fn identities(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM oidc_identities")
        .fetch_one(pool)
        .await
        .expect("identities")
}

#[tokio::test]
async fn a_verified_email_links_once_and_nothing_else_gets_in() {
    let Some(test) = db("h1_18_oidc").await else {
        return;
    };
    let pool = &test.pool;
    let issuer = stub_issuer().await;
    let secrets = tempfile::tempdir().expect("secrets");
    let secret = secrets.path().join("oidc-client-secret");
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&secret)
            .expect("secret file");
        file.write_all(b"stub-client-secret\n").expect("secret");
    }

    // A person with a verified email, and a workspace people in the
    // issuer's domain join.
    let mut transaction = pool.begin().await.expect("begin");
    let ada = agent_workspace_server::accounts::create_person(
        &mut transaction,
        agent_workspace_server::accounts::NewPerson {
            login_name: "ada",
            display_name: "Ada",
            default_workspace: None,
            created_by: None,
            claimed: true,
        },
    )
    .await
    .expect("ada");
    sqlx::query("INSERT INTO user_email_addresses (user_id, address, verified_at) VALUES ($1, 'ada@example.com', now())")
        .bind(ada.user_id)
        .execute(&mut *transaction)
        .await
        .expect("ada's email");
    let workspace = agent_workspace_server::workspace::lifecycle::create(
        &mut transaction,
        ada.account_id,
        agent_workspace_server::workspace::lifecycle::NewWorkspace {
            name: "Example".to_owned(),
            slug: Some("example".to_owned()),
            id: None,
        },
    )
    .await
    .expect("workspace");
    transaction.commit().await.expect("commit");

    let (router, _) = app(&test)
        .with("AGENT_WORKSPACE_OIDC_ISSUER", issuer.base.clone())
        .with("AGENT_WORKSPACE_OIDC_CLIENT_ID", "stub-client")
        .with(
            "AGENT_WORKSPACE_OIDC_CLIENT_SECRET_PATH",
            secret.to_string_lossy().into_owned(),
        )
        .with("AGENT_WORKSPACE_OIDC_NAME", "Example sign-in")
        .with("AGENT_WORKSPACE_OIDC_ALLOW_SIGNUP", "true")
        .with("AGENT_WORKSPACE_OIDC_JOIN", "example.com=example")
        .build();

    // The client config names it.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client-config")
                .body(Body::empty())
                .expect("config"),
        )
        .await
        .expect("config");
    let config: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.expect("body"))
            .expect("json");
    assert!(
        config["signIn"]["methods"]
            .as_array()
            .is_some_and(|methods| methods.contains(&json!("oidc"))),
        "{config}"
    );
    assert_eq!(config["signIn"]["oidcName"], "Example sign-in");

    // An unverified email that is Ada's never links, and never makes an
    // account either.
    let refused = sign_in(&router, &issuer, |nonce| {
        claims(&issuer, "mallory", "ada@example.com", false, nonce)
    })
    .await;
    assert!(!refused.session);
    assert_eq!(refused.location, "/sign-in?oidcError=unlinked");
    assert_eq!(identities(pool).await, 0);

    // A token with another sign-in's nonce is refused.
    let replayed = sign_in(&router, &issuer, |_| {
        claims(&issuer, "ada-sub", "ada@example.com", true, "not-this-one")
    })
    .await;
    assert!(!replayed.session);
    assert_eq!(identities(pool).await, 0);

    // Verified: it links to Ada and signs them in.
    let ada_in = sign_in(&router, &issuer, |nonce| {
        claims(&issuer, "ada-sub", "ada@example.com", true, nonce)
    })
    .await;
    assert!(ada_in.session, "{}", ada_in.location);
    assert_eq!(ada_in.location, "/sign-in?returnTo=%2Fhome");
    let linked = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM oidc_identities WHERE subject = 'ada-sub'",
    )
    .fetch_one(pool)
    .await
    .expect("linked");
    assert_eq!(linked, ada.user_id);
    // The same callback again: its state was taken.
    let again = router
        .clone()
        .oneshot(ada_in.callback)
        .await
        .expect("again");
    assert_eq!(
        again
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/sign-in?oidcError=1")
    );

    // Someone new with a verified address in the domain gets an account
    // and joins the workspace.
    let grace = sign_in(&router, &issuer, |nonce| {
        claims(&issuer, "grace-sub", "grace@example.com", true, nonce)
    })
    .await;
    assert!(grace.session, "{}", grace.location);
    let joined = sqlx::query_scalar::<_, String>(
        "SELECT membership.role FROM organization_memberships membership \
         JOIN users person ON person.actor_id = membership.actor_id \
         JOIN oidc_identities identity ON identity.user_id = person.id \
         WHERE identity.subject = 'grace-sub' AND membership.organization_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("joined");
    assert_eq!(joined, "member");
    assert_eq!(identities(pool).await, 2);

    test.finish().await;
}
