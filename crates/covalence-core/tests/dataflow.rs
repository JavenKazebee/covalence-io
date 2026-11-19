//! Cells and the graph (dataflow spec §5).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use covalence_core::Shared;
use covalence_core::dataflow::{CellDef, Clock, Dataflow, JournalMode};
use covalence_core::journal::{
    CoreOrigin, EventRecord, Journal, JournalOp, NewEntry, Origin, Record, Seq, StatePatchRecord,
};
use covalence_core::source::Source;
use covalence_core::state::{Meta, Quality, ReducerMeta, Source as Setter, StatePath};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::{Value as Json, json};

fn path(conn: &str, keys: &[&str]) -> StatePath {
    StatePath::new(
        conn,
        keys.iter().map(|k| PathSegment::Key((*k).into())).collect(),
    )
}

fn var(name: &str) -> StatePath {
    path("vars", &[name])
}

fn shared() -> Shared {
    Shared::new(Journal::in_memory(10_000))
}

struct Time(Arc<AtomicU64>);

impl Time {
    fn new() -> (Self, Clock) {
        let t = Arc::new(AtomicU64::new(1_000));
        let c = t.clone();
        (Time(t), Arc::new(move || c.load(Ordering::SeqCst)))
    }

    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

/// A connection's patch, journaled and applied as the host does.
fn set(shared: &Shared, at: &StatePath, value: Value) -> Seq {
    let journal = &shared.journal;
    shared.state.mutate(|tree| {
        let seq = journal
            .append(NewEntry::new(
                Origin::conn(at.conn.clone()),
                Record::StatePatch(StatePatchRecord {
                    ops: vec![JournalOp::Set {
                        path: at.clone(),
                        value: value.clone(),
                        reducer: None,
                    }],
                    replaces: None,
                    device_ts: None,
                }),
            ))
            .unwrap()
            .seq;
        let meta = Meta {
            ts: 0,
            seq,
            quality: Quality::Good,
            source: Setter::Conn(at.conn.clone()),
        };
        (seq, tree.set(at, &value, &meta))
    })
}

/// A plugin's event at `at`.
fn event(shared: &Shared, at: &StatePath, name: &str) -> Seq {
    shared
        .journal
        .append(NewEntry::new(
            Origin::conn(at.conn.clone()),
            Record::Event(EventRecord {
                conn: Some(at.conn.clone()),
                ..EventRecord::at(at.clone(), name, None)
            }),
        ))
        .unwrap()
        .seq
}

fn value(shared: &Shared, at: &StatePath) -> Option<Json> {
    shared
        .state
        .value(at)
        .as_ref()
        .map(covalence_core::expr::to_json)
}

fn cell(id: &str, home: StatePath, source: Json) -> CellDef {
    CellDef {
        id: id.into(),
        home,
        source: Source::from_json(source).unwrap(),
        journal: JournalMode::Full,
        to: Vec::new(),
        control: None,
        values: Vec::new(),
    }
}

fn expr(e: &str) -> Json {
    json!({ "expr": e })
}

/// The latest dataflow patch at `at`.
fn last_patch(shared: &Shared, at: &StatePath) -> Option<Arc<covalence_core::journal::Entry>> {
    let all = shared.journal.range(1, 100_000).unwrap();
    all.into_iter().rev().find(|e| {
        e.origin
            == Origin::Core {
                core: CoreOrigin::Dataflow,
            }
            && matches!(&e.record, Record::StatePatch(p)
                if p.ops.iter().any(|op| matches!(op,
                    JournalOp::Set { path, .. } | JournalOp::Delete { path, .. } if path == at)))
    })
}

fn aliases(df: &Dataflow) {
    df.set_aliases([("vh".to_string(), "c1".to_string())].into());
}

#[test]
fn a_computed_cell_settles_in_the_turn_of_its_input() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    df.put(
        cell("variable:double", var("double"), expr("vh.level * 2")),
        None,
    );
    let mut feed = shared.state.subscribe();

