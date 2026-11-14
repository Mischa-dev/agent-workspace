//! K5: any number of servers. Two instances on one database act as one: a
//! sign-in on one works on the other, a write on one reads on the other.

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::cluster_support::cluster;

#[tokio::test]
async fn sign_in_on_one_instance_works_on_the_other() {
    let Some(cluster) = cluster("k5_1_sign_in", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    for instance in &cluster.instances {
        let (status, session) = instance
            .send(browser.request("GET", "/api/v1/auth/session", None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::OK, "{}: {session}", instance.name);
        assert!(session.to_string().contains("owner"), "{session}");
    }
    cluster.finish().await;
}

#[tokio::test]
async fn write_on_one_read_on_the_other() {
    let Some(mut cluster) = cluster("k5_1_write_read", 2).await else {
        return;
    };
    let a = cluster.instance(0);
    let b = cluster.instance(1);
    let browser = a.login("owner").await;
    let (status, project) = a
        .send(browser.request(
            "POST",
            "/api/v1/projects",
            Some("cluster-project"),
            json!({"organizationId": cluster.organization_id, "key": "WORK", "name": "Work"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project = project["id"].as_str().expect("project").to_owned();
    let (status, task) = a
        .send(browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/work-items"),
            Some("cluster-task"),
            json!({"title": "Written on A"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let (status, read) = b
        .send(browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/work-items/{task}"),
            None,
            Value::Null,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert!(read.to_string().contains("Written on A"), "{read}");

    // A stops as a crash would; B carries on serving.
    cluster.instance_mut(0).abort();
    let b = cluster.instance(1);
    let (status, _) = b
        .send(browser.request("GET", "/api/v1/auth/session", None, Value::Null))
        .await;
    assert_eq!(status, StatusCode::OK);
    cluster.finish().await;
}
