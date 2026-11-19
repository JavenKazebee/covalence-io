//! Flow templates over HTTP (composition spec §1–§5, §9; state spec §7.6),
//! against the real plugin host and simulated Videohubs. Needs `deno` on
//! the PATH.

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

mod common;
use common::{Server, encode};

const WAIT: Duration = Duration::from_secs(10);

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

async fn create(server: &Server, flow: Value) -> Value {
    let (status, body) = server.post("/flows", flow).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["flow"].clone()
}

fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}

/// Starts a flow's `api` trigger by its runtime ID and waits for the run.
async fn run(server: &Server, flow: &str) -> Value {
    let uri = format!("/flows/{}/run", encode(flow));
    let (status, body) = server.post(&uri, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let run = body["run"].as_str().unwrap().to_owned();
    server
        .eventually(WAIT, "the run to finish", || async {
            let info = server.get(&format!("/runs/{run}")).await;
            info.get("finished").is_some().then_some(info)
        })
        .await
}

/// "Route output": routes `input` to `params.output` on `params.hub`.
fn route_template() -> Value {
    json!({
        "name": "Route output",
        "enabled": true,
        "params": {
            "hub": { "kind": "target", "required": true },
            "output": { "type": "integer", "default": 1 },
        },
        "triggers": [{ "kind": "api" }, { "kind": "call" }],
        "concurrency": "parallel",
        "body": [{
            "id": "route", "kind": "action",
            "target": { "ref": { "scope": "params", "path": ["hub"] } },
            "constraint": { "plugin": "io.covalence.videohub" },
            "action": "route",
            "params": { "output": { "ref": { "scope": "params", "path": ["output"] } }, "input": 3 },
        }],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn flow_templates_over_http() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;
    let tpl = create(&server, route_template()).await;

    // An instance binds the template's params and runs its steps.
    let a = create(
        &server,
        json!({
            "name": "Route A", "enabled": true, "template": id(&tpl),
            "params": { "hub": { "conn": vh }, "output": 2 },
        }),
    )
    .await;
    assert!(
        a.get("body").is_none(),
        "an instance has no steps of its own: {a}"
    );
    let info = run(&server, id(&a)).await;
    assert_eq!(info["status"], "ok", "{info}");
    assert_eq!(server.value("vh.outputs[2].source").await, Some(json!(3)));
    assert_eq!(
        info["instance"],
        json!([{ "template": id(&tpl), "instance": id(&a) }])
    );
    assert_eq!(info["params"]["output"], 2);

    // The journal filters by instance and by template, and names each link.
    let mine = server
        .get(&format!("/journal?from=1&limit=1000&instance={}", id(&a)))
        .await;
    let mine = mine.as_array().unwrap();
    assert!(mine.iter().any(|e| e["kind"] == "action.requested"));
    assert!(mine.iter().all(|e| e["instance"][0]["instance"] == id(&a)));
    assert_eq!(mine[0]["instance"][0]["name"], "Route A");
    assert_eq!(mine[0]["instance"][0]["templateName"], "Route output");
    let by_template = server
        .get(&format!("/journal?from=1&limit=1000&template={}", id(&tpl)))
        .await;
    assert_eq!(by_template.as_array().unwrap().len(), mine.len());
    let runs = server.get(&format!("/runs?instance={}", id(&a))).await;
    assert_eq!(runs.as_array().unwrap().len(), 1);
    let none = server.get("/runs?instance=nothing").await;
    assert!(none.as_array().unwrap().is_empty());

    // A template change that breaks an instance is refused, naming it...
    let mut colour = route_template();
    colour["params"]["colour"] = json!({ "type": "string", "required": true });
    let (status, body) = server
        .call(
            Method::PUT,
            &format!("/flows/{}", id(&tpl)),
            Some(colour.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"][0]["code"], "badInstance");
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("Route A")
    );

    // ...and passes as a batch that fixes the instance too.
    let (status, body) = server
        .post(
            "/batch",
            json!({ "ops": [
                { "op": "update", "type": "flow", "id": id(&tpl), "body": colour },
                { "op": "update", "type": "flow", "id": id(&a), "body": {
                    "name": "Route A", "enabled": true, "template": id(&tpl),
                    "params": { "hub": { "conn": vh }, "output": 2, "colour": "red" },
                } },
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"].as_array().unwrap().len(), 2);
    let stored = server.get(&format!("/flows/{}", id(&a))).await;
    assert_eq!(stored["params"]["colour"], "red");

    // A batch with an error saves nothing, and says which op it was.
    let (status, body) = server
        .post(
            "/batch",
            json!({ "ops": [
                { "op": "create", "type": "flow", "body": {
                    "name": "Route B", "enabled": true, "template": id(&tpl),
                    "params": { "hub": { "conn": vh } },
                } },
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"][0]["path"][0], "ops");
    assert_eq!(body["errors"][0]["path"][1], 0);
    assert_eq!(server.get("/flows").await.as_array().unwrap().len(), 2);

    // A template with instances can't be deleted.
    let status = server.delete(&format!("/flows/{}", id(&tpl))).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Detach: a plain flow, its params its own, defaulting to the bindings.
    let (status, body) = server
        .post(&format!("/flows/{}/detach", id(&a)), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let plain = &body["flows"][0];
    assert!(plain.get("template").is_none(), "{plain}");
    assert_eq!(plain["params"]["output"]["default"], 2);
    assert_eq!(plain["body"][0]["id"], "route");
    server
        .post(
            "/actions",
            json!({ "action": "vh.route", "params": { "output": 2, "input": 1 } }),
        )
        .await;
    assert_eq!(run(&server, id(plain)).await["status"], "ok");
    assert_eq!(server.value("vh.outputs[2].source").await, Some(json!(3)));
    // Nothing is an instance of the template now, so it can go.
    assert_eq!(
        server.delete(&format!("/flows/{}", id(&tpl))).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watchdog_per_hub_follows_the_connections() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;
    let tpl = create(&server, route_template()).await;

    // One flow per Videohub, including one connected later.
    let every = create(
        &server,
        json!({
            "name": "Every hub", "enabled": true, "template": id(&tpl),
            "forEach": { "targets": { "plugin": "io.covalence.videohub" } },
            "params": { "hub": { "ref": { "scope": "item", "path": [] } }, "output": 4 },
        }),
    )
    .await;
    let hub_param = |conn: &str| format!("flow.everyHub.items[\"{conn}\"].params.hub.conn");
    let first = hub_param(&vh);
    server
        .eventually(WAIT, "vh's item", || server.value(&first))
        .await;
    let vh2 = videohub(&server, "vh2").await;
    let second = hub_param(&vh2);
    let param = server
        .eventually(WAIT, "vh2's item", || server.value(&second))
        .await;
    assert_eq!(param, json!(vh2));

    let item = format!("{}[\"{vh2}\"]", id(&every));
    let info = run(&server, &item).await;
    assert_eq!(info["status"], "ok", "{info}");
    assert_eq!(server.value("vh2.outputs[4].source").await, Some(json!(3)));
    assert_eq!(info["instance"][0]["key"], vh2.as_str());
    let journal = server.get("/journal?from=1&limit=5000").await;
    let added = journal
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "instance.added")
        .count();
    assert_eq!(added, 2);
    // Filtered to one item.
    let one = server
        .get(&format!(
            "/runs?instance={}&key={}",
            id(&every),
            encode(&vh2)
        ))
        .await;
    assert_eq!(one.as_array().unwrap().len(), 1);

    // A call through a `flow` param runs the flow it's bound to. The
    // abstract template and the flow per hub can't be called.
    let caller = |then: &str| {
        json!({
            "name": "Call hub one", "enabled": true,
            "params": { "then": { "kind": "flow", "default": then } },
            "triggers": [{ "kind": "api" }],
            "concurrency": "parallel",
            "body": [{ "id": "go", "kind": "call", "flow": { "param": "then" } }],
        })
    };
    for unusable in [id(&tpl), id(&every)] {
        let (status, body) = server.post("/flows", caller(unusable)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["errors"][0]["code"], "badInstance", "{body}");
    }
    let single = create(
        &server,
        json!({
            "name": "Hub one", "enabled": true, "template": id(&tpl),
            "params": { "hub": { "conn": vh }, "output": 5 },
        }),
    )
    .await;
    let caller = create(&server, caller(id(&single))).await;
    assert_eq!(run(&server, id(&caller)).await["status"], "ok");
    assert_eq!(server.value("vh.outputs[5].source").await, Some(json!(3)));

    // Detaching the list makes a plain flow per hub.
    let (status, body) = server
        .post(&format!("/flows/{}/detach", id(&every)), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 2);
    let conns: Vec<&Value> = flows
        .iter()
        .map(|f| &f["params"]["hub"]["default"])
        .collect();
    assert!(conns.contains(&&json!({ "conn": vh })));
    assert!(conns.contains(&&json!({ "conn": vh2 })));
    assert_eq!(
        server
            .call(Method::GET, &format!("/flows/{}", id(&every)), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