    let level = path("c1", &["level"]);
    let seq = set(&shared, &level, Value::from(4));
    // One published turn has the input and the cell both.
    let dirty = feed.try_recv().unwrap();
    assert!(
        dirty.contains(&level) && dirty.contains(&var("double")),
        "{dirty:?}"
    );
    assert!(feed.try_recv().is_err());
    assert_eq!(value(&shared, &var("double")), Some(json!(8)));

    // Journaled from the dataflow, caused by the input's patch.
    let patch = last_patch(&shared, &var("double")).unwrap();
    assert_eq!(patch.cause, Some(seq));
    let why = shared.journal.why(patch.seq).unwrap();
    assert_eq!(why.last().unwrap().seq, seq);
    let leaf = shared.state.leaf(&var("double")).unwrap();
    assert_eq!((leaf.meta.seq, leaf.meta.source), (patch.seq, Setter::Core));
}

#[test]
fn cells_evaluate_in_dependency_order_whatever_order_they_were_added_in() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    // c reads b reads a reads the connection, added backwards.
    df.put(cell("variable:c", var("c"), expr("vars.b + 1")), None);
    df.put(cell("variable:b", var("b"), expr("vars.a * 10")), None);
    df.put(cell("variable:a", var("a"), expr("vh.level")), None);
    let before = shared.journal.last_seq().unwrap_or(0);
    set(&shared, &path("c1", &["level"]), Value::from(2));
    assert_eq!(value(&shared, &var("c")), Some(json!(21)));
    assert!(df.problems().is_empty(), "{:?}", df.problems());
    // Once each, in order: no cell was written twice.
    let patches: Vec<_> = shared
        .journal
        .range(before + 1, 100)
        .unwrap()
        .into_iter()
        .filter(|e| matches!(e.origin, Origin::Core { .. }))
        .collect();
    assert_eq!(patches.len(), 3, "{patches:#?}");
}

#[test]
fn a_cycle_is_reported_on_both_cells_and_the_rest_of_the_turn_goes_on() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    df.put(
        cell("variable:other", var("other"), expr("vh.level + 100")),
        None,
    );
    df.put(
        cell("variable:a", var("a"), expr("(vars.b ?? 0) + vh.level")),
        None,
    );
    df.put(
        cell("variable:b", var("b"), expr("(vars.a ?? 0) + 1")),
        None,
    );
    let a_before = value(&shared, &var("a"));
    let b_before = value(&shared, &var("b"));
    set(&shared, &path("c1", &["level"]), Value::from(5));
    let problems = df.problems();
    assert_eq!(problems["variable:a"].code, "cycle", "{problems:?}");
    assert_eq!(problems["variable:b"].code, "cycle");
    assert!(!problems.contains_key("variable:other"));
    assert_eq!(value(&shared, &var("other")), Some(json!(105)));
    // Both keep what they had before the turn.
    assert_eq!(value(&shared, &var("a")), a_before);
    assert_eq!(value(&shared, &var("b")), b_before);
}

#[tokio::test]
async fn a_toggle_latches_on_a_plugin_event_without_a_surface() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let contact = path("c1", &["inputs", "gpio3"]);
    df.put(
        cell(
            "variable:armed",
            var("armed"),
            json!({ "use": "toggle", "on": { "from": { "path": { "match": "vh.inputs.*" } } } }),
        ),
        None,
    );
    assert_eq!(value(&shared, &var("armed")), Some(json!(false)));
    let mut journal = shared.journal.subscribe();

    let press = event(&shared, &contact, "press");
    df.flush().await;
    assert_eq!(value(&shared, &var("armed")), Some(json!(true)));
    let patch = last_patch(&shared, &var("armed")).unwrap();
    assert_eq!(patch.cause, Some(press));
    // Its own event happens at its cell, after its patch.
    let mut on = None;
    while let Ok(e) = journal.try_recv() {
        if let Record::Event(ev) = &e.record
            && ev.path == var("armed")
        {
            on = Some((ev.event.clone(), e.cause));
        }
    }
    assert_eq!(on, Some(("on".to_string(), Some(patch.seq))));

    // Other names don't step it; another press flips it back.
    event(&shared, &contact, "release");
    event(&shared, &contact, "press");
    df.flush().await;
    assert_eq!(value(&shared, &var("armed")), Some(json!(false)));
}

