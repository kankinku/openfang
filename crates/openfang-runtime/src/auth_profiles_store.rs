//! Persistent auth profile store (`~/.openfang/auth-profiles.json`).
//!
//! This provides OpenClaw-style multi-profile credential routing metadata:
//! - `profiles`: profile definitions (provider + env var + priority)
//! - `order`: optional explicit profile order per provider
//! - `last_good`: most recently successful profile per provider
//! - `usage_stats`: optional cooldown/error metadata

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_FILE: &str = "auth-profiles.json";
const STORE_VERSION: u32 = 1;

/// One stored profile entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredAuthProfile {
    /// Provider ID (e.g. "anthropic", "openai").
    pub provider: String,
    /// Profile kind (`api_key`, `token`, `oauth`).
    pub kind: String,
    /// Environment variable containing credential material.
    pub env_var: String,
    /// Priority (lower is preferred).
    pub priority: u32,
}

impl Default for StoredAuthProfile {
    fn default() -> Self {
        Self {
            provider: String::new(),
            kind: "api_key".to_string(),
            env_var: String::new(),
            priority: 100,
        }
    }
}

/// Optional usage/cooldown state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileUsageStats {
    /// Last successful use (epoch ms).
    pub last_used_at_ms: Option<u64>,
    /// Last failure timestamp (epoch ms).
    pub last_failure_at_ms: Option<u64>,
    /// Number of recent errors.
    pub error_count: u32,
    /// Cooldown end timestamp (epoch ms).
    pub cooldown_until_ms: Option<u64>,
}

/// On-disk auth profile store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthProfileStore {
    pub version: u32,
    pub profiles: HashMap<String, StoredAuthProfile>,
    pub order: HashMap<String, Vec<String>>,
    pub last_good: HashMap<String, String>,
    pub usage_stats: HashMap<String, ProfileUsageStats>,
}

impl Default for AuthProfileStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            profiles: HashMap::new(),
            order: HashMap::new(),
            last_good: HashMap::new(),
            usage_stats: HashMap::new(),
        }
    }
}

