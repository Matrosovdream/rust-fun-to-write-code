use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub timestamp: u64,
}

pub struct MetricsCollector {
    metrics: Arc<Mutex<HashMap<String, f64>>>,
    broadcast_tx: broadcast::Sender<Metric>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            metrics: Arc::new(Mutex::new(HashMap::new())),
            broadcast_tx: tx,
        }
    }

    pub async fn record(&self, name: String, value: f64) {
        let mut metrics = self.metrics.lock().await;
        metrics.insert(name.clone(), value);

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let metric = Metric {
            name,
            value,
            timestamp,
        };

        let _ = self.broadcast_tx.send(metric);
    }

    pub async fn get(&self, name: &str) -> Option<f64> {
        self.metrics.lock().await.get(name).copied()
    }

    pub async fn get_all(&self) -> Vec<(String, f64)> {
        self.metrics
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Metric> {
        self.broadcast_tx.subscribe()
    }
}

impl Clone for MetricsCollector {
    fn clone(&self) -> Self {
        Self {
            metrics: Arc::clone(&self.metrics),
            broadcast_tx: self.broadcast_tx.clone(),
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}