#[test]
fn a_position_follows_its_input_and_options_apply_from_the_next_step() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let detents = path("c1", &["detents"]);
    set(&shared, &detents, Value::from(0));
    set(&shared, &var("top"), Value::from(10));
    df.put(
        cell(
            "variable:pos",
            var("pos"),
            json!({ "use": "position", "input": { "ref": { "conn": "c1", "path": ["detents"] } },
                    "max": { "ref": { "root": "vars", "path": ["top"] } } }),
        ),
        None,
    );
    assert_eq!(value(&shared, &var("pos")), Some(json!(0)));
    set(&shared, &detents, Value::from(4));
    assert_eq!(value(&shared, &var("pos")), Some(json!(4)));
    // A smaller max doesn't move it until it next steps.
    set(&shared, &var("top"), Value::from(2));
    assert_eq!(value(&shared, &var("pos")), Some(json!(4)));
    set(&shared, &detents, Value::from(5));
    assert_eq!(value(&shared, &var("pos")), Some(json!(2)));
}

#[test]
fn a_velocity_decays_by_its_wakes() {
    let shared = shared();
    let (time, clock) = Time::new();
    let df = Dataflow::with_clock(&shared, clock);
    let detents = path("c1", &["detents"]);
    set(&shared, &detents, Value::from(0));
    df.put(
        cell(
            "variable:speed",
            var("speed"),
            json!({ "use": "velocity", "input": { "ref": { "conn": "c1", "path": ["detents"] } },
                    "window": 100, "decay": 50 }),
        ),
        None,
    );
    time.advance(10);
    set(&shared, &detents, Value::from(3));
    assert_eq!(value(&shared, &var("speed")), Some(json!(30)));
    let meta = shared
        .state
        .read(|t| t.reducer(&var("speed")).cloned())
        .unwrap();
    assert_eq!(meta.transform, "velocity@1");
    assert!(meta.wake_at.is_some() && meta.state.is_some(), "{meta:?}");
    // Past the window it decays, then stops.
    time.advance(150);
    df.tick();
    let decayed = value(&shared, &var("speed")).unwrap().as_f64().unwrap();
    assert!(decayed > 0.0 && decayed < 30.0, "{decayed}");
    time.advance(5_000);
    df.tick();
    assert_eq!(value(&shared, &var("speed")), Some(json!(0)));
    let meta = shared
        .state
        .read(|t| t.reducer(&var("speed")).cloned())
        .unwrap();
    assert_eq!(meta.wake_at, None);
}

#[tokio::test]
async fn an_edit_keeps_state_for_the_same_transform_and_resets_it_otherwise() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let key = path("c1", &["key"]);
    let on = json!({ "from": { "path": { "match": "vh.key" } } });
    df.put(
        cell("variable:t", var("t"), json!({ "use": "toggle", "on": on })),
        None,
    );
    event(&shared, &key, "press");
    df.flush().await;
    assert_eq!(value(&shared, &var("t")), Some(json!(true)));

    // Same transform, new options: kept.
    df.put(
        cell(
            "variable:t",
            var("t"),
            json!({ "use": "toggle@1", "on": on, "initial": false }),
        ),
        Some(1),
    );
    assert_eq!(value(&shared, &var("t")), Some(json!(true)));
    // Renamed: kept, at its new home, and the old one is gone.
    df.put(
        cell(
            "variable:t",
            var("t2"),
            json!({ "use": "toggle", "on": on }),
        ),
        Some(1),
    );
    assert_eq!(value(&shared, &var("t2")), Some(json!(true)));
    assert_eq!(value(&shared, &var("t")), None);

    // Another transform: reset, caused by the edit.
    let edit = event(&shared, &path("c1", &["unrelated"]), "noted");
    df.put(
        cell(
            "variable:t",
            var("t2"),
            json!({ "use": "counter", "on": on }),
        ),
        Some(edit),
    );
    assert_eq!(value(&shared, &var("t2")), Some(json!(0)));
    assert_eq!(last_patch(&shared, &var("t2")).unwrap().cause, Some(edit));
}

