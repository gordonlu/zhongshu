use crate::agent::llm::{LlmProvider, OpenAiProvider};
use std::collections::HashMap;
use std::sync::Arc;

/// A single LLM backend configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlmProfileConfig {
    pub api_key_env: String,
    pub api_base: String,
    pub chat_model: String,
    pub reasoning_model: Option<String>,
    pub embedding_model: Option<String>,
    pub temperature: Option<f32>,
    pub max_context_tokens: Option<u32>,
}

impl Default for LlmProfileConfig {
    fn default() -> Self {
        LlmProfileConfig {
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_base: "https://api.deepseek.com".into(),
            chat_model: "deepseek-v4-flash".into(),
            reasoning_model: Some("deepseek-v4-pro".into()),
            embedding_model: None,
            temperature: None,
            max_context_tokens: None,
        }
    }
}

/// A constructed (provider + model) pair ready for use.
#[derive(Clone)]
pub struct LlmClient {
    pub provider: Arc<dyn LlmProvider>,
    pub model: String,
    pub profile_name: String,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f32>,
    pub max_context_tokens: Option<u32>,
}

/// Registry of named LLM profiles. The orb layer populates this from config.
pub struct LlmRegistry {
    profiles: HashMap<String, LlmProfileConfig>,
    role_mapping: HashMap<String, String>,
    default_profile: String,
}

impl Default for LlmRegistry {
    fn default() -> Self {
        LlmRegistry {
            profiles: HashMap::new(),
            role_mapping: HashMap::new(),
            default_profile: "default".into(),
        }
    }
}

impl LlmRegistry {
    pub fn new() -> Self {
        LlmRegistry::default()
    }

    pub fn register(&mut self, name: &str, config: LlmProfileConfig) {
        self.profiles.insert(name.to_string(), config);
    }

    pub fn set_role(&mut self, role: &str, profile: &str) {
        self.role_mapping
            .insert(role.to_string(), profile.to_string());
    }

    pub fn set_default(&mut self, profile: &str) {
        self.default_profile = profile.to_string();
    }

    /// Register a profile from its raw config.
    pub fn register_raw(
        &mut self,
        name: &str,
        api_key_env: &str,
        api_base: &str,
        chat_model: &str,
        reasoning_model: Option<String>,
        embedding_model: Option<String>,
        temperature: Option<f32>,
        max_context_tokens: Option<u32>,
    ) {
        self.profiles.insert(
            name.to_string(),
            LlmProfileConfig {
                api_key_env: api_key_env.to_string(),
                api_base: api_base.to_string(),
                chat_model: chat_model.to_string(),
                reasoning_model,
                embedding_model,
                temperature,
                max_context_tokens,
            },
        );
    }

    /// Resolve a role to a profile name, with fallback chain.
    pub fn profile_for_role(&self, role: &str) -> &str {
        self.role_mapping
            .get(role)
            .map(|s| s.as_str())
            .or_else(|| {
                // Fallback: worker.xxx → worker.default → default
                if let Some((ns, _)) = role.split_once('.') {
                    let wild = format!("{ns}.default");
                    self.role_mapping.get(&wild).map(|s| s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or(&self.default_profile)
    }

    /// Build an LlmClient for a given profile, resolving API key from env.
    pub fn build_client(&self, profile_name: &str) -> Result<LlmClient, String> {
        let config = self
            .profiles
            .get(profile_name)
            .ok_or_else(|| format!("LLM profile '{profile_name}' not found"))?;
        let api_key = std::env::var(&config.api_key_env)
            .map_err(|_| format!("env {} not set", config.api_key_env))?;
        let provider =
            OpenAiProvider::new(&api_key, &config.chat_model).with_base_url(&config.api_base);
        Ok(LlmClient {
            provider: Arc::new(provider),
            model: config.chat_model.clone(),
            profile_name: profile_name.to_string(),
            reasoning_effort: None,
            temperature: config.temperature,
            max_context_tokens: config.max_context_tokens,
        })
    }

    /// Build an LlmClient for a role, with fallback chain.
    pub fn client_for_role(&self, role: &str) -> Result<LlmClient, String> {
        let profile = self.profile_for_role(role);
        self.build_client(profile)
    }
}
