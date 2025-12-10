use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Agent {
    pub id: String,
    #[serde(skip_serializing)]
    pub secret: String,
    pub config: Option<Vec<AgentConfig>>,
    pub health: Option<HealthStatus>,
    pub last_seen: DateTime<Utc>,
}

impl Agent {
    pub fn new(id: String, secret: String) -> Self {
        Self {
            id,
            secret,
            config: None,
            health: None,
            last_seen: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Active,
    Inactive,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AgentConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_caracat_batch_size")]
    pub batch_size: u64,
    #[serde(default = "default_caracat_instance_id")]
    pub instance_id: u16,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub min_ttl: Option<u8>,
    #[serde(default)]
    pub max_ttl: Option<u8>,
    #[serde(default)]
    pub integrity_check: bool,
    #[serde(default = "default_caracat_interface")]
    pub interface: String,
    #[serde(default)]
    pub src_ipv4_prefix: Option<String>,
    #[serde(default)]
    pub src_ipv6_prefix: Option<String>,
    #[serde(default = "default_caracat_packets")]
    pub packets: u64,
    #[serde(default = "default_caracat_probing_rate")]
    pub probing_rate: u64,
    #[serde(default = "default_caracat_rate_limiting_method")]
    pub rate_limiting_method: String,
}

fn default_caracat_batch_size() -> u64 {
    1000
}
fn default_caracat_instance_id() -> u16 {
    0
}
fn default_caracat_interface() -> String {
    "eth0".to_string()
}
fn default_caracat_packets() -> u64 {
    10000
}
fn default_caracat_probing_rate() -> u64 {
    1000
}
fn default_caracat_rate_limiting_method() -> String {
    "None".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthStatus {
    pub healthy: bool,
    #[serde(default = "Utc::now")]
    pub last_check: DateTime<Utc>,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct AgentStore {
    agents: Arc<RwLock<HashMap<String, Agent>>>,
}

impl AgentStore {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_agent(&self, id: String, secret: String) -> Result<(), String> {
        let now = Utc::now();
        let mut agents = self.agents.write().await;
        if let Some(existing) = agents.get(&id) {
            if existing.secret == secret {
                // Already registered with same secret, allow idempotent registration
                return Ok(());
            } else {
                // Conflict: id taken by another agent
                return Err(format!("Agent with id '{}' already exists", id));
            }
        }
        let agent = Agent {
            id: id.clone(),
            secret,
            config: None,
            health: None,
            last_seen: now,
        };
        agents.insert(id, agent);
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Option<Agent> {
        let agents = self.agents.read().await;
        agents.get(id).cloned()
    }

    pub async fn list_all(&self) -> Vec<Agent> {
        let agents = self.agents.read().await;
        agents.values().cloned().collect()
    }

    pub async fn update_last_seen(&self, id: &str) {
        let mut agents = self.agents.write().await;
        if let Some(agent) = agents.get_mut(id) {
            agent.last_seen = Utc::now();
        }
    }

    pub async fn update_config(&self, id: &str, config: Vec<AgentConfig>) {
        let mut agents = self.agents.write().await;
        if let Some(agent) = agents.get_mut(id) {
            agent.config = Some(config);
            agent.last_seen = Utc::now();
        }
    }

    pub async fn update_health(&self, id: &str, health: HealthStatus) {
        let mut agents = self.agents.write().await;
        if let Some(agent) = agents.get_mut(id) {
            agent.health = Some(health);
            agent.last_seen = Utc::now();
        }
    }

    pub async fn remove_agent(&self, id: &str) -> bool {
        let mut agents = self.agents.write().await;
        agents.remove(id).is_some()
    }

    /// Remove agents that haven't sent a health check in the specified duration
    pub async fn remove_stale_agents(&self, max_age: chrono::Duration) -> Vec<String> {
        let mut agents = self.agents.write().await;
        let now = Utc::now();
        let mut removed_ids = Vec::new();

        agents.retain(|id, agent| {
            if let Some(health) = &agent.health {
                let age = now.signed_duration_since(health.last_check);
                if age > max_age {
                    removed_ids.push(id.clone());
                    return false;
                }
            }
            true
        });

        removed_ids
    }

    /// Get all agents that have sent a health check within the specified duration
    pub async fn list_healthy_agents(&self, max_age: chrono::Duration) -> Vec<Agent> {
        let agents = self.agents.read().await;
        let now = Utc::now();

        agents
            .values()
            .filter(|agent| {
                if let Some(health) = &agent.health {
                    let age = now.signed_duration_since(health.last_check);
                    age <= max_age
                } else {
                    // Agents without health status are excluded
                    false
                }
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_store_add_get() {
        let store = AgentStore::new();
        store
            .add_agent("agent1".to_string(), "secret1".to_string())
            .await
            .unwrap();

        let agent = store.get("agent1").await.unwrap();
        assert_eq!(agent.id, "agent1");
        assert_eq!(agent.secret, "secret1");
        assert!(agent.config.is_none());
        assert!(agent.health.is_none());
    }

    #[tokio::test]
    async fn test_agent_store_update_config() {
        let store = AgentStore::new();
        store
            .add_agent("agent1".to_string(), "secret1".to_string())
            .await
            .unwrap();

        let configs = vec![AgentConfig::default()];
        store.update_config("agent1", configs.clone()).await;

        let agent = store.get("agent1").await.unwrap();
        assert_eq!(agent.config, Some(configs));
    }

    #[tokio::test]
    async fn test_agent_store_update_health() {
        let store = AgentStore::new();
        store
            .add_agent("agent1".to_string(), "secret1".to_string())
            .await
            .unwrap();

        let health = HealthStatus {
            healthy: true,
            last_check: Utc::now(),
            message: Some("OK".to_string()),
        };
        store.update_health("agent1", health.clone()).await;

        let agent = store.get("agent1").await.unwrap();
        assert_eq!(agent.health, Some(health));
    }

    #[tokio::test]
    async fn test_remove_stale_agents() {
        let store = AgentStore::new();

        // Add agent with recent health check
        store
            .add_agent("agent1".to_string(), "secret1".to_string())
            .await
            .unwrap();
        let recent_health = HealthStatus {
            healthy: true,
            last_check: Utc::now(),
            message: None,
        };
        store.update_health("agent1", recent_health).await;

        // Add agent with old health check (11 minutes ago)
        store
            .add_agent("agent2".to_string(), "secret2".to_string())
            .await
            .unwrap();
        let old_health = HealthStatus {
            healthy: true,
            last_check: Utc::now() - chrono::Duration::minutes(11),
            message: None,
        };
        store.update_health("agent2", old_health).await;

        // Remove agents older than 10 minutes
        let removed = store
            .remove_stale_agents(chrono::Duration::minutes(10))
            .await;

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], "agent2");

        // Verify agent1 still exists
        assert!(store.get("agent1").await.is_some());
        // Verify agent2 was removed
        assert!(store.get("agent2").await.is_none());
    }

    #[tokio::test]
    async fn test_list_healthy_agents() {
        let store = AgentStore::new();

        // Add agent with recent health check
        store
            .add_agent("agent1".to_string(), "secret1".to_string())
            .await
            .unwrap();
        let recent_health = HealthStatus {
            healthy: true,
            last_check: Utc::now(),
            message: None,
        };
        store.update_health("agent1", recent_health).await;

        // Add agent with old health check
        store
            .add_agent("agent2".to_string(), "secret2".to_string())
            .await
            .unwrap();
        let old_health = HealthStatus {
            healthy: true,
            last_check: Utc::now() - chrono::Duration::minutes(11),
            message: None,
        };
        store.update_health("agent2", old_health).await;

        // Add agent without health check
        store
            .add_agent("agent3".to_string(), "secret3".to_string())
            .await
            .unwrap();

        // List agents with health check in last 10 minutes
        let healthy = store
            .list_healthy_agents(chrono::Duration::minutes(10))
            .await;

        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].id, "agent1");
    }
}
