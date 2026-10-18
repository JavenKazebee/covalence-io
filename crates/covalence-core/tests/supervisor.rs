//! The host supervisor and action path, against the fake host in
//! `testing/fake_host.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use covalence_core::gate::Verdict;
use covalence_core::host::{Backoff, HostConfig, HostPhase, HostStatus};
use covalence_core::journal::{Entry, Journal, Origin, Record, SuppressReason};
use covalence_core::state::{Quality, StatePath};
use covalence_core::{ActionRequest, Core, Shared};
use covalence_protocol::action::ActionInvoke;
use covalence_protocol::connection::{ConnCreate, ConnMode, ConnStatusKind};
use covalence_protocol::plugin::{Permissions, PluginLoad};
use covalence_protocol::state::PathSegment;
use covalence_protocol::{ErrorCode, Value};
use covalence_store::Store;
use tokio::sync::watch;
use tokio::time::Instant;

const CONN: &str = "01JCONN0000000000000000000";

fn config() -> HostConfig {
    // The format comes from COVALENCE_PROTOCOL_DEBUG, like the fake host's.
    let mut config = HostConfig::new(env!("CARGO_BIN_EXE_covalence-fake-host"));
    config.backoff = Backoff {
        initial: Duration::from_millis(50),
        max: Duration::from_millis(500),
        reset_after: Duration::from_secs(30),
    };
    config
}

fn create(mode: ConnMode) -> ConnCreate {
    ConnCreate {
        plugin: "io.covalence.fake".into(),
        version: "0.1.0".into(),
        alias: "vh".into(),
        config: BTreeMap::new(),
        secrets: BTreeMap::new(),
        permissions: Permissions::default(),
        mode,
        isolate: false,
    }
}

fn source(output: i64) -> StatePath {
    StatePath::new(
        CONN,
        vec![
            PathSegment::Key("outputs".into()),
            PathSegment::Index(output),
            PathSegment::Key("source".into()),
        ],
    )
}

fn route(output: i64, input: i64) -> ActionRequest {
    let params = Value::Map(vec![
        ("output".into(), output.into()),
        ("input".into(), input.into()),
    ]);
    let origin = Origin::User {
        user: "test".into(),
        client: "api".into(),
    };
    ActionRequest::new(origin, CONN, "route", params)
}

async fn start(config: HostConfig, shared: Shared, mode: ConnMode) -> Core {
    let core = Core::start(shared, config);
    let load = PluginLoad {
        plugin: "io.covalence.fake".into(),
        version: "0.1.0".into(),
        bundle: "/dev/null".into(),
    };
    // Registered before the host is up, so these exercise the setup path.
    core.host().load_plugin(load).await.unwrap();
    core.host()
        .create_connection(CONN, create(mode))
        .await
        .unwrap();
    wait_status(core.host().status(), |s| s.phase == HostPhase::Ready).await;
    let state = core.shared().state.clone();
    eventually("initial snapshot", || state.value(&source(1)).is_some()).await;
    core
}

async fn wait_status(
    mut rx: watch::Receiver<HostStatus>,
    pred: impl FnMut(&HostStatus) -> bool,
) -> HostStatus {
    tokio::time::timeout(Duration::from_secs(10), rx.wait_for(pred))
        .await
        .expect("host status timed out")
        .unwrap()
        .clone()
}

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn entries(journal: &Journal) -> Vec<Arc<Entry>> {
    journal.range(1, usize::MAX).unwrap()
}

#[tokio::test]
async fn routes_an_action_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("core.db")).unwrap();
    let shared = Shared::new(Journal::with_store(store, 1024).unwrap());
    let core = start(config(), shared.clone(), ConnMode::Simulate).await;

    let outcome = core.request_action(route(1, 3)).await.unwrap();
    assert_eq!(outcome.verdict, Verdict::Dispatch);
    assert_eq!(outcome.result, Some(Ok(None)));

    // The host sends the patch before the result, and the core keeps that
    // order, so the state is already updated.
    assert_eq!(shared.state.value(&source(1)), Some(Value::from(3)));
    let leaf = shared.state.leaf(&source(1)).unwrap();
    assert_eq!(leaf.meta.quality, Quality::Good);
    let patch = shared.journal.get(leaf.meta.seq).unwrap().unwrap();
    assert!(matches!(patch.record, Record::StatePatch(_)));
    // The patch names its call, so it is caused by the request.
    assert_eq!(patch.cause, Some(outcome.requested));
    let chain: Vec<&str> = shared
        .journal
        .why(patch.seq)
        .unwrap()
        .iter()
        .map(|e| e.record.kind())
        .collect();
    assert_eq!(chain, ["state.patch", "action.requested"]);

    // why(action.result) walks back to the request.
    let chain: Vec<&str> = shared
        .journal
        .why(outcome.result_seq.unwrap())
        .unwrap()
        .iter()
        .map(|e| e.record.kind())
        .collect();
    assert_eq!(chain, ["action.result", "action.requested"]);

    let kinds: Vec<&str> = entries(&shared.journal)
        .iter()
        .filter(|e| e.seq >= outcome.requested)
        .map(|e| e.record.kind())
        .filter(|k| k.starts_with("action."))
        .collect();
    assert_eq!(
        kinds,
        ["action.requested", "action.dispatched", "action.result"]
    );
    eventually("routeChanged event", || {
        entries(&shared.journal)
            .iter()
            .any(|e| matches!(&e.record, Record::Event(ev) if ev.event == "routeChanged"))
    })
    .await;
    let event = entries(&shared.journal)
        .into_iter()
        .find(|e| matches!(&e.record, Record::Event(_)))
        .unwrap();
    assert_eq!(event.cause, Some(outcome.requested));

    // A failing action journals a failed result.
    let mut fail = route(0, 0);
    fail.action = "fail".into();
    let outcome = core.request_action(fail).await.unwrap();
    assert_eq!(
        outcome.result.unwrap().unwrap_err().code,
        ErrorCode::ActionFailed
    );
    let result = shared
        .journal
        .get(outcome.result_seq.unwrap())
        .unwrap()
        .unwrap();
    assert!(matches!(&result.record, Record::ActionResult(r) if !r.ok));

    core.shutdown().await;
}

