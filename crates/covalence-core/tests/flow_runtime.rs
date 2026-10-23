//! The flow runtime on a paused clock, with a stand-in for the core that
//! journals actions the way the core does (M3 Phase 2).

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use covalence_core::Shared;
use covalence_core::engine::{ActionOutcome, ActionRequest, Actions, CoreError};
use covalence_core::expr::QuickJs;
use covalence_core::flow::runtime::{FireOutcome, QUEUE_LIMIT, RunInfo, RunPhase, STATE_ROOT};
use covalence_core::flow::{Flow, Runtime};
use covalence_core::gate::Verdict;
use covalence_core::journal::{
    ActionRequestedRecord, ActionResultRecord, BlockReason, Entry, EventRecord, Journal, JournalOp,
    NewEntry, Origin, Record, Seq, StatePatchRecord, StepDetail, StepStatus, TriggerMatchedRecord,
};
use covalence_core::state::{Meta, Quality, Source, StatePath};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use covalence_protocol::{ErrorCode, ProtocolError};
use serde_json::json;

const VH: &str = "vh-id";
const VH2: &str = "vh2-id";
/// A connection the stand-in reports as not running.
const GONE: &str = "gone-id";

/// Journals `action.requested`, takes `delay`, then emits `routeChanged`
/// from the target (as the Videohub simulator does) and journals the result.
/// Actions named in `failing` fail.
struct FakeCore {
    journal: Arc<Journal>,
    delay: Duration,
    failing: Mutex<BTreeSet<String>>,
}

impl Actions for FakeCore {
    fn request_action(
        &self,
        req: ActionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CoreError>> + Send + '_>> {
        Box::pin(async move {
            if req.conn == GONE {
                return Err(CoreError::UnknownConnection(req.conn));
            }
            let call = format!("call-{}", self.journal.last_seq().unwrap_or(0));
            let requested = self
                .journal
                .append(
                    NewEntry::new(
                        req.origin,
                        Record::ActionRequested(ActionRequestedRecord {
                            call: call.clone(),
                            conn: req.conn.clone(),
                            action: req.action.clone(),
                            params: req.params,
                            deadline: req.deadline,
                        }),
                    )
                    .cause(req.cause)
                    .run(req.run.clone()),
                )?
                .seq;
            tokio::time::sleep(self.delay).await;
            let fails = self.failing.lock().unwrap().contains(&req.action);
            if !fails {
                self.journal.append(
                    NewEntry::new(
                        Origin::conn(req.conn.clone()),
                        Record::Event(EventRecord {
                            source: Source::Conn(req.conn.clone()),
                            event: "routeChanged".into(),
                            payload: None,
                        }),
                    )
                    .cause(requested)
                    .run(req.run.clone()),
                )?;
            }
            let result = if fails {
                Err(ProtocolError::new(
                    ErrorCode::ActionFailed,
                    "no such output",
                ))
            } else {
                Ok(None)
            };
            let entry = self.journal.append(
                NewEntry::new(
                    Origin::conn(req.conn),
                    Record::ActionResult(ActionResultRecord {
                        call: call.clone(),
                        ok: result.is_ok(),
                        result: None,
                        error: result.clone().err(),
                        duration_ms: self.delay.as_millis() as u64,
                    }),
                )
                .cause(requested)
                .run(req.run),
            )?;
            Ok(ActionOutcome {
                call,
                requested,
                verdict: Verdict::Dispatch,
                result_seq: Some(entry.seq),
                result: Some(result),
            })
        })
    }
}

struct World {
    shared: Shared,
    fake: Arc<FakeCore>,
    runtime: Runtime,
}

impl World {
    fn new() -> Self {
        Self::with_delay(Duration::from_millis(10))
    }

    fn with_delay(delay: Duration) -> Self {
        let shared = Shared::new(Journal::in_memory(10_000));
        let fake = Arc::new(FakeCore {
            journal: shared.journal.clone(),
            delay,
            failing: Mutex::new(BTreeSet::new()),
        });
        let runtime = Runtime::start(shared.clone(), fake.clone(), Arc::new(QuickJs::default()));
        World {
            shared,
            fake,
            runtime,
        }
    }

