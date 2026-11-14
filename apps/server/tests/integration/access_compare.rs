//! K1.2: the access module beside today's checks. For every kind of caller,
//! every catalogue permission, a space and the workspace, `access::check`
//! answers exactly as the operation layer's old check does (compare mode
//! with no differences); a forced difference is recorded once and counted;
//! and the server's operators read it on the workspace's server page.

use agent_workspace_server::{
    access::{
        self, Caller, Need,
        compare::{Compared, compare},
    },
    ops::{Target as OpTarget, access_shim, target::Located},
    policy::{ActorKind, PERMISSIONS, Permission},
};
use axum::http::StatusCode;
use uuid::Uuid;

use super::support::{app::send, world::WorldBuilder};

#[tokio::test]
async fn access_agrees_with_the_old_checks_and_records_a_difference() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vera", "viewer")
        .agent("rook", "member")
        .custom_role("Reader", &["tasks.view", "directory.view"])
        .build("access_compare")
        .await
    else {
        return;
    };
    let space: Uuid = world.project("WORK").parse().expect("space id");
    let workspace = world.organization;
    let reader = world.people["vera"].actor;
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Reader"]).expect("role id"))
    .bind(space)
    .bind(reader)
    .execute(world.pool())
    .await
    .expect("give vera the custom role in WORK");

    let mut callers = vec![
        Caller::person(world.owner.actor, ActorKind::Human),
        Caller::person(world.people["lena"].actor, ActorKind::Human),
        Caller::person(reader, ActorKind::Human),
    ];
    let mut agent = Caller::person(world.agents["rook"].actor, ActorKind::ExternalAgent);
    agent.connection_id = Some(world.agents["rook"].connection);
    callers.push(agent);
    let mut read_only = Caller::person(world.people["lena"].actor, ActorKind::Human);
    read_only.can_write = false;
    callers.push(read_only);
    // A stranger: no membership anywhere here.
    callers.push(Caller::person(Uuid::now_v7(), ActorKind::Human));

    let mut connection = world.pool().acquire().await.expect("connection");
    let mut differences = Vec::new();
    for caller in &callers {
        for (target, located) in [
            (
                OpTarget::Space(space),
                Located {
                    workspace_id: workspace,
                    space_id: Some(space),
                },
            ),
            (
                OpTarget::Workspace(Some(workspace)),
                Located {
                    workspace_id: workspace,
                    space_id: None,
                },
            ),
        ] {
            let access_target = match located.space_id {
                Some(space) => access::Target::Space(space),
                None => access::Target::Workspace(workspace),
            };
            for permission in PERMISSIONS
                .iter()
                .map(|info| Some(info.permission))
                .chain([None])
            {
                for writes in [false, true] {
                    let old = access_shim::check(
                        &mut connection,
                        caller,
                        &target,
                        &located,
                        permission,
                        writes,
                        "compare",
                    )
                    .await
                    .is_ok();
                    let new = access::check(
                        &mut connection,
                        caller,
                        Need::maybe(permission).writing(writes),
                        access_target,
                    )
                    .await
                    .expect("access check")
                    .is_ok();
                    if old != new {
                        differences.push(format!(
                            "{} {permission:?} writes={writes} on {access_target:?}: old {old}, new {new}",
                            caller.account_id
                        ));
                    }
                }
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));

    // The list rule (`access::sql_filter`) answers as `check` does for a
    // space, for every key.
    for caller in callers.iter().filter(|caller| caller.can_read) {
        for info in PERMISSIONS {
            let filter = access::sql_filter(caller, info.permission, "$4::uuid", 1);
            let listed = sqlx::query_scalar::<_, bool>(&format!("SELECT {}", filter.sql))
                .bind(filter.account_id)
                .bind(filter.connection_id)
                .bind(filter.permission)
                .bind(space)
                .fetch_one(&mut *connection)
                .await
                .expect("filter");
            let checked = access::check(
                &mut connection,
                caller,
                Need::permission(info.permission),
                access::Target::Space(space),
            )
            .await
            .expect("access check")
            .is_ok();
            assert_eq!(
                listed, checked,
                "{} {}: list {listed}, check {checked}",
                caller.account_id, info.key
            );
        }
    }

    // A forced difference: the old check said yes where access says no
    // (a viewer editing tasks).
    let stranger = Caller::person(reader, ActorKind::Human);
    let need = Need::permission(Permission::TasksEdit).writes();
    let target = access::Target::Space(space);
    let new = access::check(&mut connection, &stranger, need, target)
        .await
        .expect("access check");
    assert!(new.is_err(), "a reader may not edit tasks here");
    for _ in 0..2 {
        compare(
            world.pool(),
            Compared {
                area: "ops",
                operation: "task_update",
                caller: &stranger,
                need: &need,
                target: &target,
                workspace_id: Some(workspace),
                space_id: Some(space),
                old_allowed: true,
            },
            &new,
        )
        .await;
    }
    // The same answers again on the same account count on one row.
    let rows = sqlx::query_as::<_, (i64, bool, bool, String)>(
        "SELECT count, old_allowed, new_allowed, permission FROM access_disagreements",
    )
    .fetch_all(world.pool())
    .await
    .expect("disagreements");
    assert_eq!(rows, vec![(2, true, false, "tasks.edit".to_owned())]);

    // Answers that agree record nothing.
    compare(
        world.pool(),
        Compared {
            area: "ops",
            operation: "task_update",
            caller: &stranger,
            need: &need,
            target: &target,
            workspace_id: Some(workspace),
            space_id: Some(space),
            old_allowed: false,
        },
        &new,
    )
    .await;
    let total = sqlx::query_scalar::<_, i64>("SELECT sum(count)::bigint FROM access_disagreements")
        .fetch_one(world.pool())
        .await
        .expect("total");
    assert_eq!(total, 2);

    // The server's operator reads it on the workspace's server page; a
    // member who isn't an operator doesn't.
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "GET",
            &format!("/api/v1/organizations/{workspace}/server/access-disagreements"),
            None,
            serde_json::Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["disagreements"][0]["count"], 2, "{body}");
    assert_eq!(body["disagreements"][0]["operation"], "task_update");
    let (status, _) = send(
        &world.app,
        world.people["lena"].browser.request(
            "GET",
            &format!("/api/v1/organizations/{workspace}/server/access-disagreements"),
            None,
            serde_json::Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    drop(connection);
    world.finish().await;
}
