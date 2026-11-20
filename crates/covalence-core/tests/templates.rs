//! Flow templates and instances in the runtime (composition spec §1–§5,
//! §9): params as cells at the instance's home, `params` in expressions,
//! one flow per item of a live list, and the instance chain in the journal.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use covalence_core::Shared;
use covalence_core::compose::{Link, MAX_ITEMS};
use covalence_core::dataflow::Dataflow;
use covalence_core::engine::{ActionOutcome, ActionRequest, Actions, CoreError};
use covalence_core::expr::QuickJs;
use covalence_core::flow::runtime::{FireOutcome, Owner, Placement, RunInfo, item_id};
use covalence_core::flow::{Flow, Runtime};
use covalence_core::gate::Verdict;
use covalence_core::journal::{ActionRequestedRecord, Journal, NewEntry, Origin, Record};
use covalence_core::state::{Meta, Quality, Source, StatePath};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::json;

/// Journals each request, with its instance chain, and succeeds.
struct Accept(Arc<Journal>);

impl Actions for Accept {
    fn request_action(
        &self,
        req: ActionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CoreError>> + Send + '_>> {
        Box::pin(async move {
            let requested = self
                .0
                .append(
                    NewEntry::new(
                        req.origin,
                        Record::ActionRequested(ActionRequestedRecord {
                            call: "c".into(),
                            conn: req.conn,
                            action: req.action,
                            params: req.params,
                            deadline: None,
                        }),
                    )
                    .cause(req.cause)
                    .run(req.run)
                    .instance(req.instance),
                )?
                .seq;
            Ok(ActionOutcome {
                call: "c".into(),
                requested,
                verdict: Verdict::Dispatch,
                result_seq: None,
                result: Some(Ok(None)),
            })
        })
    }
}

struct World {
    shared: Shared,
    runtime: Runtime,
    _dataflow: Arc<Dataflow>,
}

impl World {
    fn new() -> Self {
        let shared = Shared::new(Journal::in_memory(100_000));
        let dataflow = Arc::new(Dataflow::start(&shared));
        let runtime = Runtime::start(
            shared.clone(),
            Arc::new(Accept(shared.journal.clone())),
            Arc::new(QuickJs::default()),
        );
        runtime.set_cells(dataflow.clone());
        World {
            shared,
            runtime,
            _dataflow: dataflow,
        }
    }

    fn place(&self, instance: &Flow, template: &Flow) {
        self.runtime
            .place(Placement::of(instance, Some(template)).expect("it runs"));
    }

    fn set_var(&self, name: &str, value: serde_json::Value) {
        let path = StatePath::new("vars", vec![PathSegment::Key(name.into())]);
        let value = covalence_core::expr::json_to_value(&value);
        let seq = self.shared.journal.last_seq().unwrap_or(0);
        let meta = Meta {
            ts: 0,
            seq,
            quality: Quality::Good,
            source: Source::Core,
        };
        self.shared
            .state
            .mutate(|tree| ((), tree.set(&path, &value, &meta)));
    }

    fn value(&self, path: &[&str]) -> Option<Value> {
        let (root, rest) = path.split_first().unwrap();
        let path = StatePath::new(
            *root,
            rest.iter().map(|s| PathSegment::Key((*s).into())).collect(),
        );
        self.shared.state.value(&path)
    }