#[tokio::test]
async fn gate_suppresses_without_invoking() {
    let shared = Shared::new(Journal::in_memory(1024));
    let core = start(config(), shared.clone(), ConnMode::Live).await;

    core.set_simulate(true);
    let outcome = core.request_action(route(1, 3)).await.unwrap();
    assert_eq!(outcome.verdict, Verdict::Suppress(SuppressReason::Simulate));
    assert_eq!(outcome.result, None);
    core.set_simulate(false);

    shared.journal.set_term(2).unwrap();
    let mut stale = route(1, 3);
    stale.term = Some(1);
    let outcome = core.request_action(stale).await.unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::Suppress(SuppressReason::StaleTerm)
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(shared.state.value(&source(1)), Some(Value::from(0)));
    let suppressed = entries(&shared.journal)
        .iter()
        .filter(|e| matches!(e.record, Record::ActionSuppressed(_)))
        .count();
    assert_eq!(suppressed, 2);

    // The host fences too: after seeing term 2 it rejects term 1.
    let outcome = core.request_action(route(1, 2)).await.unwrap();
    assert_eq!(outcome.result, Some(Ok(None)));
    let invoke = ActionInvoke {
        call: "x".into(),
        run: None,
        term: 1,
        action: "route".into(),
        params: Value::Nil,
        deadline: None,
    };
    let err = core.host().invoke(CONN, &invoke, None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleTerm);

    core.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn restarts_and_resyncs_after_the_host_is_killed() {
    let shared = Shared::new(Journal::in_memory(4096));
    let core = start(config(), shared.clone(), ConnMode::Simulate).await;
    let before = core.host().status().borrow().clone();
    assert_eq!(before.generation, 1);

    let killed_at = shared.journal.last_seq().unwrap();
    let started = Instant::now();
    let status = std::process::Command::new("kill")
        .args(["-9", &before.pid.unwrap().to_string()])
        .status()
        .unwrap();
    assert!(status.success());

    // The subtree goes stale, but keeps its values.
    eventually("stale state", || {
        shared.state.leaf(&source(1)).unwrap().meta.quality == Quality::Stale
    })
    .await;
    assert_eq!(shared.state.value(&source(1)), Some(Value::from(0)));

    let after = wait_status(core.host().status(), |s| {
        s.generation == 2 && s.phase == HostPhase::Ready
    })
    .await;
    assert_ne!(after.pid, before.pid);
    eventually("good state after resync", || {
        shared.state.leaf(&source(1)).unwrap().meta.quality == Quality::Good
    })
    .await;
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(3), "restart took {elapsed:?}");

    let since: Vec<Arc<Entry>> = entries(&shared.journal)
        .into_iter()
        .filter(|e| e.seq > killed_at)
        .collect();
    let disconnected = since
        .iter()
        .position(|e| {
            matches!(&e.record, Record::ConnStatus(s) if s.status == ConnStatusKind::Disconnected)
        })
        .expect("conn.status disconnected");
    let stale = since
        .iter()
        .position(|e| matches!(&e.record, Record::StateQuality(q) if q.quality == Quality::Stale))
        .expect("state.quality stale");
    assert_eq!(since[stale].cause, Some(since[disconnected].seq));
    let resync = since
        .iter()
        .position(|e| {
            matches!(&e.record, Record::Log(l) if l.message == "resync")
                && e.origin == Origin::conn(CONN)
        })
        .expect("conn.resync reached the new host");
    assert!(resync > stale);

    // Actions work on the new host.
    let outcome = core.request_action(route(2, 1)).await.unwrap();
    assert_eq!(outcome.result, Some(Ok(None)));

    core.shutdown().await;
}

#[tokio::test]
async fn unanswered_pings_mark_the_host_dead() {
    let mut config = config();
    config.ping_interval = Duration::from_millis(100);
    config.env = vec![("FAKE_HOST_IGNORE_PINGS".into(), "1".into())];
    let shared = Shared::new(Journal::in_memory(4096));
    let core = start(config, shared.clone(), ConnMode::Simulate).await;

    wait_status(core.host().status(), |s| s.generation >= 2).await;
    let reported = entries(&shared.journal)
        .iter()
        .any(|e| matches!(&e.record, Record::Log(l) if l.message.contains("3 pings unanswered")));
    assert!(reported);

    core.shutdown().await;
}

#[tokio::test]
async fn a_host_that_cannot_start_is_retried_with_backoff() {
    let mut config = config();
    config.program = "/nonexistent/covalence-host".into();
    let shared = Shared::new(Journal::in_memory(1024));
    let core = Core::start(shared.clone(), config);
    wait_status(core.host().status(), |s| s.generation >= 3).await;
    let delays: Vec<String> = entries(&shared.journal)
        .iter()
        .filter_map(|e| match &e.record {
            Record::Log(l) if l.message.contains("restarting in") => {
                l.message.rsplit("restarting in ").next().map(str::to_owned)
            }
            _ => None,
        })
        .collect();
    assert_eq!(delays[..2], ["50ms", "100ms"]);
    core.shutdown().await;
}