/// Resolved profile selection for runtime driver creation.
#[derive(Debug, Clone)]
pub struct ResolvedProfileEnv {
    pub profile_id: String,
    pub env_var: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve store path from OpenFang home dir.
pub fn store_path(home_dir: &Path) -> PathBuf {
    home_dir.join(STORE_FILE)
}

/// Load store from disk (returns default store on missing/invalid file).
pub fn load_store(home_dir: &Path) -> AuthProfileStore {
    let path = store_path(home_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return AuthProfileStore::default();
    };
    serde_json::from_str::<AuthProfileStore>(&raw).unwrap_or_default()
}

/// Save store to disk.
pub fn save_store(home_dir: &Path, store: &AuthProfileStore) -> Result<(), String> {
    let path = store_path(home_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create auth store dir: {e}"))?;
    }
    let content =
        serde_json::to_string_pretty(store).map_err(|e| format!("Serialize auth store: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Write auth store: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Upsert one env-backed profile entry.
pub fn upsert_env_profile(
    home_dir: &Path,
    provider: &str,
    profile_name: &str,
    env_var: &str,
    kind: &str,
) -> Result<String, String> {
    let provider = provider.trim().to_lowercase();
    let profile_name = profile_name.trim().to_lowercase();
    let env_var = env_var.trim().to_string();
    if provider.is_empty() || profile_name.is_empty() || env_var.is_empty() {
        return Err("provider/profile/env_var cannot be empty".to_string());
    }

    let profile_id = format!("{provider}:{profile_name}");
    let mut store = load_store(home_dir);
    let next_priority = store
        .profiles
        .iter()
        .filter(|(_, p)| p.provider == provider)
        .map(|(_, p)| p.priority)
        .min()
        .unwrap_or(100);

    store.profiles.insert(
        profile_id.clone(),
        StoredAuthProfile {
            provider: provider.clone(),
            kind: if kind.trim().is_empty() {
                "api_key".to_string()
            } else {
                kind.trim().to_lowercase()
            },
            env_var,
            priority: next_priority,
        },
    );
    store.last_good.insert(provider, profile_id.clone());
    save_store(home_dir, &store)?;
    Ok(profile_id)
}

/// Remove one profile entry.
pub fn remove_profile(home_dir: &Path, provider: &str, profile_name: &str) -> Result<bool, String> {
    let provider = provider.trim().to_lowercase();
    let profile_name = profile_name.trim().to_lowercase();
    if provider.is_empty() || profile_name.is_empty() {
        return Ok(false);
    }
    let profile_id = format!("{provider}:{profile_name}");
    let mut store = load_store(home_dir);
    let removed = store.profiles.remove(&profile_id).is_some();
    if removed {
        if store.last_good.get(&provider).map(|v| v == &profile_id).unwrap_or(false) {
            store.last_good.remove(&provider);
        }
        if let Some(order) = store.order.get_mut(&provider) {
            order.retain(|id| id != &profile_id);
            if order.is_empty() {
                store.order.remove(&provider);
            }
        }
        store.usage_stats.remove(&profile_id);
        save_store(home_dir, &store)?;
    }
    Ok(removed)
}

/// Number of profiles configured for a provider.
pub fn profile_count_for_provider(home_dir: &Path, provider: &str) -> usize {
    let provider = provider.trim().to_lowercase();
    if provider.is_empty() {
        return 0;
    }
    let store = load_store(home_dir);
    store
        .profiles
        .values()
        .filter(|p| p.provider == provider)
        .count()
}

/// Return whether provider has at least one profile entry.
pub fn provider_has_profiles(home_dir: &Path, provider: &str) -> bool {
    profile_count_for_provider(home_dir, provider) > 0
}

/// Resolve best env var for a provider, honoring last_good/order/priority/cooldown.
pub fn resolve_env_var_for_provider(home_dir: &Path, provider: &str) -> Option<ResolvedProfileEnv> {
    let provider = provider.trim().to_lowercase();
    if provider.is_empty() {
        return None;
    }

    let store = load_store(home_dir);
    let now = now_ms();

    let mut candidates: Vec<(String, StoredAuthProfile)> = store
        .profiles
        .iter()
        .filter(|(_, profile)| profile.provider == provider)
        .map(|(id, profile)| (id.clone(), profile.clone()))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    let ordered_ids = store.order.get(&provider).cloned().unwrap_or_default();
    let last_good = store.last_good.get(&provider).cloned();

    candidates.sort_by_key(|(id, profile)| {
        let last_good_rank = if last_good.as_deref() == Some(id.as_str()) {
            0usize
        } else {
            1usize
        };
        let order_rank = ordered_ids
            .iter()
            .position(|pid| pid == id)
            .unwrap_or(usize::MAX);
        (last_good_rank, order_rank, profile.priority)
    });

    for (id, profile) in candidates {
        if profile.env_var.trim().is_empty() {
            continue;
        }
        if let Some(stats) = store.usage_stats.get(&id) {
            if let Some(cooldown_until) = stats.cooldown_until_ms {
                if cooldown_until > now {
                    continue;
                }
            }
        }
        let has_secret = std::env::var(&profile.env_var)
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        if !has_secret {
            continue;
        }
        return Some(ResolvedProfileEnv {
            profile_id: id,
            env_var: profile.env_var,
        });
    }

    None
}

/// Update last_good pointer for provider.
pub fn touch_last_good(home_dir: &Path, provider: &str, profile_id: &str) -> Result<(), String> {
    let provider = provider.trim().to_lowercase();
    let profile_id = profile_id.trim().to_string();
    if provider.is_empty() || profile_id.is_empty() {
        return Ok(());
    }
    let mut store = load_store(home_dir);
    store.last_good.insert(provider, profile_id);
    save_store(home_dir, &store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_load_store() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let profile_id =
            upsert_env_profile(home, "openai", "default", "OPENAI_API_KEY", "api_key").unwrap();
        assert_eq!(profile_id, "openai:default");

        let store = load_store(home);
        assert!(store.profiles.contains_key("openai:default"));
        assert_eq!(profile_count_for_provider(home, "openai"), 1);
    }

    #[test]
    fn resolve_env_var_prefers_available_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let var = "OPENFANG_TEST_PROFILE_ENV";
        std::env::set_var(var, "test-secret");
        let _ = upsert_env_profile(home, "anthropic", "default", var, "api_key").unwrap();

        let resolved = resolve_env_var_for_provider(home, "anthropic").unwrap();
        assert_eq!(resolved.profile_id, "anthropic:default");
        assert_eq!(resolved.env_var, var);

        std::env::remove_var(var);
    }

    #[test]
    fn remove_profile_cleans_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let _ = upsert_env_profile(home, "groq", "default", "GROQ_API_KEY", "api_key").unwrap();
        let removed = remove_profile(home, "groq", "default").unwrap();
        assert!(removed);
        assert_eq!(profile_count_for_provider(home, "groq"), 0);
    }
}