    async fn eventually<T>(&self, what: &str, mut check: impl FnMut() -> Option<T>) -> T {
        for _ in 0..500 {
            if let Some(v) = check() {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn finished(&self, run: &str) -> RunInfo {
        self.eventually("the run to finish", || {
            self.runtime.run(run).filter(|i| i.finished.is_some())
        })
        .await
    }

    fn kinds(&self, kind: &str) -> Vec<Arc<covalence_core::journal::Entry>> {
        self.shared
            .journal
            .range(1, usize::MAX)
            .unwrap()
            .into_iter()
            .filter(|e| e.record.kind() == kind)
            .collect()
    }
}

fn parse(json: serde_json::Value) -> Flow {
    Flow::parse(&json).unwrap()
}

/// A camera watchdog: waits, then reports its camera through `stop`.
fn template() -> Flow {
    parse(json!({
        "id": "tpl",
        "name": "Watch camera",
        "slug": "watchCamera",
        "enabled": true,
        "params": {
            "cam": { "type": "string", "required": true },
            "label": { "type": "string", "default": "CAM" },
        },
        "triggers": [{ "kind": "api" }],
        "concurrency": "parallel",
        "outputs": { "said": { "type": "string" } },
        "body": [
            { "id": "hold", "kind": "waitUntil", "until": "vars.go", "timeout": "5s" },
            { "id": "say", "kind": "stop", "outputs": {
                "said": { "expr": "`${params.label} ${params.cam}`" },
            } },
        ],
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_instance_runs_its_template_with_its_params() {
    let w = World::new();
    let tpl = template();
    // Abstract: a required param with no default, so it doesn't run by itself.
    w.runtime.set_flow(tpl.clone());
    assert!(
        w.runtime
            .fire_api("tpl", Origin::system(), Default::default())
            .is_err()
    );

    let one = parse(json!({
        "id": "one", "name": "Cam one", "slug": "camOne", "enabled": true,
        "template": "tpl",
        "params": { "cam": { "expr": "vars.which" } },
    }));
    w.set_var("which", json!("A"));
    w.set_var("go", json!(true));
    w.place(&one, &tpl);
    // Params are cells at the instance's home, and follow state.
    w.eventually("the param", || {
        w.value(&["flow", "camOne", "params", "cam"])
    })
    .await;
    assert_eq!(
        w.value(&["flow", "camOne", "params", "label"]),
        Some(Value::from("CAM"))
    );

    let FireOutcome::Started(run) = w
        .runtime
        .fire_api("one", Origin::system(), Default::default())
        .unwrap()
        .outcome
    else {
        panic!("it starts")
    };
    let info = w.finished(&run).await;
    assert_eq!(info.outputs, Some(json!({ "said": "CAM A" })));
    let chain = vec![Link {
        template: "tpl".into(),
        instance: "one".into(),
        key: None,
    }];
    assert_eq!(info.instance.as_ref(), Some(&chain));
    assert_eq!(info.params, Some(json!({ "cam": "A", "label": "CAM" })));

    // run.started records the params; every entry of the run carries the chain.
    let started = w.kinds("run.started");
    let Record::RunStarted(r) = &started[0].record else {
        unreachable!()
    };
    assert_eq!(r.params.as_ref().unwrap()["cam"], Value::from("A"));
    for e in w.shared.journal.range(1, usize::MAX).unwrap() {
        if e.run.as_deref() == Some(run.as_str()) {
            assert_eq!(e.instance.as_ref(), Some(&chain), "{}", e.record.kind());
        }
    }

    // The param follows state: the next run sees the new camera.
    w.set_var("which", json!("B"));
    w.eventually("the param to follow", || {
        (w.value(&["flow", "camOne", "params", "cam"]) == Some(Value::from("B"))).then_some(())
    })
    .await;
    let FireOutcome::Started(run) = w
        .runtime
        .fire_api("one", Origin::system(), Default::default())
        .unwrap()
        .outcome
    else {
        panic!("it starts")
    };
    assert_eq!(
        w.finished(&run).await.outputs,
        Some(json!({ "said": "CAM B" }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn for_each_makes_a_flow_per_item_that_follows_the_list() {
    let w = World::new();
    let tpl = template();
    let cams = parse(json!({
        "id": "cams", "name": "Cameras", "slug": "cams", "enabled": true,
        "template": "tpl",
        "forEach": { "items": { "ref": { "root": "vars", "path": ["rig"] } }, "key": "item.name" },
        "params": {
            "cam": { "ref": { "scope": "item", "path": ["name"] } },
            "label": { "expr": "`#${index}`" },
        },
    }));
    w.set_var("rig", json!([{ "name": "a" }, { "name": "b" }]));
    w.place(&cams, &tpl);
    let key = |k: &str| PathSegment::Key(k.into());
    let keys = |w: &World| -> Vec<PathSegment> {
        w.runtime
            .items("cams")
            .unwrap()
            .into_iter()
            .map(|i| i.key)
            .collect()
    };
    assert_eq!(keys(&w), [key("a"), key("b")]);
    w.eventually("b's params", || {
        w.value(&["flow", "cams", "items", "b", "params", "cam"])
    })
    .await;
    assert_eq!(
        w.value(&["flow", "cams", "items", "b", "params", "label"]),
        Some(Value::from("#1"))
    );
    assert_eq!(w.kinds("instance.added").len(), 2);

    // An item's run waits; removing the item from the list cancels it.
    let b = item_id("cams", &key("b"));
    assert_eq!(b, "cams[\"b\"]");
    let FireOutcome::Started(run) = w
        .runtime
        .fire_api(&b, Origin::system(), Default::default())
        .unwrap()
        .outcome
    else {
        panic!("it starts")
    };
    w.eventually("the run to start", || {
        w.runtime.run(&run).filter(|i| i.started.is_some())
    })
    .await;
    w.set_var("rig", json!([{ "name": "a" }, { "name": "c" }]));
    let info = w.finished(&run).await;
    assert_eq!(
        info.error.as_ref().map(|e| e.code.as_str()),
        Some("removed")
    );
    w.eventually("the list to follow", || {
        (keys(&w) == [key("a"), key("c")]).then_some(())
    })
    .await;
    let removed = w.kinds("instance.removed");
    assert_eq!(removed.len(), 1);
    let Record::InstanceRemoved(r) = &removed[0].record else {
        unreachable!()
    };
    assert_eq!(r.instance[0].key, Some(key("b")));
    assert!(
        w.value(&["flow", "cams", "items", "b", "params", "cam"])
            .is_none()
    );
    assert!(
        w.runtime
            .fire_api(&b, Origin::system(), Default::default())
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_past_the_limit_reports_too_many() {
    let w = World::new();
    let tpl = template();
    let list: Vec<String> = (0..300).map(|i| format!("cam{i}")).collect();
    w.set_var("rig", json!(list));
    let cams = parse(json!({
        "id": "cams", "name": "Cameras", "slug": "cams", "enabled": true,
        "template": "tpl",
        "forEach": { "items": { "ref": { "root": "vars", "path": ["rig"] } } },
        "params": { "cam": { "ref": { "scope": "item", "path": [] } } },
    }));
    w.place(&cams, &tpl);
    assert_eq!(w.runtime.items("cams").unwrap().len(), MAX_ITEMS);
    assert_eq!(
        w.value(&["flow", "cams", "tooMany"]),
        Some(Value::from((300 - MAX_ITEMS) as u64))
    );
    w.set_var("rig", json!(["one"]));
    w.eventually("the list to shrink", || {
        (w.runtime.items("cams").unwrap().len() == 1).then_some(())
    })
    .await;
    assert!(w.value(&["flow", "cams", "tooMany"]).is_none());
}

/// A press of `control` while component `component` controls it.
fn press(component: &str, control: &str) -> covalence_core::journal::EventRecord {
    covalence_core::journal::EventRecord {
        surface: Some("deck".into()),
        component: Some(component.into()),
        control: Some(control.into()),
        ..covalence_core::journal::EventRecord::at(
            StatePath::new(
                "surface",
                vec![
                    PathSegment::Key("deck".into()),
                    PathSegment::Key("controls".into()),
                    PathSegment::Key(control.into()),
                ],
            ),
            "press",
            None,
        )
    }
}

fn owned_flow() -> Flow {
    parse(json!({
        "id": "take", "name": "Take", "slug": "take", "enabled": true,
        "owner": "L",
        "triggers": [{ "kind": "event", "from": { "controls": "k1" }, "event": "press" }],
        "concurrency": "ignore",
        "body": [
            { "id": "note", "kind": "set", "path": "vars.seen", "value": "trigger.component + '/' + trigger.control" },
            { "id": "hold", "kind": "waitUntil", "until": "vars.go", "timeout": "5s" },
        ],
    }))
}

fn owner(slug: &str) -> Owner {
    Owner {
        component: "L".into(),
        home: StatePath::new("component", vec![PathSegment::Key(slug.into())]),
        instance: None,
    }
}

/// An owned flow (composition spec §2) handles its component's controls with
/// the component implied, and keeps its status under the component's home.
#[tokio::test(flavor = "multi_thread")]
async fn an_owned_flow_runs_on_its_layers_controls() {
    let w = World::new();
    let flow = owned_flow();
    w.runtime.place(
        Placement::of(&flow, None)
            .expect("it runs")
            .owned_by(vec![owner("deck")]),
    );
    let home = ["component", "deck", "flows", "take", "running"];
    w.eventually("its status under the component", || w.value(&home))
        .await;
    assert_eq!(w.value(&["flow", "take", "running"]), None);

    // Another component's k1 isn't its owner's.
    w.shared
        .journal
        .append(NewEntry::new(
            Origin::system(),
            Record::Event(press("M", "k1")),
        ))
        .unwrap();
    w.shared
        .journal
        .append(NewEntry::new(
            Origin::system(),
            Record::Event(press("L", "k1")),
        ))
        .unwrap();
    w.eventually("the run", || {
        (w.value(&home) == Some(Value::from(1u64))).then_some(())
    })
    .await;
    assert_eq!(w.runtime.runs(Some("take")).len(), 1);
    assert_eq!(w.value(&["vars", "seen"]), Some(Value::from("L/k1")));

    // Its events are at its home.
    w.set_var("go", json!(true));
    let run = w.runtime.runs(Some("take"))[0].run.clone();
    w.finished(&run).await;
    let finished: Vec<_> = w
        .kinds("event")
        .into_iter()
        .filter_map(|e| match &e.record {
            Record::Event(ev) if ev.event == "finished" => Some(ev.path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        vec![StatePath::new(
            "component",
            ["deck", "flows", "take"]
                .iter()
                .map(|s| PathSegment::Key((*s).into()))
                .collect()
        )]
    );

    // A renamed component moves it.
    w.runtime.place(
        Placement::of(&flow, None)
            .expect("it runs")
            .owned_by(vec![owner("main")]),
    );
    w.eventually("its new home", || {
        w.value(&["component", "main", "flows", "take", "running"])
    })
    .await;
    assert_eq!(w.value(&home), None);
}

/// Placed without its owner, a `{controls}` source matches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_owned_flow_without_its_layer_takes_no_input() {
    let w = World::new();
    w.runtime.set_flow(owned_flow());
    w.shared
        .journal
        .append(NewEntry::new(
            Origin::system(),
            Record::Event(press("L", "k1")),
        ))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(w.runtime.runs(Some("take")).is_empty());
}
