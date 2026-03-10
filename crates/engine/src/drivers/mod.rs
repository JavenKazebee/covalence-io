pub mod virtual_kb;

use async_trait::async_trait;
use dashmap::DashMap;
use shared::{Message, Value};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::error;

pub struct DriverContext {
    pub name: String,
    pub cache: Arc<DashMap<String, Value>>,
}

impl DriverContext {
    pub fn update(&self, id: &str, value: Value) {
        let key = format!("{}/{}", self.name, id);
        self.cache.insert(key, value);
    }
}

#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> &str;
    async fn start(
        &self,
        context: DriverContext,
        mut rx: broadcast::Receiver<Message>,
    ) -> Result<(), String>;
}

pub struct DriverManager {
    tx: broadcast::Sender<Message>,
    cache: Arc<DashMap<String, Value>>,
    drivers: Vec<Box<dyn Driver>>,
}

impl DriverManager {
    pub fn new(tx: broadcast::Sender<Message>) -> Self {
        Self {
            tx,
            cache: Arc::new(DashMap::new()),
            drivers: Vec::new(),
        }
    }

    pub fn register(&mut self, driver: Box<dyn Driver>) {
        self.drivers.push(driver);
    }

    pub async fn start_all(self) {
        for driver in self.drivers {
            let id = driver.id().to_string();
            let cache = self.cache.clone();
            let rx = self.tx.subscribe();
            let context = DriverContext {
                name: id.clone(),
                cache,
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
}
