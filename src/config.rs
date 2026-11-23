use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};
use url::Url;

use crate::effort::{self, ReasoningEffort};
use crate::session::{atomic_write, private_dir};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
}

fn app_directory(base: &Path) -> PathBuf {
    base.join("ruvx")
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?;
        fn xdg(name: &str, fallback: PathBuf) -> Result<PathBuf> {
            let path = env::var_os(name).map(PathBuf::from).unwrap_or(fallback);
            ensure!(path.is_absolute(), "{name} must be an absolute path");
            Ok(app_directory(&path))
        }
        Ok(Self {
            config: xdg("XDG_CONFIG_HOME", home.join(".config"))?,
            data: xdg("XDG_DATA_HOME", home.join(".local/share"))?,
            cache: xdg("XDG_CACHE_HOME", home.join(".cache"))?,
        })
    }
    pub fn at(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
        }
    }
    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }
    pub fn load(&self) -> Result<Config> {
        let path = self.config_file();
        let mut config: Config = match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                // TOML errors may contain source excerpts: never include the original text.
                anyhow::anyhow!(
                    "Invalid config {}: {}",
                    path.display(),
                    e.message().lines().next().unwrap_or("parse error")
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(e).with_context(|| format!("Read {}", path.display())),
        };
        config.tools.protected_paths =
            vec![self.config.clone(), self.data.clone(), self.cache.clone()];
        for path in [&self.config, &self.data, &self.cache] {
            let alias = match path.file_name().and_then(|name| name.to_str()) {
                Some("ruvx") => Some("kovr"),
                Some("kovr") => Some("ruvx"),
                _ => None,
            };
            if let Some(alias) = alias {
                config
                    .tools
                    .protected_paths
                    .push(path.with_file_name(alias));
            }
        }
        config.validate()?;
        Ok(config)
    }
    pub fn save(&self, config: &Config) -> Result<()> {
        config.validate()?;
        private_dir(&self.config)?;
        atomic_write(
            &self.config_file(),
            toml::to_string_pretty(config)?.as_bytes(),
            None,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub agent: AgentConfig,
    pub ui: UiConfig,
    pub tools: ToolConfig,
    pub shell: ShellConfig,
    pub network: NetworkConfig,
    pub providers: BTreeMap<String, Profile>,
    pub skills: SkillsConfig,
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// Explicit process-local approval bypass; never loaded from or saved to TOML.
    #[serde(skip)]
    pub bypass: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            agent: AgentConfig::default(),
            ui: UiConfig::default(),
            tools: ToolConfig::default(),
            shell: ShellConfig::default(),
            network: NetworkConfig::default(),
            providers: BTreeMap::new(),
            skills: SkillsConfig::default(),
            mcp_servers: BTreeMap::new(),
            bypass: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
    pub enabled: bool,
    /// Additional local skills or one-level collections, in increasing priority.
    pub paths: Vec<PathBuf>,
}
impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            paths: Vec::new(),
        }
    }
}

/// Only global user configuration may define executable MCP servers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerConfig {
    pub enabled: bool,
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Names of environment variables to inherit, never literal secret values.
    pub inherit_env: Vec<String>,
    pub timeout_seconds: u64,
    pub max_message_bytes: usize,
}
impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: PathBuf::new(),
            args: Vec::new(),
            inherit_env: vec!["PATH".into(), "HOME".into()],
            timeout_seconds: 60,
            max_message_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub max_turns: u32,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self { max_turns: 30 }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Deprecated inline setting, accepted for compatibility and ignored.
    pub viewport_rows: u16,
    pub max_fps: u16,
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            viewport_rows: 6,
            max_fps: 30,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolConfig {
    pub max_file_bytes: usize,
    pub read_max_lines: usize,
    pub result_max_bytes: usize,
    pub list_page_size: usize,
    #[serde(skip)]
    pub protected_paths: Vec<PathBuf>,
}
impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            read_max_lines: 200,
            result_max_bytes: 64 * 1024,
            list_page_size: 100,
            protected_paths: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellConfig {
    pub executable: PathBuf,
    pub timeout_seconds: u64,
    pub max_timeout_seconds: u64,
}
impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            executable: "/bin/sh".into(),
            timeout_seconds: 120,
            max_timeout_seconds: 600,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub header_timeout_seconds: u64,
    pub chunk_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
}
impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            header_timeout_seconds: 60,
            chunk_timeout_seconds: 120,
            request_timeout_seconds: 600,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Openai,
    Chatgpt,
    Compatible,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "lowercase", deny_unknown_fields)]
