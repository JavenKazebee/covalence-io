pub mod timer;
pub mod virtual_kb;

use async_trait::async_trait;
use dashmap::DashMap;
use shared::{Message, Value};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::error;

use crate::nodes::Node;

pub type WatchRegistry = Arc<DashMap<String, watch::Sender<Value>>>;

pub struct DriverContext {
    pub name: String,
    pub cache: Arc<DashMap<String, Value>>,
    pub watch_registry: WatchRegistry,
}

impl DriverContext {
    /// Write a value to the cache. If the key has a registered watcher,
    /// the new value is also sent on its watch channel.
    pub fn update(&self, id: &str, value: Value) {
        let key = format!("{}/{}", self.name, id);
        self.cache.insert(key.clone(), value.clone());
        if let Some(tx) = self.watch_registry.get(&key) {
            let _ = tx.send(value);
        }
    }
}

#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> &str;

    // Nodes defined by the driver
    fn nodes(&self) -> Vec<Box<dyn Node>> {
        vec![]
    }

    async fn start(
        &self,
        context: DriverContext,
        rx: broadcast::Receiver<Message>,
    ) -> Result<(), String>;
}

pub struct DriverManager {
    tx: broadcast::Sender<Message>,
    cache: Arc<DashMap<String, Value>>,
    watch_registry: WatchRegistry,
    drivers: Vec<Box<dyn Driver>>,
}

impl DriverManager {
    pub fn new(tx: broadcast::Sender<Message>) -> Self {
        Self {
            tx,
            cache: Arc::new(DashMap::new()),
            watch_registry: Arc::new(DashMap::new()),
            drivers: Vec::new(),
        }
    }

    pub fn register(&mut self, driver: Box<dyn Driver>) {
        self.drivers.push(driver);
    }

    /// Collect all driver-defined nodes for registration into `NodeRegistry`.
    /// Call this before `start_all()`, which consumes `self`.
    pub fn collect_nodes(&self) -> Vec<Box<dyn Node>> {
        self.drivers.iter().flat_map(|d| d.nodes()).collect()
    }

    pub async fn start_all(self) {
        for driver in self.drivers {
            let id = driver.id().to_string();
            let cache = self.cache.clone();
            let watch_registry = self.watch_registry.clone();
            let rx = self.tx.subscribe();
            let context = DriverContext {
                name: id.clone(),
                cache,
                watch_registry,
            };
            tokio::spawn(async move {
                if let Err(e) = driver.start(context, rx).await {
                    error!(driver = id, "Failed to start driver: {}", e);
                }
            });
        }
    }

    pub fn get_cache(&self) -> Arc<DashMap<String, Value>> {
        self.cache.clone()
    }

    pub fn get_watch_registry(&self) -> WatchRegistry {
        self.watch_registry.clone()
    }
}
