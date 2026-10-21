//! Flows are stored as `flow` entities and survive a restart (M3 Phase 1).
//! Starts the real server, so it needs `deno` on the PATH like the others.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use covalence_core::flow::Flow;
use covalence_core::flow::runtime::STATE_ROOT;
use covalence_core::state::StatePath;
use covalence_protocol::state::PathSegment;
use covalence_server::{App, Config};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn start(data: &Path) -> Arc<App> {
    App::start(Config {
        data_dir: data.to_owned(),
        root: root(),
        plugins: Config::default_plugins(),
        deno: "deno".into(),
    })
    .unwrap()
}

fn fixture(name: &str) -> Flow {
    let path = root()
        .join("crates/covalence-core/fixtures/flows")
        .join(name);
    let json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    Flow::parse(&json).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn flows_survive_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let route_twice = fixture("route-twice.json");
    let mut follow = fixture("follow-vh.json");

    let app = start(data.path());
    assert!(app.flows().is_empty());
    app.put_flow(route_twice.clone()).unwrap();
    app.put_flow(follow.clone()).unwrap();
    follow.enabled = false;
    app.put_flow(follow.clone()).unwrap();
    app.shutdown().await;

    let app = start(data.path());
    let flows = app.flows();
    assert_eq!(flows.len(), 2);
    assert_eq!(flows[&route_twice.id], route_twice);
    assert_eq!(flows[&follow.id], follow, "the update is what was kept");
    // Loaded flows reach the runtime, which publishes their status.
    let running = StatePath::new(
        STATE_ROOT,
        vec![
            PathSegment::Key(route_twice.slug.clone()),
            PathSegment::Key("running".into()),
        ],
    );
    assert_eq!(app.core().shared().state.value(&running), Some(0u64.into()));

    assert_eq!(app.delete_flow(&follow.id).unwrap(), Some(follow.clone()));
    assert_eq!(app.delete_flow(&follow.id).unwrap(), None);
    assert!(
        app.runtime()
            .fire_api(&follow.id, covalence_core::journal::Origin::system())
            .is_err()
    );
    app.shutdown().await;

    let app = start(data.path());
    assert_eq!(app.flows().keys().collect::<Vec<_>>(), [&route_twice.id]);
    app.shutdown().await;
}