#[tokio::test]
async fn a_reducer_resumes_from_its_leaf_and_a_standby_never_steps() {
    // What a standby holds after applying the leader's patches.
    let shared = shared();
    let meta = Meta {
        ts: 0,
        seq: 1,
        quality: Quality::Good,
        source: Setter::Core,
    };
    shared.state.mutate(|tree| {
        let dirty = tree.set(&var("n"), &Value::from(7), &meta);
        tree.set_reducer(
            &var("n"),
            Some(ReducerMeta {
                transform: "counter@1".into(),
                state: None,
                wake_at: None,
            }),
        );
        ((), dirty)
    });
    let df = Dataflow::start(&shared);
    aliases(&df);
    df.set_active(false);
    let key = path("c1", &["key"]);
    df.put(
        cell(
            "variable:n",
            var("n"),
            json!({ "use": "counter", "on": { "from": { "path": { "match": "vh.key" } } } }),
        ),
        None,
    );
    event(&shared, &key, "press");
    df.flush().await;
    assert_eq!(
        value(&shared, &var("n")),
        Some(json!(7)),
        "a standby stepped"
    );

    // Promoted: it resumes from 7, without stepping or writing.
    let before = shared.journal.last_seq();
    df.set_active(true);
    assert_eq!(shared.journal.last_seq(), before);
    event(&shared, &key, "press");
    df.flush().await;
    assert_eq!(value(&shared, &var("n")), Some(json!(8)));
}

#[test]
fn a_sampled_cell_changes_state_every_time_and_journals_once_per_interval() {
    let shared = shared();
    let (time, clock) = Time::new();
    let df = Dataflow::with_clock(&shared, clock);
    aliases(&df);
    let fader = path("c1", &["fader"]);
    df.put(
        CellDef {
            journal: JournalMode::Sampled(Duration::from_millis(100)),
            ..cell("variable:level", var("level"), expr("vh.fader"))
        },
        None,
    );
    let count = |shared: &Shared| {
        shared
            .journal
            .range(1, 100_000)
            .unwrap()
            .iter()
            .filter(|e| matches!(e.origin, Origin::Core { .. }))
            .count()
    };
    set(&shared, &fader, Value::from(1));
    let first = count(&shared);
    for v in 2..6 {
        time.advance(10);
        set(&shared, &fader, Value::from(v));
        assert_eq!(value(&shared, &var("level")), Some(json!(v)));
    }
    assert_eq!(count(&shared), first, "sampled values were journaled");
    // When it settles, the last value is journaled.
    time.advance(100);
    df.tick();
    assert_eq!(count(&shared), first + 1);
    match &last_patch(&shared, &var("level")).unwrap().record {
        Record::StatePatch(p) => {
            assert!(matches!(&p.ops[0], JournalOp::Set { value, .. } if *value == Value::from(5)))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_cell_is_stale_while_what_it_reads_is() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let level = path("c1", &["level"]);
    set(&shared, &level, Value::from(1));
    df.put(cell("variable:copy", var("copy"), expr("vh.level")), None);
    assert_eq!(
        shared.state.leaf(&var("copy")).unwrap().meta.quality,
        Quality::Good
    );
    shared
        .state
        .mutate(|tree| ((), tree.set_quality(&StatePath::root("c1"), Quality::Stale)));
    assert_eq!(
        shared.state.leaf(&var("copy")).unwrap().meta.quality,
        Quality::Stale
    );
}

#[test]
fn a_failing_cell_keeps_its_value_and_a_removed_one_takes_its_value_with_it() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let level = path("c1", &["level"]);
    set(&shared, &level, Value::from(2));
    df.put(
        cell(
            "variable:x",
            var("x"),
            expr("if (vh.level > 5) throw new Error('too high'); return vh.level"),
        ),
        None,
    );
    assert_eq!(value(&shared, &var("x")), Some(json!(2)));
    set(&shared, &level, Value::from(9));
    assert_eq!(value(&shared, &var("x")), Some(json!(2)));
    assert_eq!(df.problems()["variable:x"].code, "exprError");
    set(&shared, &level, Value::from(3));
    assert_eq!(
        value(&shared, &var("x")),
        Some(json!(3)),
        "{:?}",
        df.problems()
    );
    assert!(df.problems().is_empty());

    df.remove("variable:x", None);
    assert_eq!(value(&shared, &var("x")), None);
}

// ---------------------------------------------------------------------------
// Sinks (dataflow §7)

/// Journals requests as the core does, and keeps them.
#[derive(Default)]
struct Recorder {
    journal: std::sync::OnceLock<Arc<Journal>>,
    sent: std::sync::Mutex<Vec<(Seq, String, Json)>>,
}

impl covalence_core::engine::Actions for Recorder {
    fn request_action(
        &self,
        req: covalence_core::engine::ActionRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        covalence_core::engine::ActionOutcome,
                        covalence_core::CoreError,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let journal = self.journal.get().unwrap();
            let entry = journal
                .append(
                    NewEntry::new(
                        req.origin.clone(),
                        Record::ActionRequested(covalence_core::journal::ActionRequestedRecord {
                            call: "call".into(),
                            conn: req.conn.clone(),
                            action: req.action.clone(),
                            params: req.params.clone(),
                            deadline: None,
                        }),
                    )
                    .cause(req.cause),
                )
                .unwrap();
            self.sent.lock().unwrap().push((
                entry.seq,
                req.action.clone(),
                covalence_core::expr::to_json(&req.params),
            ));
            Ok(covalence_core::engine::ActionOutcome {
                call: "call".into(),
                requested: entry.seq,
                verdict: covalence_core::gate::Verdict::Dispatch,
                result_seq: None,
                result: None,
            })
        })
    }
}

