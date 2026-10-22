//! Milestone 3 acceptance over HTTP: the real server, the real Deno plugin
//! host and two Videohubs in simulate mode. Needs `deno` on the PATH.
//!
//! Covers the "Done when" items in docs/milestones/m3-first-flows.md that the
//! API can reach. Concurrency modes are covered in detail by
//! crates/covalence-core/tests/flow_runtime.rs.

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

mod common;
use common::Server;

const WAIT: Duration = Duration::from_secs(10);

/// Creates a Videohub connection and waits for its simulator's state.
async fn videohub(server: &Server, alias: &str) -> String {
    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": "io.covalence.videohub", "alias": alias, "config": { "host": "h" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    let path = format!("{alias}.outputs[1].source");
    server.eventually(WAIT, &path, || server.value(&path)).await;
    conn["id"].as_str().unwrap().to_owned()
}

fn route(id: &str, conn: &str, output: u32, input: u32) -> Value {
    json!({
        "id": id, "kind": "action", "target": { "conn": conn },
        "action": "route", "params": { "output": output, "input": input },
    })
}

/// Saves a flow and returns it as stored.
async fn create(server: &Server, flow: Value) -> Value {
    let (status, body) = server.post("/flows", flow).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["flow"].clone()
}

/// Starts a flow's `api` trigger and returns the run ID.
async fn start(server: &Server, flow: &Value) -> String {
    let (status, body) = server
        .post(
            &format!("/flows/{}/run", flow["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "started", "{body}");
    assert!(body["matched"].is_u64());
    body["run"].as_str().unwrap().to_owned()
}

async fn finished(server: &Server, run: &str) -> Value {
    server
        .eventually(WAIT, &format!("run {run} to finish"), || async {
            let info = server.get(&format!("/runs/{run}")).await;
            info.get("finished").is_some().then_some(info)
        })
        .await
}

/// `kind` of each journal entry in a run, with the step and status of `run.step`.
async fn run_entries(server: &Server, run: &str) -> Vec<String> {
    let journal = server.get("/journal?from=1&limit=1000").await;
    journal
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["run"] == run)
        .map(|e| match e["kind"].as_str().unwrap() {
            "run.step" => format!(
                "run.step {} {}",
                e["data"]["step"].as_str().unwrap(),
                e["data"]["status"].as_str().unwrap()
            ),
            "run.finished" => format!("run.finished {}", e["data"]["status"].as_str().unwrap()),
            kind => kind.to_owned(),
        })
        .collect()
}

async fn why_kinds(server: &Server, seq: u64) -> Vec<String> {
    let chain = server.get(&format!("/journal/{seq}/why")).await;
    chain
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn flows_over_http() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;
    let vh2 = videohub(&server, "vh2").await;

    // Done when 1: create, list, read. The slug comes from the name.
    let route_twice = create(
        &server,
        json!({
            "name": "Route twice",
            "enabled": true,
            "triggers": [{ "kind": "api" }],
            "concurrency": "parallel",
            "body": [route("route1", &vh, 1, 3), { "id": "wait1", "kind": "wait", "for": "1s" }, route("route2", &vh, 1, 4)],
        }),
    )
    .await;
    assert_eq!(route_twice["slug"], "routeTwice");
    let id = route_twice["id"].as_str().unwrap();
    assert_eq!(server.get(&format!("/flows/{id}")).await, route_twice);
    assert_eq!(server.get("/flows").await, json!([route_twice]));
    assert_eq!(
        server.value("flow.routeTwice.running").await,
        Some(json!(0))
    );

    // Done when 2: a run journals every step, and the route reads 3, then 4.
    let run = start(&server, &route_twice).await;
    server
        .eventually(WAIT, "vh.outputs[1].source = 3", || async {
            (server.value("vh.outputs[1].source").await == Some(json!(3))).then_some(())
        })
        .await;
    let info = finished(&server, &run).await;
    assert_eq!(info["status"], "ok", "{info}");
    assert_eq!(server.value("vh.outputs[1].source").await, Some(json!(4)));
    assert_eq!(
        run_entries(&server, &run).await,
        [
            "run.started",
            "run.step route1 started",
            "action.requested",
            "action.dispatched",
            "state.patch",
            "event",
            "action.result",
            "run.step route1 done",
            "run.step wait1 started",
            "run.step wait1 done",
            "run.step route2 started",
            "action.requested",
            "action.dispatched",
            "state.patch",
            "event",
            "action.result",
            "run.step route2 done",
            "run.finished ok",
        ]
    );

    // Done when 6: flow status in state.
    assert_eq!(
        server.value("flow.routeTwice.running").await,
        Some(json!(0))
    );
    assert_eq!(
        server.value("flow.routeTwice.lastRun.status").await,
        Some(json!("ok"))
    );
    let all = server.get("/state").await;
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"] == "flow.routeTwice.running"),
        "GET /state includes the flow root"
    );

    // Done when 3: an event trigger on vh routes vh2, and why walks back to
    // the API call that moved vh.
    let follow = create(
        &server,
        json!({
            "name": "Follow vh",
            "enabled": true,
            "triggers": [{ "kind": "event", "source": { "conn": vh }, "event": "routeChanged" }],
            "concurrency": "queue",
            "body": [route("follow1", &vh2, 1, 2)],
        }),
    )
    .await;
    let follow_id = follow["id"].as_str().unwrap();
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": "vh.route", "params": { "output": 2, "input": 5 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    let follower = server
        .eventually(WAIT, "the follow flow to run", || async {
            let runs = server.get(&format!("/runs?flow={follow_id}")).await;
            runs[0]["run"].as_str().map(str::to_owned)
        })
        .await;
    assert_eq!(finished(&server, &follower).await["status"], "ok");
    assert_eq!(server.value("vh2.outputs[1].source").await, Some(json!(2)));
    let journal = server.get("/journal?from=1&limit=1000").await;
    let result = journal
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["run"] == follower.as_str() && e["kind"] == "action.result")
        .unwrap()["seq"]
        .as_u64()
        .unwrap();
    assert_eq!(
        why_kinds(&server, result).await,
        [
            "action.result",
            "action.requested",
            "run.step",
            "run.started",
            "trigger.matched",
            "event",
            "action.requested",
        ]
    );
    let chain = server.get(&format!("/journal/{result}/why")).await;
    assert_eq!(
        chain.as_array().unwrap().last().unwrap()["origin"],
        json!({ "user": "api", "client": "http" })
    );
    assert_eq!(
        server.delete(&format!("/flows/{follow_id}")).await,
        StatusCode::NO_CONTENT
    );

    // Done when 5: cancel during a wait.
    let slow = create(
        &server,
        json!({
            "name": "Slow", "enabled": true, "triggers": [{ "kind": "api" }],
            "concurrency": "parallel",
            "body": [{ "id": "wait1", "kind": "wait", "for": "1m" }, route("route1", &vh, 1, 1)],
        }),
    )
    .await;
    let run = start(&server, &slow).await;
    server
        .eventually(WAIT, "the wait to start", || async {
            let info = server.get(&format!("/runs/{run}")).await;
            (info["position"] == json!(["wait1"])).then_some(())
        })
        .await;
    let (status, body) = server.post(&format!("/runs/{run}/cancel"), json!({})).await;
    assert_eq!(
        (status, &body["status"]),
        (StatusCode::ACCEPTED, &json!("running")),
        "{body}"
    );
    assert_eq!(finished(&server, &run).await["status"], "cancelled");
    assert_eq!(
        server
            .post(&format!("/runs/{run}/cancel"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        server.post("/runs/nope/cancel", json!({})).await.0,
        StatusCode::NOT_FOUND
    );

    // Done when 5: a failing action fails the run, unless onError continues.
    let mut bad = route("bad", &vh, 99, 1);
    let failing = create(
        &server,
        json!({
            "name": "Failing", "enabled": true, "triggers": [{ "kind": "api" }],
            "concurrency": "parallel", "body": [bad.clone()],
        }),
    )
    .await;
    let info = finished(&server, &start(&server, &failing).await).await;
    assert_eq!(info["status"], "failed");
    assert_eq!(info["error"]["code"], "invalid_params", "{info}");
    assert_eq!(
        server.value("flow.failing.lastRun.error.code").await,
        Some(json!("invalid_params"))
    );

    bad["onError"] = json!("continue");
    let mut tolerant = failing.clone();
    tolerant["body"] = json!([bad, route("good", &vh, 1, 2)]);
    let (status, body) = server
        .put(
            &format!("/flows/{}", failing["id"].as_str().unwrap()),
            tolerant,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let info = finished(&server, &start(&server, &body["flow"]).await).await;
    assert_eq!(info["status"], "ok");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn flow_requests_are_checked() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;

    // Problems come back per field, errors and warnings apart.
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Bad", "enabled": true,
                "triggers": [{ "kind": "event", "source": { "conn": vh }, "event": "routeChanged" }],
                "concurrency": "parallel",
                "conditions": ["vars.armed &&"],
                "inputs": {},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["errors"][0]["path"], json!(["inputs"]));
    assert_eq!(body["errors"][0]["code"], "unsupported");

    // An expression that doesn't compile points into its source.
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Bad", "enabled": true, "triggers": [{ "kind": "api" }],
                "concurrency": "parallel", "conditions": ["vars.armed &&"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"][0]["path"], json!(["conditions", 0]));
    assert_eq!(body["errors"][0]["code"], "exprSyntax");
    assert_eq!(body["errors"][0]["at"]["line"], 1, "{body}");

    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Bad params", "enabled": true, "triggers": [{ "kind": "api" }],
                "concurrency": "parallel", "body": [route("r", &vh, 1, 1), { "id": "r", "kind": "wait", "for": "1s" }],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"][0]["code"], "duplicateId", "{body}");

    // Warnings don't block a save.
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Echo", "enabled": true,
                "triggers": [{ "kind": "event", "source": { "conn": vh }, "event": "routeChanged" }],
                "concurrency": "ignore", "body": [route("r", &vh, 1, 1)],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["warnings"][0]["code"], "selfTrigger");
    let echo = body["flow"].clone();
    let echo_id = echo["id"].as_str().unwrap();

    // A flow without an api trigger, or a disabled one, can't be run.
    assert_eq!(
        server
            .post(&format!("/flows/{echo_id}/run"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let mut off = echo.clone();
    off["enabled"] = json!(false);
    off["triggers"] = json!([{ "kind": "api" }]);
    assert_eq!(
        server.put(&format!("/flows/{echo_id}"), off).await.0,
        StatusCode::OK
    );
    assert_eq!(
        server
            .post(&format!("/flows/{echo_id}/run"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );

    // A second flow can't take the slug; the URL decides the ID.
    let mut copy = echo.clone();
    copy["id"] = json!("");
    let (status, body) = server.post("/flows", copy.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["errors"][0]["code"], "duplicateSlug");
    copy["id"] = json!("01JC0000000000000000000099");
    assert_eq!(
        server
            .put(&format!("/flows/{echo_id}"), copy.clone())
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        server
            .put("/flows/01JC0000000000000000000099", copy)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // Deleting removes the flow and its state.
    assert_eq!(server.value("flow.echo.running").await, Some(json!(0)));
    assert_eq!(
        server.delete(&format!("/flows/{echo_id}")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        server.delete(&format!("/flows/{echo_id}")).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .call(axum::http::Method::GET, &format!("/flows/{echo_id}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(server.value("flow.echo.running").await, None);
    assert_eq!(
        server.post("/flows/nope/run", json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .call(axum::http::Method::GET, "/runs/nope", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Milestone 4: expressions (docs/milestones/m4-expressions.md)

async fn evaluate(server: &Server, body: Value) -> Value {
    let (status, out) = server.post("/expressions/evaluate", body).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out
}

/// `trigger.matched` entries for one flow.
async fn matched_for(server: &Server, flow: &Value) -> Vec<Value> {
    let journal = server.get("/journal?from=1&limit=1000").await;
    journal
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "trigger.matched" && e["data"]["flow"] == flow["id"])
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn expressions_preview_over_http() {
    let server = Server::start();
    videohub(&server, "vh").await;
    let source: Value = server.value("vh.outputs[1].source").await.unwrap();

    let out = evaluate(&server, json!({ "source": "vh.outputs[1].source + 1" })).await;
    assert_eq!(out["value"], json!(source.as_i64().unwrap() + 1), "{out}");
    assert_eq!(out["reads"], json!(["vh.outputs[1].source"]));

    // A sample trigger, for previewing filters; `undefined` has no value.
    let out = evaluate(
        &server,
        json!({ "source": "trigger.payload.output == 1", "trigger": { "payload": { "output": 1 } } }),
    )
    .await;
    assert_eq!(out["value"], true);
    let out = evaluate(&server, json!({ "source": "vars.nothing" })).await;
    assert!(
        out.get("value").is_none() && out.get("error").is_none(),
        "{out}"
    );

    // Errors carry their code and position; nothing is written.
    let out = evaluate(&server, json!({ "source": "1 +" })).await;
    assert_eq!(out["error"]["code"], "exprSyntax", "{out}");
    assert_eq!(out["error"]["at"]["line"], 1);
    let out = evaluate(&server, json!({ "source": "vars.x = 1" })).await;
    assert_eq!(out["error"]["code"], "exprError", "{out}");
    assert!(server.value("vars.x").await.is_none());
    let out = evaluate(&server, json!({ "source": "while (true) {}" })).await;
    assert_eq!(out["error"]["code"], "exprTimeout", "{out}");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expression_params_route_to_a_variable() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;

    // `set` writes the variable; a second step routes to it.
    let flow = create(
        &server,
        json!({
            "name": "Route to next", "enabled": true, "triggers": [{ "kind": "api" }],
            "concurrency": "queue",
            "body": [
                { "id": "pick", "kind": "set", "path": "vars.nextSource", "value": "(vars.nextSource ?? 3) + 1" },
                {
                    "id": "route1", "kind": "action", "target": { "conn": vh }, "action": "route",
                    "params": { "output": 1, "input": { "expr": "vars.nextSource" } },
                },
            ],
        }),
    )
    .await;
    let info = finished(&server, &start(&server, &flow).await).await;
    assert_eq!(info["status"], "ok", "{info}");
    assert_eq!(server.value("vars.nextSource").await, Some(json!(4)));
    server
        .eventually(WAIT, "vh output 1 on 4", || async {
            (server.value("vh.outputs[1].source").await == Some(json!(4))).then_some(())
        })
        .await;

    // A value that fails the action's schema fails the step before the request.
    let bad = create(
        &server,
        json!({
            "name": "Bad input", "enabled": true, "triggers": [{ "kind": "api" }],
            "concurrency": "parallel",
            "body": [{
                "id": "route1", "kind": "action", "target": { "conn": vh }, "action": "route",
                "params": { "output": 1, "input": { "expr": "'four'" } },
            }],
        }),
    )
    .await;
    let run = start(&server, &bad).await;
    let info = finished(&server, &run).await;
    assert_eq!(info["error"]["code"], "invalidParams", "{info}");
    assert!(
        info["error"]["message"].as_str().unwrap().contains("four"),
        "{info}"
    );
    assert!(
        !run_entries(&server, &run)
            .await
            .contains(&"action.requested".to_owned())
    );

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn state_triggers_and_filters_over_http() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;
    let vh2 = videohub(&server, "vh2").await;
    let route_vh = |output: u32, input: u32| json!({ "action": "vh.route", "params": { "output": output, "input": input } });
    let (status, _) = server.post("/actions", route_vh(1, 0)).await;
    assert_eq!(status, StatusCode::OK);

    // Runs once each time vh output 1 becomes 3, and copies it to vh2.
    let follow = create(
        &server,
        json!({
            "name": "On three", "enabled": true,
            "triggers": [{ "kind": "state", "when": "vh.outputs[1].source == 3", "edge": "rising" }],
            "concurrency": "queue",
            "body": [{
                "id": "copy", "kind": "action", "target": { "conn": vh2 }, "action": "route",
                "params": { "output": 1, "input": { "expr": "vh.outputs[1].source" } },
            }],
        }),
    )
    .await;
    server.post("/actions", route_vh(1, 3)).await;
    server
        .eventually(WAIT, "vh2 output 1 on 3", || async {
            (server.value("vh2.outputs[1].source").await == Some(json!(3))).then_some(())
        })
        .await;
    server.post("/actions", route_vh(1, 4)).await;
    server.post("/actions", route_vh(1, 4)).await;
    let matched = matched_for(&server, &follow).await;
    assert_eq!(matched.len(), 1, "{matched:?}");
    let run = server
        .get(&format!("/runs?flow={}", follow["id"].as_str().unwrap()))
        .await;
    let run = finished(&server, run[0]["run"].as_str().unwrap()).await;
    let chain = why_kinds(&server, run["finished"].as_u64().unwrap()).await;
    assert_eq!(
        chain[..3],
        ["run.finished", "run.started", "trigger.matched"]
    );
    assert_eq!(chain[3], "state.patch", "{chain:?}");

    // A filter that leaves out the flow's own change keeps it from looping,
    // live echo or not: one match for one outside route.
    let mirror = create(
        &server,
        json!({
            "name": "Mirror 1 to 2", "enabled": true,
            "triggers": [{
                "kind": "event", "source": { "conn": vh }, "event": "routeChanged",
                "filter": "trigger.payload.output == 1",
            }],
            "concurrency": "queue",
            "body": [{
                "id": "mirror", "kind": "action", "target": { "conn": vh }, "action": "route",
                "params": { "output": 2, "input": { "expr": "trigger.payload.input" } },
            }],
        }),
    )
    .await;
    server.post("/actions", route_vh(1, 5)).await;
    server
        .eventually(WAIT, "vh output 2 on 5", || async {
            (server.value("vh.outputs[2].source").await == Some(json!(5))).then_some(())
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(matched_for(&server, &mirror).await.len(), 1);

    server.stop().await;
}