    fn add(&self, flow: serde_json::Value) -> Flow {
        let flow = Flow::parse(&flow).unwrap();
        self.runtime.set_flow(flow.clone());
        flow
    }

    fn fire(&self, flow: &str) -> FireOutcome {
        self.runtime.fire_api(flow, user()).unwrap().outcome
    }

    fn start(&self, flow: &str) -> String {
        match self.fire(flow) {
            FireOutcome::Started(run) => run,
            other => panic!("expected a start, got {other:?}"),
        }
    }

    /// Waits (on the paused clock) until `run` has finished.
    async fn finished(&self, run: &str) -> RunInfo {
        for _ in 0..10_000 {
            if let Some(info) = self.runtime.run(run)
                && info.finished.is_some()
            {
                return info;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("run {run} never finished");
    }

    /// Lets every task that can make progress do so.
    async fn settle(&self) {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    fn entries(&self) -> Vec<Arc<Entry>> {
        self.shared.journal.range(1, usize::MAX).unwrap()
    }

    fn entries_of_run(&self, run: &str) -> Vec<String> {
        self.entries()
            .iter()
            .filter(|e| e.run.as_deref() == Some(run))
            .map(|e| describe(e))
            .collect()
    }

    fn why(&self, seq: Seq) -> Vec<String> {
        self.shared
            .journal
            .why(seq)
            .unwrap()
            .iter()
            .map(|e| e.record.kind().to_owned())
            .collect()
    }

    fn state(&self, slug: &str, rest: &[&str]) -> Option<serde_json::Value> {
        let mut path = vec![PathSegment::Key(slug.into())];
        path.extend(rest.iter().map(|s| PathSegment::Key((*s).into())));
        self.shared
            .state
            .value(&StatePath::new(STATE_ROOT, path))
            .map(|v| serde_json::to_value(v).unwrap())
    }

    fn last_of(&self, pred: impl Fn(&Entry) -> bool) -> Arc<Entry> {
        self.entries()
            .into_iter()
            .rev()
            .find(|e| pred(e))
            .expect("a matching entry")
    }
}

fn user() -> Origin {
    Origin::User {
        user: "test".into(),
        client: "test".into(),
    }
}

/// `kind`, with the step and status for `run.step` and the status for `run.finished`.
fn describe(entry: &Entry) -> String {
    match &entry.record {
        Record::RunStep(step) => format!("run.step {} {:?}", step.step, step.status).to_lowercase(),
        Record::RunFinished(done) => format!("run.finished {:?}", done.status).to_lowercase(),
        other => other.kind().to_owned(),
    }
}

fn route(id: &str, conn: &str, output: i64, input: i64) -> serde_json::Value {
    json!({
        "id": id, "kind": "action", "target": { "conn": conn },
        "action": "route", "params": { "output": output, "input": input },
    })
}

fn wait(id: &str, duration: &str) -> serde_json::Value {
    json!({ "id": id, "kind": "wait", "for": duration })
}

fn api_flow(id: &str, concurrency: &str, body: serde_json::Value) -> serde_json::Value {
    json!({
        "id": id, "name": id, "slug": id, "enabled": true,
        "triggers": [{ "kind": "api" }],
        "concurrency": concurrency, "body": body,
    })
}

#[tokio::test(start_paused = true)]
async fn a_run_journals_every_step_and_why_reaches_the_trigger() {
    let world = World::new();
    world.add(api_flow(
        "routeTwice",
        "parallel",
        json!([
            route("route1", VH, 1, 3),
            wait("wait1", "1s"),
            route("route2", VH, 1, 4)
        ]),
    ));
    assert_eq!(world.state("routeTwice", &["running"]), Some(json!(0)));

    let run = world.start("routeTwice");
    world.settle().await;
    assert_eq!(world.state("routeTwice", &["running"]), Some(json!(1)));
    let info = world.finished(&run).await;

    assert_eq!(info.status, RunPhase::Ok);
    assert!(info.duration_ms.unwrap() >= 1_000, "{info:?}");
    assert_eq!(
        world.entries_of_run(&run),
        [
            "run.started",
            "run.step route1 started",
            "action.requested",
            "event",
            "action.result",
            "run.step route1 done",
            "run.step wait1 started",
            "run.step wait1 done",
            "run.step route2 started",
            "action.requested",
            "event",
            "action.result",
            "run.step route2 done",
            "run.finished ok",
        ]
    );
    let result = world.last_of(|e| matches!(e.record, Record::ActionResult(_)));
    assert_eq!(
        world.why(result.seq),
        [
            "action.result",
            "action.requested",
            "run.step",
            "run.started",
            "trigger.matched"
        ]
    );
    let Origin::Flow { step, .. } = &world
        .last_of(|e| matches!(e.record, Record::ActionRequested(_)))
        .origin
    else {
        panic!("actions from a run have a flow origin");
    };
    assert_eq!(step.as_deref(), Some("route2"));

    assert_eq!(world.state("routeTwice", &["running"]), Some(json!(0)));
    let last = world.state("routeTwice", &["lastRun"]).unwrap();
    assert_eq!(last["status"], "ok");
    assert!(last["durationMs"].as_u64().unwrap() >= 1_000);
    assert!(last.get("error").is_none());
}

#[tokio::test(start_paused = true)]
async fn cancelling_during_a_wait_stops_the_run() {
    let world = World::new();
    world.add(api_flow(
        "f",
        "parallel",
        json!([wait("wait1", "10s"), route("route1", VH, 1, 3)]),
    ));
    let run = world.start("f");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(world.runtime.cancel(&run).unwrap(), RunPhase::Running);
    let info = world.finished(&run).await;

    assert_eq!(info.status, RunPhase::Cancelled);
    assert!(info.duration_ms.unwrap() < 2_000);
    assert_eq!(
        world.entries_of_run(&run),
        [
            "run.started",
            "run.step wait1 started",
            "run.step wait1 failed",
            "run.finished cancelled"
        ]
    );
    assert!(world.runtime.cancel(&run).is_err(), "already finished");
    assert_eq!(
        world.state("f", &["lastRun", "status"]),
        Some(json!("cancelled"))
    );
}

#[tokio::test(start_paused = true)]
async fn a_cancel_waits_for_an_action_in_flight() {
    let world = World::with_delay(Duration::from_secs(2));
    world.add(api_flow(
        "f",
        "parallel",
        json!([route("route1", VH, 1, 3), route("route2", VH, 1, 4)]),
    ));
    let run = world.start("f");
    tokio::time::sleep(Duration::from_millis(500)).await;
    world.runtime.cancel(&run).unwrap();
    let info = world.finished(&run).await;
    assert_eq!(info.status, RunPhase::Cancelled);
    assert_eq!(
        world.entries_of_run(&run),
        [
            "run.started",
            "run.step route1 started",
            "action.requested",
            "event",
            "action.result",
            "run.step route1 done",
            "run.finished cancelled",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_failing_action_fails_the_run_unless_on_error_continues() {
    let world = World::new();
    world.fake.failing.lock().unwrap().insert("route".into());
    world.add(api_flow(
        "fail",
        "parallel",
        json!([route("route1", VH, 99, 1), wait("wait1", "1s")]),
    ));
    let mut tolerant = route("route1", VH, 99, 1);
    tolerant["onError"] = json!("continue");
    world.add(api_flow(
        "carryOn",
        "parallel",
        json!([tolerant, wait("wait1", "1s")]),
    ));

    let failed = world.finished(&world.start("fail")).await;
    assert_eq!(failed.status, RunPhase::Failed);
    let error = failed.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("action_failed", "no such output")
    );
    assert_eq!(
        world.state("fail", &["lastRun", "error", "code"]),
        Some(json!("action_failed"))
    );

    let run = world.start("carryOn");
    let carried = world.finished(&run).await;
    assert_eq!(carried.status, RunPhase::Ok);
    assert!(
        world
            .entries_of_run(&run)
            .contains(&"run.step wait1 done".to_owned())
    );
}

#[tokio::test(start_paused = true)]
async fn timeouts_fail_the_step_but_the_late_result_is_journaled() {
    let world = World::with_delay(Duration::from_secs(5));
    let mut slow = route("route1", VH, 1, 3);
    slow["timeout"] = json!("1s");
    world.add(api_flow("slow", "parallel", json!([slow])));

    let run = world.start("slow");
    let info = world.finished(&run).await;
    assert_eq!(info.status, RunPhase::Failed);
    assert_eq!(info.error.unwrap().code, "timeout");
    assert!(info.duration_ms.unwrap() < 2_000);

    tokio::time::sleep(Duration::from_secs(5)).await;
    let kinds = world.entries_of_run(&run);
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("action.result"),
        "{kinds:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn unavailable_connections_and_disabled_steps() {
    let world = World::new();
    let mut off = wait("off", "1h");
    off["enabled"] = json!(false);
    world.add(api_flow(
        "f",
        "parallel",
        json!([off, route("gone", GONE, 1, 1)]),
    ));

    let run = world.start("f");
    let info = world.finished(&run).await;
    assert_eq!(info.error.unwrap().code, "unavailable");
    assert_eq!(
        world.entries_of_run(&run),
        [
            "run.started",
            "run.step off skipped",
            "run.step gone started",
            "run.step gone failed",
            "run.finished failed"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn parallel_starts_another_run() {
    let world = World::new();
    world.add(api_flow("f", "parallel", json!([wait("wait1", "1s")])));
    let first = world.start("f");
    let second = world.start("f");
    world.settle().await;
    assert_eq!(world.state("f", &["running"]), Some(json!(2)));
    let running: Vec<RunPhase> = world
        .runtime
        .runs(Some("f"))
        .iter()
        .map(|r| r.status)
        .collect();
    assert_eq!(running, [RunPhase::Running, RunPhase::Running]);
    world.finished(&first).await;
    world.finished(&second).await;
    assert_eq!(world.state("f", &["running"]), Some(json!(0)));
}

#[tokio::test(start_paused = true)]
async fn ignore_blocks_a_trigger_while_a_run_is_active() {
    let world = World::new();
    world.add(api_flow("f", "ignore", json!([wait("wait1", "1s")])));
    let first = world.start("f");
    assert_eq!(world.fire("f"), FireOutcome::Blocked(BlockReason::Ignore));
    let matched = world.last_of(|e| matches!(e.record, Record::TriggerMatched(_)));
    let Record::TriggerMatched(record) = &matched.record else {
        unreachable!()
    };
    assert_eq!(
        (record.blocked, record.reason),
        (Some(true), Some(BlockReason::Ignore))
    );

    world.finished(&first).await;
    world.start("f");
}

#[tokio::test(start_paused = true)]
async fn queue_runs_one_after_another_up_to_the_limit() {
    let world = World::new();
    world.add(api_flow("f", "queue", json!([wait("wait1", "1s")])));
    let first = world.start("f");
    let mut queued = Vec::new();
    for _ in 0..QUEUE_LIMIT {
        match world.fire("f") {
            FireOutcome::Queued(run) => queued.push(run),
            other => panic!("expected a queued run, got {other:?}"),
        }
    }
    assert_eq!(
        world.fire("f"),
        FireOutcome::Blocked(BlockReason::QueueFull)
    );

    // Cancelling a queued run drops it without starting it.
    let dropped = queued.pop().unwrap();
    assert_eq!(world.runtime.cancel(&dropped).unwrap(), RunPhase::Cancelled);
    assert_eq!(world.entries_of_run(&dropped), ["run.finished cancelled"]);

    let second = queued.remove(0);
    let first_info = world.finished(&first).await;
    let second_info = world.finished(&second).await;
    assert!(second_info.started.unwrap() > first_info.finished.unwrap());
    for run in &queued {
        world.finished(run).await;
    }
    assert!(
        world
            .runtime
            .runs(Some("f"))
            .iter()
            .all(|r| r.finished.is_some())
    );
}

#[tokio::test(start_paused = true)]
async fn restart_cancels_the_active_run_first() {
    let world = World::new();
    world.add(api_flow("f", "restart", json!([wait("wait1", "10s")])));
    let first = world.start("f");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let second = world.start("f");

    let first_info = world.finished(&first).await;
    assert_eq!(first_info.status, RunPhase::Cancelled);
    world.settle().await;
    let second_info = world.runtime.run(&second).unwrap();
    assert!(second_info.started.unwrap() > first_info.finished.unwrap());
    assert_eq!(world.finished(&second).await.status, RunPhase::Ok);
}

#[tokio::test(start_paused = true)]
async fn event_triggers_start_runs_and_why_walks_back_through_both() {
    let world = World::new();
    world.add(api_flow(
        "source",
        "parallel",
        json!([route("route1", VH, 1, 3)]),
    ));
    world.add(json!({
        "id": "follow", "name": "Follow", "slug": "follow", "enabled": true,
        "triggers": [{ "kind": "event", "source": { "conn": VH }, "event": "routeChanged" }],
        "concurrency": "queue",
        "body": [route("follow1", VH2, 1, 2)],
    }));

    let source = world.start("source");
    world.finished(&source).await;
    let follower = loop {
        if let Some(run) = world.runtime.runs(Some("follow")).first() {
            break run.run.clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(world.finished(&follower).await.status, RunPhase::Ok);

    let result = world.last_of(|e| {
        e.run.as_deref() == Some(&follower) && matches!(e.record, Record::ActionResult(_))
    });
    assert_eq!(
        world.why(result.seq),
        [
            "action.result",
            "action.requested",
            "run.step",
            "run.started",
            "trigger.matched",
            "event",
            "action.requested",
            "run.step",
            "run.started",
            "trigger.matched",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn the_loop_guard_blocks_a_flow_that_triggers_itself() {
    let world = World::new();
    world.add(json!({
        "id": "echo", "name": "Echo", "slug": "echo", "enabled": true,
        "triggers": [
            { "kind": "api" },
            { "kind": "event", "source": { "conn": VH }, "event": "routeChanged" },
        ],
        "concurrency": "parallel",
        "body": [route("route1", VH, 2, 2)],
    }));
    let run = world.start("echo");
    world.finished(&run).await;
    world.settle().await;

    let matches: Vec<(u32, Option<BlockReason>)> = world
        .entries()
        .iter()
        .filter_map(|e| match &e.record {
            Record::TriggerMatched(m) => Some((m.trigger, m.reason)),
            _ => None,
        })
        .collect();
    assert_eq!(matches, [(0, None), (1, Some(BlockReason::Loop))]);
    assert_eq!(world.runtime.runs(Some("echo")).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn disabled_flows_and_flows_without_an_api_trigger_refuse_api_runs() {
    let world = World::new();
    let mut off = api_flow("off", "parallel", json!([]));
    off["enabled"] = json!(false);
    world.add(off);
    world.add(json!({
        "id": "noApi", "name": "x", "slug": "noApi", "enabled": true,
        "concurrency": "parallel", "body": [],
    }));
    assert!(world.runtime.fire_api("off", user()).is_err());
    assert!(world.runtime.fire_api("noApi", user()).is_err());
    assert!(world.runtime.fire_api("missing", user()).is_err());
    assert!(world.entries().is_empty(), "refusals aren't journaled");
}

#[tokio::test(start_paused = true)]
async fn removing_a_flow_cancels_its_runs_and_deletes_its_state() {
    let world = World::new();
    world.add(api_flow("f", "queue", json!([wait("wait1", "10s")])));
    let running = world.start("f");
    let FireOutcome::Queued(queued) = world.fire("f") else {
        panic!("expected a queued run");
    };
    world.runtime.remove_flow("f").unwrap();
    assert_eq!(world.state("f", &[]), None);

    assert_eq!(world.finished(&running).await.status, RunPhase::Cancelled);
    assert_eq!(world.finished(&queued).await.status, RunPhase::Cancelled);
    world.settle().await;
    assert_eq!(
        world.state("f", &[]),
        None,
        "finishing runs don't bring it back"
    );
}

#[tokio::test(start_paused = true)]
async fn renaming_a_slug_moves_the_state() {
    let world = World::new();
    let mut flow = world.add(api_flow("before", "parallel", json!([])));
    assert_eq!(world.state("before", &["running"]), Some(json!(0)));
    flow.slug = "after".into();
    world.runtime.set_flow(flow);
    assert_eq!(world.state("before", &[]), None);
    assert_eq!(world.state("after", &["running"]), Some(json!(0)));
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_active_runs() {
    let world = World::new();
    world.add(api_flow("f", "parallel", json!([wait("wait1", "1h")])));
    let run = world.start("f");
    world.settle().await;
    let shared = world.shared.clone();
    world.runtime.shutdown().await;
    let last = shared.journal.range(1, usize::MAX).unwrap().pop().unwrap();
    assert_eq!(describe(&last), "run.finished cancelled");
    assert_eq!(last.run.as_deref(), Some(run.as_str()));
}

// ---------------------------------------------------------------------------
// Expressions (M4 Phase 2)

impl World {
    /// Sets a state leaf the way a plugin's patch does: journaled, then applied.
    fn set_state(&self, conn: &str, path: &[&str], value: Value) -> Seq {
        let at = StatePath::new(
            conn,
            path.iter()
                .map(|s| match s.parse() {
                    Ok(i) => PathSegment::Index(i),
                    Err(_) => PathSegment::Key((*s).into()),
                })
                .collect(),
        );
        let journal = self.shared.journal.clone();
        self.shared.state.mutate(|tree| {
            let seq = journal
                .append(NewEntry::new(
                    Origin::conn(conn),
                    Record::StatePatch(StatePatchRecord {
                        ops: vec![JournalOp::Set {
                            path: at.clone(),
                            value: value.clone(),
                        }],
                        replaces: None,
                        device_ts: None,
                    }),
                ))
                .unwrap()
                .seq;
            let meta = Meta {
                ts: 1,
                seq,
                quality: Quality::Good,
                source: Source::Conn(conn.into()),
            };
            (seq, tree.set(&at, &value, &meta))
        })
    }

    fn var(&self, name: &str) -> Option<serde_json::Value> {
        self.shared
            .state
            .value(&StatePath::new("vars", vec![PathSegment::Key(name.into())]))
            .map(|v| serde_json::to_value(v).unwrap())
    }

    fn emit(&self, conn: &str, payload: serde_json::Value) {
        self.shared
            .journal
            .append(NewEntry::new(
                Origin::conn(conn),
                Record::Event(EventRecord {
                    source: Source::Conn(conn.into()),
                    event: "routeChanged".into(),
                    payload: Some(covalence_protocol::to_value(&payload).unwrap()),
                }),
            ))
            .unwrap();
    }

    fn matched(&self) -> Vec<TriggerMatchedRecord> {
        self.entries()
            .iter()
            .filter_map(|e| match &e.record {
                Record::TriggerMatched(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    fn step_detail(&self, run: &str, step: &str, status: StepStatus) -> StepDetail {
        self.entries()
            .iter()
            .find_map(|e| match &e.record {
                Record::RunStep(s)
                    if e.run.as_deref() == Some(run) && s.step == step && s.status == status =>
                {
                    s.detail.clone()
                }
                _ => None,
            })
            .expect("a matching run.step")
    }

    fn requested_params(&self) -> Vec<serde_json::Value> {
        self.entries()
            .iter()
            .filter_map(|e| match &e.record {
                Record::ActionRequested(r) => Some(serde_json::to_value(&r.params).unwrap()),
                _ => None,
            })
            .collect()
    }
}

fn set(id: &str, path: &str, value: &str) -> serde_json::Value {
    json!({ "id": id, "kind": "set", "path": path, "value": value })
}

#[tokio::test(start_paused = true)]
async fn expressions_read_vars_locals_and_earlier_steps() {
    let world = World::new();
    world.set_state("vars", &["nextSource"], Value::from(5));
    world.add(api_flow(
        "pick",
        "parallel",
        json!([
            set("remember", "local.input", "vars.nextSource"),
            {
                "id": "route1", "kind": "action", "target": { "conn": VH },
                "action": "route", "params": { "output": 1, "input": { "expr": "local.input" } },
            },
            set("note", "vars.lastStatus", "steps.route1.status + ':' + steps.remember.status"),
            { "id": "pause", "kind": "wait", "for": { "expr": "steps.route1.durationMs + 250" } },
        ]),
    ));
    let run = world.start("pick");
    let info = world.finished(&run).await;
    assert_eq!(info.status, RunPhase::Ok, "{info:?}");
    assert_eq!(
        world.requested_params(),
        [json!({ "output": 1, "input": 5 })]
    );
    assert_eq!(world.var("lastStatus"), Some(json!("done:done")));

    // The variable's patch walks back to the run.
    let patch = world.last_of(|e| matches!(e.record, Record::StatePatch(_)) && e.run.is_some());
    assert_eq!(
        world.why(patch.seq),
        ["state.patch", "run.step", "run.started", "trigger.matched"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_failing_expression_fails_its_step() {
    let world = World::new();
    world.add(api_flow(
        "f",
        "parallel",
        json!([{
            "id": "route1", "kind": "action", "target": { "conn": VH },
            "action": "route", "params": { "output": 1, "input": { "expr": "nothing.here" } },
        }]),
    ));
    let run = world.start("f");
    let info = world.finished(&run).await;
    assert_eq!(info.status, RunPhase::Failed);
    let error = info.error.unwrap();
    assert_eq!(error.code, "exprError", "{error:?}");
    assert!(world.requested_params().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_branch_runs_the_first_truthy_arm() {
    let world = World::new();
    let flow = |id: &str| {
        api_flow(
            id,
            "parallel",
            json!([{
                "id": "pick", "kind": "branch",
                "arms": [
                    { "when": "vars.n == 1", "steps": [route("one", VH, 1, 1)] },
                    { "when": "vars.n == 2", "steps": [route("two", VH, 1, 2)] },
                ],
                "else": [wait("rest", "1s")],
            }]),
        )
    };
    world.add(flow("b"));
    world.set_state("vars", &["n"], Value::from(2));
    let run = world.start("b");
    assert_eq!(world.finished(&run).await.status, RunPhase::Ok);
    assert_eq!(
        world.entries_of_run(&run),
        [
            "run.started",
            "run.step pick started",
            "run.step two started",
            "action.requested",
            "event",
            "action.result",
            "run.step two done",
            "run.step pick done",
            "run.finished ok",
        ]
    );
    assert_eq!(
        world.step_detail(&run, "two", StepStatus::Done).position,
        ["pick", "two"]
    );
    assert_eq!(
        world.step_detail(&run, "pick", StepStatus::Done).arm,
        Some(json!(1))
    );

    world.set_state("vars", &["n"], Value::from(9));
    let run = world.start("b");
    world.finished(&run).await;
    assert_eq!(
        world.step_detail(&run, "pick", StepStatus::Done).arm,
        Some(json!("else"))
    );
}

#[tokio::test(start_paused = true)]
async fn a_falsy_condition_blocks_the_run() {
    let world = World::new();
    let mut flow = api_flow("c", "parallel", json!([wait("w", "1s")]));
    flow["conditions"] = json!(["true", "vars.armed"]);
    world.add(flow);
    assert_eq!(
        world.fire("c"),
        FireOutcome::Blocked(BlockReason::Condition)
    );
    let matched = world.matched().pop().unwrap();
    assert_eq!(
        (matched.reason, matched.condition),
        (Some(BlockReason::Condition), Some(1))
    );

    world.set_state("vars", &["armed"], Value::from(true));
    assert!(matches!(world.fire("c"), FireOutcome::Started(_)));
}

#[tokio::test(start_paused = true)]
async fn an_event_filter_picks_events_and_a_false_one_journals_nothing() {
    let world = World::new();
    world.add(json!({
        "id": "f", "name": "f", "slug": "f", "enabled": true,
        "triggers": [{
            "kind": "event", "source": { "conn": VH }, "event": "routeChanged",
            "filter": "trigger.payload?.output == 1",
        }],
        "concurrency": "parallel",
        "body": [wait("w", "1s")],
    }));
    world.emit(VH, json!({ "output": 2 }));
    world.settle().await;
    assert!(world.matched().is_empty());
    world.emit(VH, json!({ "output": 1 }));
    world.settle().await;
    assert_eq!(world.matched().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_state_trigger_fires_on_its_edge_only() {
    let world = World::new();
    world
        .runtime
        .set_aliases([("vh".to_owned(), VH.to_owned())].into());
    world.set_state(VH, &["outputs", "1", "source"], Value::from(3));
    world.add(json!({
        "id": "s", "name": "s", "slug": "s", "enabled": true,
        "triggers": [{ "kind": "state", "when": "vh.outputs[1].source == 3", "edge": "rising" }],
        "concurrency": "parallel",
        "body": [{ "id": "seen", "kind": "set", "path": "local.v", "value": "trigger.value" }],
    }));
    // Already true when loaded: the baseline never fires.
    world.settle().await;
    assert!(world.matched().is_empty());

    world.set_state(VH, &["outputs", "1", "source"], Value::from(4));
    world.settle().await;
    let patch = world.set_state(VH, &["outputs", "1", "source"], Value::from(3));
    world.settle().await;
    let matched = world.last_of(|e| matches!(e.record, Record::TriggerMatched(_)));
    assert_eq!(matched.cause, Some(patch));
    // An unrelated change, or the same value again, doesn't fire.
    world.set_state(VH, &["outputs", "0", "source"], Value::from(9));
    world.set_state(VH, &["outputs", "1", "source"], Value::from(3));
    world.settle().await;
    assert_eq!(world.matched().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_state_trigger_on_what_its_flow_writes_is_a_loop() {
    let world = World::new();
    world.add(json!({
        "id": "l", "name": "l", "slug": "l", "enabled": true,
        "triggers": [
            { "kind": "state", "when": "vars.count", "edge": "change" },
            { "kind": "api" },
        ],
        "concurrency": "parallel",
        "body": [set("bump", "vars.count", "(vars.count ?? 0) + 1")],
    }));
    let run = world.start("l");
    world.finished(&run).await;
    world.settle().await;
    let matched = world.matched();
    assert_eq!(matched.len(), 2, "{matched:?}");
    assert_eq!(matched[1].reason, Some(BlockReason::Loop));
    assert_eq!(world.var("count"), Some(json!(1)));
}

#[tokio::test(start_paused = true)]
async fn wait_until_waits_for_a_change_or_times_out() {
    let world = World::new();
    world.add(api_flow(
        "w",
        "parallel",
        json!([
            { "id": "ready", "kind": "waitUntil", "until": "vars.ready", "timeout": "10s" },
            set("got", "local.v", "steps.ready.result.value"),
        ]),
    ));
    let run = world.start("w");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(world.runtime.run(&run).unwrap().status, RunPhase::Running);
    world.set_state("vars", &["ready"], Value::from("yes"));
    let info = world.finished(&run).await;
    assert_eq!(info.status, RunPhase::Ok);
    assert!(info.duration_ms.unwrap() < 3_000, "{info:?}");

    // Already true: done at once. Never true: the timeout.
    let run = world.start("w");
    assert_eq!(world.finished(&run).await.status, RunPhase::Ok);
    world.set_state("vars", &["ready"], Value::from(false));
    let run = world.start("w");
    let info = world.finished(&run).await;
    assert_eq!(info.error.map(|e| e.code), Some("timeout".into()));
}

#[tokio::test(start_paused = true)]
async fn cancelling_during_wait_until_stops_the_run() {
    let world = World::new();
    world.add(api_flow(
        "w",
        "parallel",
        json!([{ "id": "ready", "kind": "waitUntil", "until": "vars.never" }]),
    ));
    let run = world.start("w");
    tokio::time::sleep(Duration::from_secs(1)).await;
    world.runtime.cancel(&run).unwrap();
    assert_eq!(world.finished(&run).await.status, RunPhase::Cancelled);
    assert_eq!(
        world
            .step_detail(&run, "ready", StepStatus::Failed)
            .error
            .map(|e| e.code),
        Some("cancelled".into())
    );
}