impl Recorder {
    fn start(shared: &Shared, df: &Dataflow) -> Arc<Recorder> {
        let r = Arc::new(Recorder::default());
        let _ = r.journal.set(shared.journal.clone());
        df.set_actions(r.clone());
        r
    }

    fn sent(&self) -> Vec<(Seq, String, Json)> {
        self.sent.lock().unwrap().clone()
    }

    /// Waits up to a second for `n` requests.
    fn wait_for(&self, n: usize) -> Vec<(Seq, String, Json)> {
        for _ in 0..200 {
            if self.sent.lock().unwrap().len() >= n {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.sent()
    }
}

/// A patch with a cause, as a device's answer to an action is journaled.
fn set_caused(shared: &Shared, at: &StatePath, value: Value, cause: Seq) -> Seq {
    let journal = &shared.journal;
    shared.state.mutate(|tree| {
        let seq = journal
            .append(
                NewEntry::new(
                    Origin::conn(at.conn.clone()),
                    Record::StatePatch(StatePatchRecord {
                        ops: vec![JournalOp::Set {
                            path: at.clone(),
                            value: value.clone(),
                            reducer: None,
                        }],
                        replaces: None,
                        device_ts: None,
                    }),
                )
                .cause(cause),
            )
            .unwrap()
            .seq;
        let meta = Meta {
            ts: 0,
            seq,
            quality: Quality::Good,
            source: Setter::Conn(at.conn.clone()),
        };
        (seq, tree.set(at, &value, &meta))
    })
}

fn with_sinks(def: CellDef, to: Json) -> CellDef {
    CellDef {
        to: serde_json::from_value(to).unwrap(),
        ..def
    }
}

#[test]
fn a_set_sink_writes_its_variable_in_the_same_turn() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    df.put(
        with_sinks(
            cell("variable:double", var("double"), expr("vh.level * 2")),
            json!([{ "set": { "root": "vars", "path": ["copy"] } }]),
        ),
        None,
    );
    // And a cell that reads what the sink writes.
    df.put(
        cell("variable:plus", var("plus"), expr("vars.copy + 1")),
        None,
    );
    let mut feed = shared.state.subscribe();
    set(&shared, &path("c1", &["level"]), Value::from(3));
    let dirty = feed.try_recv().unwrap();
    assert!(
        dirty.contains(&var("copy")) && dirty.contains(&var("plus")),
        "{dirty:?}"
    );
    assert_eq!(value(&shared, &var("copy")), Some(json!(6)));
    assert_eq!(value(&shared, &var("plus")), Some(json!(7)));
    let copy = last_patch(&shared, &var("copy")).unwrap();
    assert_eq!(
        copy.cause,
        Some(last_patch(&shared, &var("double")).unwrap().seq)
    );
}

#[test]
fn an_action_sink_sends_changes_rate_limited_with_the_latest_value() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let sent = Recorder::start(&shared, &df);
    let level = path("c1", &["level"]);
    set(&shared, &level, Value::from(1));
    df.put(
        with_sinks(
            cell(
                "variable:out",
                var("out"),
                expr("vh.level > 0 ? vh.level : undefined"),
            ),
            json!([{ "action": { "target": "c1", "action": "setVolume", "param": "level",
                                  "params": { "bus": 2 }, "rateLimit": "200ms" } }]),
        ),
        None,
    );
    // Loading it sends nothing.
    std::thread::sleep(Duration::from_millis(50));
    assert!(sent.sent().is_empty());

