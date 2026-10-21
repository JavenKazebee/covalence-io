//! Headless Covalence IO server: the core, its plugin host, and an HTTP API.
//!
//! Milestone 1 API (JSON):
//!
//! | Method and path              | Does                                                     |
//! |------------------------------|----------------------------------------------------------|
//! | `GET /health`                | Host phase and journal position                          |
//! | `GET /plugins`               | Configured plugins and their manifests, once loaded      |
//! | `GET /connections`           | Connections                                              |
//! | `POST /connections`          | Creates a connection: `{plugin, alias, config, mode?}`   |
//! | `GET /state?path=vh.outputs` | Leaves at or below a text path (all state without one)   |
//! | `POST /actions`              | Requests an action: `{action: "vh.route", params}`       |
//! | `GET /journal?from&limit`    | Journal entries, oldest first                            |
//! | `GET /journal/{seq}/why`     | The cause chain from an entry back to its root           |
//! | `GET /ws`                    | Live state, journal and host status (see `ws`)           |
//!
//! Any other path is the built web UI, when there is one (see `ui`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use covalence_core::host::{HostConfig, HostPhase};
use covalence_core::journal::{DEFAULT_RING_CAPACITY, Journal};
use covalence_core::state::ConnId;
use covalence_core::{Core, Shared};
use covalence_protocol::connection::{ConnCreate, ConnMode};
use covalence_protocol::plugin::{Manifest, Permissions, PluginLoad};
use covalence_protocol::{Object, Value};
use covalence_store::Store;
use serde::{Deserialize, Serialize};

mod api;
mod ui;
mod ws;

pub use api::router;
pub use ui::{DEFAULT_UI_DIR, with_ui};

/// Entity type for connections in the store.
const CONNECTION: &str = "connection";

/// Roots that aliases may not take (state spec §1.1).
const RESERVED_ROOTS: &[&str] = &["var", "sys", "surface", "page", "flow"];

/// A plugin the server makes available: `io.covalence.videohub@0.1.0=plugins/videohub/mod.ts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSpec {
    pub id: String,
    pub version: String,
    /// The bundle passed to `plugin.load`. Relative paths are resolved against the root.
    pub bundle: PathBuf,
}