pub enum AuthConfig {
    Saved { credential: String },
    Environment { variable: String },
    Chatgpt { credential: String },
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub kind: ProviderKind,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth: AuthConfig,
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
}
impl Profile {
    pub fn base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1")
    }
    pub fn responses(&self) -> bool {
        self.kind != ProviderKind::Compatible
    }
    pub fn route(&self) -> String {
        format!("{:?}:{}", self.kind, self.base_url())
    }
    pub fn model(&self, id: &str) -> Result<&ModelConfig> {
        self.models
            .get(id)
            .context("Model is not configured; use /model")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub model_id: Option<String>,
    pub name: String,
    pub context_limit: Option<usize>,
    pub output_reserve_tokens: usize,
    pub supports_tools: Option<bool>,
    pub supports_streaming: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<ReasoningEffort>>,
}
impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_id: None,
            name: String::new(),
            context_limit: None,
            output_reserve_tokens: 4096,
            supports_tools: None,
            supports_streaming: true,
            reasoning_effort: None,
            reasoning_efforts: None,
        }
    }
}
impl ModelConfig {
    pub fn supported_efforts(&self, kind: ProviderKind, id: &str) -> Option<&[ReasoningEffort]> {
        self.reasoning_efforts.as_deref().or_else(|| {
            if kind == ProviderKind::Compatible {
                None
            } else {
                effort::openai_levels(self.model_id.as_deref().unwrap_or(id))
            }
        })
    }
    pub fn validate_effort(&self, kind: ProviderKind, id: &str) -> Result<()> {
        if let Some(levels) = &self.reasoning_efforts {
            for (index, level) in levels.iter().enumerate() {
                ensure!(
                    !levels[..index].contains(level),
                    "Duplicate reasoning effort for model {id}"
                );
            }
        }
        if let Some(level) = self.reasoning_effort {
            let levels = self.supported_efforts(kind,id).with_context(||format!("Effort support for model {id} is unknown; declare reasoning_efforts or use /effort levels"))?;
            ensure!(
                levels.contains(&level),
                "Effort {level} is not supported by model {id}; use /effort default or choose a supported level"
            );
        }
        Ok(())
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
}
pub fn validate_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("Invalid provider URL")?;
    let local = url.host_str().is_some_and(|h| {
        h == "localhost"
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && local),
        "Use HTTPS for remote providers; HTTP is only allowed on loopback"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Provider URL must not contain credentials, query, or fragment"
    );
    Ok(url)
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "Unsupported config schema_version"
        );
        ensure!(
            (1..=60).contains(&self.ui.max_fps),
            "ui.max_fps must be 1..60"
        );
        ensure!(
            self.tools.max_file_bytes > 0
                && self.tools.read_max_lines > 0
                && self.tools.result_max_bytes >= 256
                && self.tools.list_page_size > 0,
            "Tool limits must be positive; result_max_bytes must be at least 256"
        );
        ensure!(
            self.shell.executable.is_absolute(),
            "shell.executable must be absolute"
        );
        ensure!(
            self.shell.timeout_seconds > 0
                && self.shell.max_timeout_seconds >= self.shell.timeout_seconds,
            "Invalid shell timeout limits"
        );
        ensure!(
            self.network.header_timeout_seconds > 0
                && self.network.chunk_timeout_seconds > 0
                && self.network.request_timeout_seconds > 0,
            "Network timeouts must be positive"
        );
        ensure!(self.skills.paths.iter().all(|path| path.is_absolute()
            && !path.as_os_str().as_encoded_bytes().contains(&0)),
            "Skill paths must be absolute and must not contain NUL");
        for (id, server) in &self.mcp_servers {
            ensure!(valid_id(id), "Invalid MCP server ID");
            ensure!(
                server.command.is_absolute()
                    && !server.command.as_os_str().as_encoded_bytes().contains(&0)
                    && server.args.iter().all(|arg| !arg.contains('\0')),
                "MCP command must be absolute and command/args must not contain NUL"
            );
            ensure!(
                (1..=600).contains(&server.timeout_seconds)
                    && (256..=16 * 1024 * 1024).contains(&server.max_message_bytes),
                "MCP timeout must be 1..600 seconds and message limit 256..16777216 bytes"
            );
            for name in &server.inherit_env {
                ensure!(
                    !name.is_empty()
                        && name.chars().enumerate().all(|(i, c)| {
                            c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                        }),
                    "Invalid MCP environment variable name"
                );
            }
        }
        for (id, profile) in &self.providers {
            ensure!(valid_id(id), "Invalid provider profile ID");
            let url = validate_url(profile.base_url())?;
            match (&profile.kind, &profile.auth) {
                (ProviderKind::Chatgpt, AuthConfig::Chatgpt { .. }) => ensure!(
                    profile.base_url() == "https://api.openai.com/v1",
                    "ChatGPT tokens may only be sent to the official API"
                ),
                (ProviderKind::Chatgpt, _) | (_, AuthConfig::Chatgpt { .. }) => {
                    bail!("ChatGPT auth requires a ChatGPT profile")
                }
                (_, AuthConfig::None) => ensure!(
                    profile.kind == ProviderKind::Compatible
                        && url.host_str().is_some_and(|h| h == "localhost"
                            || h.parse::<std::net::IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())),
                    "No-auth is only supported on local compatible endpoints"
                ),
                _ => {}
            }
            match &profile.auth {
                AuthConfig::Saved { credential } | AuthConfig::Chatgpt { credential } => {
                    ensure!(valid_id(credential), "Invalid credential reference")
                }
                AuthConfig::Environment { variable } => ensure!(
                    !variable.is_empty()
                        && variable
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "Invalid environment variable name"
                ),
                _ => {}
            }
            for (model_id, model) in &profile.models {
                model.validate_effort(profile.kind, model_id)?;
                ensure!(!model_id.trim().is_empty(), "Model ID must not be empty");
                ensure!(
                    model
                        .context_limit
                        .is_none_or(|limit| limit > model.output_reserve_tokens),
                    "Model context_limit must exceed output_reserve_tokens"
                );
                ensure!(
                    profile.kind != ProviderKind::Compatible || model.supports_tools.is_some(),
                    "Custom model must explicitly declare supports_tools"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bypass_is_explicit_runtime_state_and_never_persisted_in_config() {
        let mut config: Config = toml::from_str("schema_version = 1\n").unwrap();
        assert!(!config.bypass);
        config.bypass = true;
        let text = toml::to_string(&config).unwrap();
        assert!(!text.contains("bypass"));
        assert!(!toml::from_str::<Config>(&text).unwrap().bypass);
        assert!(toml::from_str::<Config>("bypass = true\n").is_err());
    }

    #[test]
    fn extensions_are_backward_compatible_and_validate_commands() {
        let mut config: Config = toml::from_str("schema_version = 1\n").unwrap();
        assert!(config.skills.enabled);
        assert!(config.skills.paths.is_empty());
        assert!(config.mcp_servers.is_empty());
        config.skills.paths = vec!["relative".into()];
        assert!(config.validate().is_err());
        config.skills.paths = vec!["/absolute/skill collection".into()];
        config.mcp_servers.insert(
            "local".into(),
            McpServerConfig {
                command: "/usr/bin/true".into(),
                enabled: true,
                ..Default::default()
            },
        );
        config.validate().unwrap();
        let roundtrip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        roundtrip.validate().unwrap();
        assert_eq!(roundtrip.skills.paths, config.skills.paths);
        config.mcp_servers.get_mut("local").unwrap().command = "relative".into();
        assert!(config.validate().is_err());
        config.mcp_servers.get_mut("local").unwrap().command = "/usr/bin/true".into();
        config.mcp_servers.get_mut("local").unwrap().inherit_env = vec!["BAD=VALUE".into()];
        assert!(config.validate().is_err());
    }
    #[test]
    fn storage_uses_ruvx_even_when_only_legacy_directories_exist() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        for root in ["config", "data", "cache"] {
            let root = base.join(root);
            assert_eq!(app_directory(&root), root.join("ruvx"));
            fs::create_dir_all(root.join("kovr")).unwrap();
            assert_eq!(app_directory(&root), root.join("ruvx"));
        }
        let legacy_config = b"schema_version = 1\n[agent]\nmax_turns = 7\n";
        fs::write(base.join("config/kovr/config.toml"), legacy_config).unwrap();
        fs::write(
            base.join("data/kovr/last-model.json"),
            b"preserve existing data",
        )
        .unwrap();
        let paths = Paths {
            config: app_directory(&base.join("config")),
            data: app_directory(&base.join("data")),
            cache: app_directory(&base.join("cache")),
        };
        let mut config = paths.load().unwrap();
        assert_eq!(config.agent.max_turns, Config::default().agent.max_turns);
        assert!(config.providers.is_empty());
        config.agent.max_turns = 11;
        paths.save(&config).unwrap();
        assert_eq!(paths.load().unwrap().agent.max_turns, 11);
        assert_eq!(
            fs::read(base.join("config/kovr/config.toml")).unwrap(),
            legacy_config
        );
        assert_eq!(
            fs::read(base.join("data/kovr/last-model.json")).unwrap(),
            b"preserve existing data"
        );
        let protected = config.tools.protected_paths;
        for path in [
            base.join("config/ruvx"),
            base.join("config/kovr"),
            base.join("data/ruvx"),
            base.join("data/kovr"),
            base.join("cache/ruvx"),
            base.join("cache/kovr"),
        ] {
            assert!(protected.contains(&path));
        }
    }
    #[test]
    fn limits_and_unknown_fields() {
        assert_eq!(Config::default().agent.max_turns, 30);
        let config: Config = toml::from_str("[agent]\nmax_turns = 0").unwrap();
        config.validate().unwrap();
        assert!(toml::from_str::<Config>("[agent]\nmax_turns = -1").is_err());
        assert!(toml::from_str::<Config>("[agent]\nmax_turn = 2").is_err());
    }
    #[test]
    fn remote_transport_and_secrets_in_urls() {
        assert!(validate_url("http://127.0.0.1:8000/v1").is_ok());
        assert!(validate_url("http://example.com/v1").is_err());
        assert!(validate_url("https://secret@example.com/v1").is_err());
        assert!(validate_url("https://example.com/v1?key=secret").is_err());
    }
    #[test]
    fn effort_validation_keeps_legacy_defaults_and_requires_known_capabilities() {
        use ReasoningEffort as E;
        let old: ModelConfig = toml::from_str("supports_tools = true").unwrap();
        assert_eq!(old.reasoning_effort, None);
        old.validate_effort(ProviderKind::Compatible, "m").unwrap();
        let mut model = old;
        model.reasoning_effort = Some(E::High);
        assert!(
            model
                .validate_effort(ProviderKind::Compatible, "gpt-5.2")
                .is_err()
        );
        model
            .validate_effort(ProviderKind::Openai, "gpt-5.2")
            .unwrap();
        model.reasoning_effort = Some(E::None);
        assert!(
            model
                .validate_effort(ProviderKind::Openai, "gpt-6.2-astra")
                .is_err()
        );
        model.reasoning_efforts = Some(vec![E::None, E::High]);
        model
            .validate_effort(ProviderKind::Compatible, "alias")
            .unwrap();
        model.reasoning_efforts = Some(vec![]);
        assert!(
            model
                .validate_effort(ProviderKind::Compatible, "alias")
                .is_err()
        );
        model.reasoning_effort = None;
        model
            .validate_effort(ProviderKind::Compatible, "alias")
            .unwrap();
        model.reasoning_efforts = Some(vec![E::High, E::High]);
        assert!(
            model
                .validate_effort(ProviderKind::Compatible, "alias")
                .is_err()
        );
        assert!(toml::from_str::<ModelConfig>("reasoning_effort = 'super'").is_err());
    }
}
