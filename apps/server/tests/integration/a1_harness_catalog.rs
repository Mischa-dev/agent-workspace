//! A1.17: the harnesses the server offers are what the built-in host
//! described when it took its lease, plus each workspace's registered ones.
//! A workspace sees only its own registered harnesses, a new list replaces
//! the last, and only the built-in host may describe.

use agent_workspace_host_protocol::{BuiltinHarnesses, PROTOCOL_HEADER, PROTOCOL_VERSION};
use agent_workspace_server::{
    harness::{manifest::RunLocation, registered},
    harness_catalog::Harnesses,
    hosts::builtin::transport,
    tenancy,
};
use axum::{body::Body, http::Request};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use crate::support::two_workspaces::two_workspaces;

fn manifest(kind: &str) -> agent_workspace_server::harness::manifest::AdapterManifest {
    registered::command_line_manifest(kind, "Described", &json!({}), RunLocation::LocalHost)
        .expect("manifest")
}

#[tokio::test]
async fn the_catalog_is_what_the_host_described_and_the_workspace_registered() {
    let Some(world) = two_workspaces("a1_harness_catalog").await else {
        return;
    };
    let state = world.state.clone();
    let of = |organization| {
        let state = state.clone();
        tenancy::system("harnesses.reload", async move {
            Harnesses::of(&state, organization).await.expect("catalog")
        })
    };

    // Before the host has described anything, the process that holds it
    // answers for it.
    let before = of(world.a.organization).await;
    assert!(before.manifests().next().is_some());

    // The host describes its harnesses through its own client.
    let client = transport::client(&state);
    let first = manifest("described_one");
    client
        .builtin_harnesses(&BuiltinHarnesses {
            harnesses: vec![first.clone()],
        })
        .await
        .expect("describe");
    let a = of(world.a.organization).await;
    assert_eq!(a.manifest("described_one"), Some(&first));
    // Besides the workspace's registered harnesses (`x_…`), only what the
    // host described.
    assert_eq!(
        a.manifests()
            .map(|manifest| manifest.kind.as_str())
            .filter(|kind| !kind.starts_with("x_"))
            .collect::<Vec<_>>(),
        ["described_one"],
    );

    // A registered harness belongs to its workspace alone.
    sqlx::query(
        "INSERT INTO custom_harnesses (id, organization_id, kind, name, transport, manifest, \
         command, created_by_actor_id) \
         VALUES ($1, $2, 'x_catalog', 'Catalog harness', 'command', '{}', '[]', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(world.a.organization)
    .bind(world.a.agent)
    .execute(&world.pool)
    .await
    .expect("register a harness");
    let a = of(world.a.organization).await;
    let b = of(world.b.organization).await;
    assert_eq!(
        a.manifest("x_catalog")
            .map(|manifest| manifest.display_name.as_str()),
        Some("Catalog harness")
    );
    assert!(b.manifest("x_catalog").is_none());
    assert!(b.manifest("described_one").is_some());

    // A new list replaces the last.
    client
        .builtin_harnesses(&BuiltinHarnesses {
            harnesses: vec![manifest("described_two")],
        })
        .await
        .expect("describe again");
    let b = of(world.b.organization).await;
    assert!(b.manifest("described_one").is_none());
    assert!(b.manifest("described_two").is_some());

    // A list that breaks the rules is refused whole.
    assert!(
        client
            .builtin_harnesses(&BuiltinHarnesses {
                harnesses: vec![manifest("twice"), manifest("twice")],
            })
            .await
            .is_err()
    );
    assert!(
        of(world.b.organization)
            .await
            .manifest("described_two")
            .is_some()
    );

    // Nobody else may describe the built-in host's harnesses.
    let refused = world
        .app
        .clone()
        .oneshot(
            Request::put("/api/v1/host/builtin/harnesses")
                .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
                .header("authorization", format!("Bearer {}", world.a.agent_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&BuiltinHarnesses {
                        harnesses: vec![manifest("intruder")],
                    })
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("answer");
    assert_eq!(refused.status(), axum::http::StatusCode::UNAUTHORIZED);
    assert!(
        of(world.a.organization)
            .await
            .manifest("intruder")
            .is_none()
    );

    world.finish().await;
}