impl std::str::FromStr for PluginSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let usage = || format!("expected <id>@<version>=<bundle>, got {s:?}");
        let (name, bundle) = s.split_once('=').ok_or_else(usage)?;
        let (id, version) = name.split_once('@').ok_or_else(usage)?;
        if id.is_empty() || version.is_empty() || bundle.is_empty() {
            return Err(usage());
        }
        Ok(PluginSpec {
            id: id.to_owned(),
            version: version.to_owned(),
            bundle: bundle.into(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Holds `covalence.db`.
    pub data_dir: PathBuf,
    /// The repository, or later the install directory: the plugin host and
    /// first-party plugins live under it.
    pub root: PathBuf,
    pub plugins: Vec<PluginSpec>,
    /// The `deno` executable.
    pub deno: PathBuf,
}

impl Config {
    /// The first-party plugins Milestone 1 ships.
    pub fn default_plugins() -> Vec<PluginSpec> {
        vec![
            "io.covalence.videohub@0.1.0=plugins/videohub/mod.ts"
                .parse()
                .expect("valid spec"),
        ]
    }

    /// The Deno plugin host, with the coarse allowlist from
    /// `packages/plugin-host/README.md` (ADR 0015).
    fn host_config(&self) -> HostConfig {
        let root = self.root.display();
        let mut config = HostConfig::new(&self.deno);
        config.args = vec![
            "run".into(),
            "--no-prompt".into(),
            format!("--config={root}/deno.json"),
            format!("--allow-read={root}/packages,{root}/plugins"),
            "--allow-net".into(),
            "--allow-env=COVALENCE_PROTOCOL_DEBUG".into(),
            format!("{root}/packages/plugin-host/main.ts"),
        ];
        config
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("cannot create {0}: {1}")]
    DataDir(PathBuf, std::io::Error),
    #[error("cannot resolve {0}: {1}")]
    Root(PathBuf, std::io::Error),
    #[error(transparent)]
    Store(#[from] covalence_store::StoreError),
    #[error(transparent)]
    Journal(#[from] covalence_core::journal::JournalError),
    #[error("stored connection {id} is unreadable: {source}")]
    Entity {
        id: String,
        source: serde_json::Error,
    },
}

/// A connection as stored in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: ConnId,
    pub plugin: String,
    pub version: String,
    pub alias: String,
    pub config: serde_json::Map<String, serde_json::Value>,
    pub mode: ConnMode,
    /// Resolved from the manifest's templates. Not enforced yet (ADR 0015).
    pub permissions: Permissions,
}

impl Connection {
    fn create_params(&self) -> ConnCreate {
        ConnCreate {
            plugin: self.plugin.clone(),
            version: self.version.clone(),
            alias: self.alias.clone(),
            config: json_object_to_value(&self.config),
            secrets: BTreeMap::new(),
            permissions: self.permissions.clone(),
            mode: self.mode,
            isolate: false,
        }
    }
}

pub struct App {
    core: Core,
    plugins: Vec<PluginSpec>,
    /// Project entities. The journal has its own connection to the same file.
    store: Mutex<Store>,
    conns: RwLock<BTreeMap<ConnId, Connection>>,
    /// Set at shutdown, so long-lived WebSocket sessions let go of the app.
    closing: tokio::sync::watch::Sender<bool>,
}

impl App {
    /// Opens the project, starts the core and the plugin host, and registers
    /// the plugins and stored connections. Must be called inside a tokio runtime.
    pub fn start(config: Config) -> Result<Arc<App>, StartError> {
        std::fs::create_dir_all(&config.data_dir)
            .map_err(|e| StartError::DataDir(config.data_dir.clone(), e))?;
        let root = config
            .root
            .canonicalize()
            .map_err(|e| StartError::Root(config.root.clone(), e))?;
        let config = Config { root, ..config };

        let db = config.data_dir.join("covalence.db");
        let journal = Journal::with_store(Store::open(&db)?, DEFAULT_RING_CAPACITY)?;
        let store = Store::open(&db)?;
        let core = Core::start(Shared::new(journal), config.host_config());

        let plugins: Vec<PluginSpec> = config
            .plugins
            .iter()
            .map(|p| PluginSpec {
                bundle: resolve(&config.root, &p.bundle),
                ..p.clone()
            })
            .collect();

        let mut conns = BTreeMap::new();
        for row in store.entities(CONNECTION)? {
            let conn: Connection =
                serde_json::from_str(&row.body).map_err(|source| StartError::Entity {
                    id: row.id.clone(),
                    source,
                })?;
            conns.insert(conn.id.clone(), conn);
        }

        let app = Arc::new(App {
            core,
            plugins,
            store: Mutex::new(store),
            conns: RwLock::new(conns),
            closing: tokio::sync::watch::Sender::new(false),
        });
        let registering = app.clone();
        tokio::spawn(async move { registering.register().await });
        Ok(app)
    }

    /// Registers plugins and stored connections with the host supervisor,
    /// which loads and creates them whenever the host (re)starts.
    async fn register(&self) {
        let host = self.core.host();
        for plugin in &self.plugins {
            let load = PluginLoad {
                plugin: plugin.id.clone(),
                version: plugin.version.clone(),
                bundle: plugin.bundle.display().to_string(),
            };
            if let Err(e) = host.load_plugin(load).await {
                tracing::error!(plugin = %plugin.id, "cannot load plugin: {e}");
            }
        }
        let conns: Vec<Connection> = self.conns().values().cloned().collect();
        for conn in conns {
            if let Err(e) = host.create_connection(&conn.id, conn.create_params()).await {
                tracing::error!(alias = %conn.alias, "cannot create connection: {e}");
            }
        }
    }

    pub fn core(&self) -> &Core {
        &self.core
    }

    fn conns(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<ConnId, Connection>> {
        self.conns.read().unwrap_or_else(|e| e.into_inner())
    }

    fn conn_by_alias(&self, alias: &str) -> Option<Connection> {
        self.conns().values().find(|c| c.alias == alias).cloned()
    }

    /// Waits until the plugin host is up, so manifests are available.
    async fn host_ready(&self, timeout: Duration) -> bool {
        let mut status = self.core.host().status();
        tokio::time::timeout(timeout, status.wait_for(|s| s.phase == HostPhase::Ready))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    async fn manifest(&self, plugin: &str) -> Option<Manifest> {
        self.core.host().manifest(plugin).await
    }

    /// Stops the plugin host. Waits for other holders of the app (request
    /// handlers) to finish first, for up to a few seconds.
    pub async fn shutdown(self: Arc<Self>) {
        self.closing.send_replace(true);
        let mut app = self;
        for _ in 0..50 {
            match Arc::try_unwrap(app) {
                Ok(app) => return app.core.shutdown().await,
                Err(shared) => app = shared,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::warn!("app still in use at shutdown; dropping the host");
    }
}

fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

fn json_to_value(json: &serde_json::Value) -> Value {
    covalence_protocol::to_value(json).expect("JSON always encodes as MessagePack")
}

fn json_object_to_value(json: &serde_json::Map<String, serde_json::Value>) -> Object {
    json.iter()
        .map(|(k, v)| (k.clone(), json_to_value(v)))
        .collect()
}

/// Fills top-level config properties the user left out with the defaults in
/// the plugin's config schema.
fn apply_defaults(schema: &Object, config: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(Value::Map(properties)) = schema.get("properties") else {
        return;
    };
    for (name, property) in properties {
        let (Some(name), Value::Map(fields)) = (name.as_str(), property) else {
            continue;
        };
        let default = fields.iter().find(|(k, _)| k.as_str() == Some("default"));
        if let (false, Some((_, default))) = (config.contains_key(name), default)
            && let Ok(json) = serde_json::to_value(default)
        {
            config.insert(name.to_owned(), json);
        }
    }
}

/// Resolves `{config.<key>}` placeholders in the manifest's permission templates.
fn resolve_permissions(
    templates: &Permissions,
    config: &serde_json::Map<String, serde_json::Value>,
) -> Result<Permissions, String> {
    let fill = |template: &String| -> Result<String, String> {
        let mut out = String::new();
        let mut rest = template.as_str();
        while let Some(start) = rest.find("{config.") {
            out.push_str(&rest[..start]);
            let after = &rest[start + "{config.".len()..];
            let end = after
                .find('}')
                .ok_or_else(|| format!("unterminated placeholder in {template:?}"))?;
            let key = &after[..end];
            match config.get(key) {
                Some(serde_json::Value::String(s)) => out.push_str(s),
                Some(v @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_))) => {
                    out.push_str(&v.to_string())
                }
                _ => {
                    return Err(format!(
                        "permission template {template:?} needs config.{key}"
                    ));
                }
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    };
    Ok(Permissions {
        net: templates
            .net
            .as_ref()
            .map(|net| net.iter().map(fill).collect())
            .transpose()?,
        ..templates.clone()
    })
}

fn is_valid_alias(alias: &str) -> bool {
    let mut chars = alias.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !RESERVED_ROOTS.contains(&alias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn parses_plugin_specs() {
        let spec: PluginSpec = "io.x.y@1.2.3=plugins/y/mod.ts".parse().unwrap();
        assert_eq!(
            (spec.id.as_str(), spec.version.as_str()),
            ("io.x.y", "1.2.3")
        );
        assert_eq!(spec.bundle, PathBuf::from("plugins/y/mod.ts"));
        for bad in [
            "io.x.y=mod.ts",
            "io.x.y@1",
            "@1=a",
            "io.x.y@=a",
            "io.x.y@1=",
        ] {
            assert!(bad.parse::<PluginSpec>().is_err(), "{bad}");
        }
    }

    #[test]
    fn fills_defaults_and_resolves_permissions() {
        let schema: Object = json_object_to_value(&object(json!({
            "type": "object",
            "properties": { "host": { "type": "string" }, "port": { "type": "integer", "default": 9990 } }
        })));
        let mut config = object(json!({ "host": "10.0.0.12" }));
        apply_defaults(&schema, &mut config);
        assert_eq!(config["port"], json!(9990));

        let templates = Permissions {
            net: Some(vec!["{config.host}:{config.port}".into()]),
            ..Permissions::default()
        };
        let resolved = resolve_permissions(&templates, &config).unwrap();
        assert_eq!(resolved.net, Some(vec!["10.0.0.12:9990".into()]));

        config.remove("host");
        assert!(resolve_permissions(&templates, &config).is_err());
    }

    #[test]
    fn validates_aliases() {
        for ok in ["vh", "_x", "pp2"] {
            assert!(is_valid_alias(ok), "{ok}");
        }
        for bad in ["", "2vh", "v h", "v.h", "var", "sys"] {
            assert!(!is_valid_alias(bad), "{bad}");
        }
    }
}