    for v in 2..6 {
        set(&shared, &level, Value::from(v));
    }
    let first = sent.wait_for(1);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].2, json!({ "bus": 2, "level": 2 }));
    // The rest wait for the rate limit, and only the latest goes.
    let all = sent.wait_for(2);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1].2["level"], json!(5));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sent.sent().len(), 2);
    // Its request is caused by the cell's patch.
    let request = shared.journal.get(all[0].0).unwrap().unwrap();
    let patch = shared.journal.get(request.cause.unwrap()).unwrap().unwrap();
    assert!(matches!(
        patch.origin,
        Origin::Core {
            core: CoreOrigin::Dataflow
        }
    ));

    // `undefined` sends nothing.
    set(&shared, &level, Value::from(0));
    assert_eq!(value(&shared, &var("out")), None);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sent.sent().len(), 2);
}

#[test]
fn a_sink_stays_quiet_for_its_own_echo_and_for_edits() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    aliases(&df);
    let sent = Recorder::start(&shared, &df);
    let volume = path("c1", &["volume"]);
    set(&shared, &volume, Value::from(1));
    let fader = || {
        with_sinks(
            cell("variable:fader", var("fader"), expr("vh.volume")),
            json!([{ "action": { "target": "c1", "action": "setVolume", "param": "level" } }]),
        )
    };
    df.put(fader(), None);
    // A change from elsewhere is sent on.
    set(&shared, &volume, Value::from(5));
    let requests = sent.wait_for(1);
    assert_eq!(requests.len(), 1);
    // The mixer's answer to that request isn't sent back.
    set_caused(&shared, &volume, Value::from(6), requests[0].0);
    assert_eq!(value(&shared, &var("fader")), Some(json!(6)));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sent.sent().len(), 1);
    // An edit that changes the value sends nothing either.
    df.put(
        with_sinks(
            cell("variable:fader", var("fader"), expr("vh.volume * 10")),
            json!([{ "action": { "target": "c1", "action": "setVolume", "param": "level" } }]),
        ),
        Some(1),
    );
    assert_eq!(value(&shared, &var("fader")), Some(json!(60)));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sent.sent().len(), 1);
}

// ---------------------------------------------------------------------------
// Helpers on controls (surfaces §4.3)

const LAYER: &str = "01JC5PAGE00000000000000001";

