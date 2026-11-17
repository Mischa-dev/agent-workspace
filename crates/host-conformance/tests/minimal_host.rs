//! The minimal host passes the suite; a host that forgets the starts it
//! ran is caught as "start not idempotent", never passed.

use agent_workspace_host_conformance::{FakeWorkspace, Outcome, check_host};
use agent_workspace_minimal_host::{MinimalHost, enrol, serve};

async fn run_against(
    fake: &FakeWorkspace,
    forgetful: bool,
) -> agent_workspace_host_conformance::Report {
    let base = fake.base.clone();
    let code = fake.code.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = tokio::spawn(async move {
        let client = enrol(&base, &code, "Minimal host").await.expect("enrol");
        let mut host = MinimalHost::new(client);
        if forgetful {
            host.forget_starts();
        }
        serve(host, async {
            let _ = stopped.await;
        })
        .await
    });
    let report = check_host(fake).await;
    let _ = stop.send(());
    let _ = host.await;
    report
}

#[tokio::test]
async fn the_minimal_host_passes() {
    let fake = FakeWorkspace::start("127.0.0.1:0").await.expect("listen");
    let report = run_against(&fake, false).await;
    assert!(report.passed(), "{report}");
    for notice in [
        "sends agent_profile_changed",
        "sends agent_ref_changed",
        "sends agent_retired",
    ] {
        assert_eq!(
            report.check(notice).map(|check| check.outcome),
            Some(Outcome::Pass),
            "{report}"
        );
    }
}

#[tokio::test]
async fn a_host_that_starts_a_run_twice_fails() {
    let fake = FakeWorkspace::start("127.0.0.1:0").await.expect("listen");
    let report = run_against(&fake, true).await;
    let check = report
        .check("a start offered again is not started twice")
        .expect("checked");
    assert_eq!(check.outcome, Outcome::Fail, "{report}");
    assert!(check.detail.starts_with("start not idempotent"), "{report}");
}