/// A control's input event, as a surface journals it.
fn control_event(shared: &Shared, control: &str, name: &str, payload: Option<Json>) -> Seq {
    shared
        .journal
        .append(NewEntry::new(
            Origin::Surface {
                surface: "deck".into(),
                orbital: None,
            },
            Record::Event(EventRecord {
                surface: Some("01JC5URFACE000000000000001".into()),
                component: Some(LAYER.into()),
                control: Some(control.into()),
                ..EventRecord::at(
                    path("surface", &["deck", "controls", control]),
                    name,
                    payload.as_ref().map(covalence_core::expr::json_to_value),
                )
            }),
        ))
        .unwrap()
        .seq
}

fn helper_cells(json: Json) -> Vec<CellDef> {
    let b = common::placed(&json).unwrap();
    b.helper_cells("cams")
}

#[tokio::test]
async fn an_encoder_follows_the_volume_and_sends_only_its_own_turns() {
    let shared = shared();
    let (time, clock) = Time::new();
    let df = Dataflow::with_clock(&shared, clock);
    aliases(&df);
    let sent = Recorder::start(&shared, &df);
    let volume = path("c1", &["volume"]);
    set(&shared, &volume, Value::from(50));
    let cells = helper_cells(json!({
        "id": "b1", "component": LAYER, "control": "e1",
        "helpers": { "vol": {
            "use": "position", "max": 100,
            "follow": { "ref": { "conn": "c1", "path": ["volume"] } },
            "to": { "action": { "target": "c1", "action": "setVolume", "param": "level" } }
        } }
    }));
    df.replace("helper:b1/", cells, None);
    let vol = path("component", &["cams", "controls", "e1", "vol"]);
    // It starts where the volume is, and sends nothing for that.
    assert_eq!(value(&shared, &vol), Some(json!(50)));

    // Turning sends.
    control_event(&shared, "e1", "turn", Some(json!({ "delta": 3 })));
    df.flush().await;
    assert_eq!(value(&shared, &vol), Some(json!(53)));
    let requests = sent.wait_for(1);
    assert_eq!(requests[0].2, json!({ "level": 53 }));

    // The volume moving from elsewhere during the hold-off doesn't move it...
    time.advance(100);
    set(&shared, &volume, Value::from(70));
    assert_eq!(value(&shared, &vol), Some(json!(53)));
    // ...until the hold-off is over; then it follows and sends nothing back.
    time.advance(200);
    df.tick();
    assert_eq!(value(&shared, &vol), Some(json!(70)));

    // Touched, it doesn't follow at all.
    control_event(&shared, "e1", "touch", None);
    df.flush().await;
    time.advance(1_000);
    set(&shared, &volume, Value::from(20));
    df.tick();
    assert_eq!(value(&shared, &vol), Some(json!(70)));
    control_event(&shared, "e1", "untouch", None);
    df.flush().await;
    time.advance(300);
    df.tick();
    assert_eq!(value(&shared, &vol), Some(json!(20)));

    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sent.sent().len(), 1, "{:?}", sent.sent());
}

#[tokio::test]
async fn a_select_helper_pages_through_a_list() {
    let shared = shared();
    let df = Dataflow::start(&shared);
    let cells = helper_cells(json!({
        "id": "b2", "component": LAYER, "control": "e2",
        "helpers": { "page": { "use": "select", "items": ["cams", "audio", "lights"],
                               "to": { "set": { "root": "vars", "path": ["page"] } } } }
    }));
    df.replace("helper:b2/", cells, None);
    let page = path("component", &["cams", "controls", "e2", "page"]);
    assert_eq!(value(&shared, &page), Some(json!("cams")));
    control_event(&shared, "e2", "turn", Some(json!({ "delta": 1 })));
    control_event(&shared, "e2", "turn", Some(json!({ "delta": 1 })));
    df.flush().await;
    assert_eq!(value(&shared, &page), Some(json!("lights")));
    assert_eq!(value(&shared, &var("page")), Some(json!("lights")));
    control_event(&shared, "e2", "turn", Some(json!({ "delta": -5 })));
    df.flush().await;
    assert_eq!(value(&shared, &page), Some(json!("cams")));
    // Removing the binding's helpers removes their values.
    df.replace("helper:b2/", Vec::new(), None);
    assert_eq!(value(&shared, &page), None);
}
