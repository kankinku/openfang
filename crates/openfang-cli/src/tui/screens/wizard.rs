//! Setup wizard: provider list → API key → gateway auth → model → config save.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;

use crate::tui::theme;

/// Provider metadata for the setup wizard.
struct ProviderInfo {
    name: &'static str,
    env_var: &'static str,
    default_model: &'static str,
    needs_key: bool,
}

const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        name: "groq",
        env_var: "GROQ_API_KEY",
        default_model: "llama-3.3-70b-versatile",
        needs_key: true,
    },
    ProviderInfo {
        name: "anthropic",
        env_var: "ANTHROPIC_API_KEY",
        default_model: "claude-sonnet-4-20250514",
        needs_key: true,
    },
    ProviderInfo {
        name: "openai",
        env_var: "OPENAI_API_KEY",
        default_model: "gpt-4o",
        needs_key: true,
    },
    ProviderInfo {
        name: "openrouter",
        env_var: "OPENROUTER_API_KEY",
        default_model: "anthropic/claude-sonnet-4-20250514",
        needs_key: true,
    },
    ProviderInfo {
        name: "deepseek",
        env_var: "DEEPSEEK_API_KEY",
        default_model: "deepseek-chat",
        needs_key: true,
    },
    ProviderInfo {
        name: "together",
        env_var: "TOGETHER_API_KEY",
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        needs_key: true,
    },
    ProviderInfo {
        name: "mistral",
        env_var: "MISTRAL_API_KEY",
        default_model: "mistral-large-latest",
        needs_key: true,
    },
    ProviderInfo {
        name: "fireworks",
        env_var: "FIREWORKS_API_KEY",
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        needs_key: true,
    },
    ProviderInfo {
        name: "ollama",
        env_var: "OLLAMA_API_KEY",
        default_model: "llama3.2",
        needs_key: false,
    },
    ProviderInfo {
        name: "vllm",
        env_var: "VLLM_API_KEY",
        default_model: "local-model",
        needs_key: false,
    },
    ProviderInfo {
        name: "lmstudio",
        env_var: "LMSTUDIO_API_KEY",
        default_model: "local-model",
        needs_key: false,
    },
];

/// Check if first-run setup is needed.
pub fn needs_setup() -> bool {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return true,
    };
    !home.join(".openfang").join("config.toml").exists()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Provider,
    ApiKey,
    Model,
    Saving,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GatewayAuthMode {
    Token,
    Password,
    None,
    TrustedProxy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApiSetupPhase {
    ProviderKey,
    GatewayAuth,
}

pub struct WizardState {
    pub step: WizardStep,
    pub provider_list: ListState,
    pub provider_order: Vec<usize>, // indices into PROVIDERS, detected first
    pub selected_provider: Option<usize>, // index into PROVIDERS
    pub api_key_input: String,
    pub api_key_from_env: bool,
    api_setup_phase: ApiSetupPhase,
    gateway_auth_list: ListState,
    gateway_auth_mode: GatewayAuthMode,
    gateway_auth_secret_input: String,
    gateway_auth_error: String,
    pub model_input: String,
    pub status_msg: String,
    pub created_config: Option<PathBuf>,
}

impl WizardState {
    pub fn new() -> Self {
        let mut state = Self {
            step: WizardStep::Provider,
            provider_list: ListState::default(),
            provider_order: Vec::new(),
            selected_provider: None,
            api_key_input: String::new(),
            api_key_from_env: false,
            api_setup_phase: ApiSetupPhase::ProviderKey,
            gateway_auth_list: ListState::default(),
            gateway_auth_mode: GatewayAuthMode::Token,
            gateway_auth_secret_input: String::new(),
            gateway_auth_error: String::new(),
            model_input: String::new(),
            status_msg: String::new(),
            created_config: None,
        };
        state.build_provider_order();
        state.provider_list.select(Some(0));
        state.gateway_auth_list.select(Some(0));
        state
    }

    pub fn reset(&mut self) {
        self.step = WizardStep::Provider;
        self.selected_provider = None;
        self.api_key_input.clear();
        self.api_key_from_env = false;
        self.api_setup_phase = ApiSetupPhase::ProviderKey;
        self.gateway_auth_mode = GatewayAuthMode::Token;
        self.gateway_auth_secret_input.clear();
        self.gateway_auth_error.clear();
        self.model_input.clear();
        self.status_msg.clear();
        self.created_config = None;
        self.build_provider_order();
        self.provider_list.select(Some(0));
        self.gateway_auth_list.select(Some(0));
    }

    fn build_provider_order(&mut self) {
        self.provider_order.clear();
        // Detected providers first
        for (i, p) in PROVIDERS.iter().enumerate() {
            if std::env::var(p.env_var).is_ok() {
                self.provider_order.push(i);
            }
        }
        // Then the rest
        for (i, p) in PROVIDERS.iter().enumerate() {
            if std::env::var(p.env_var).is_err() {
                self.provider_order.push(i);
            }
        }
    }

    fn selected_provider_info(&self) -> Option<&'static ProviderInfo> {
        self.selected_provider.map(|i| &PROVIDERS[i])
    }

    fn selected_gateway_auth_mode(&self) -> GatewayAuthMode {
        match self.gateway_auth_list.selected().unwrap_or(0) {
            0 => GatewayAuthMode::Token,
            1 => GatewayAuthMode::Password,
            2 => GatewayAuthMode::None,
            _ => GatewayAuthMode::TrustedProxy,
        }
    }

    fn gateway_auth_needs_secret(&self) -> bool {
        matches!(
            self.selected_gateway_auth_mode(),
            GatewayAuthMode::Token | GatewayAuthMode::Password
        )
    }

    fn persist_gateway_auth_secret(&mut self) -> Result<(), String> {
        let mode = self.selected_gateway_auth_mode();
        self.gateway_auth_mode = mode;
        self.gateway_auth_error.clear();

        let token_env = "OPENFANG_API_TOKEN";
        let password_env = "OPENFANG_API_PASSWORD";
        match mode {
            GatewayAuthMode::Token => {
                let secret = self.gateway_auth_secret_input.trim();
                if secret.is_empty() {
                    return Err("Enter a gateway token".to_string());
                }
                crate::dotenv::save_env_key(token_env, secret)?;
                let _ = crate::dotenv::remove_env_key(password_env);
                std::env::set_var(token_env, secret);
                std::env::remove_var(password_env);
            }
            GatewayAuthMode::Password => {
                let secret = self.gateway_auth_secret_input.trim();
                if secret.is_empty() {
                    return Err("Enter a gateway password".to_string());
                }
                crate::dotenv::save_env_key(password_env, secret)?;
                let _ = crate::dotenv::remove_env_key(token_env);
                std::env::set_var(password_env, secret);
                std::env::remove_var(token_env);
            }
            GatewayAuthMode::None | GatewayAuthMode::TrustedProxy => {
                let _ = crate::dotenv::remove_env_key(token_env);
                let _ = crate::dotenv::remove_env_key(password_env);
                std::env::remove_var(token_env);
                std::env::remove_var(password_env);
            }
        }
        Ok(())
    }

    /// Handle a key event. Returns true if wizard is complete or cancelled.
    /// `cancelled` is set if the user backed out entirely.
    pub fn handle_key(&mut self, key: KeyEvent) -> WizardResult {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return WizardResult::Cancelled;
        }

        match self.step {
            WizardStep::Provider => self.handle_provider(key),
            WizardStep::ApiKey => self.handle_api_key(key),
            WizardStep::Model => self.handle_model(key),
            WizardStep::Saving | WizardStep::Done => WizardResult::Continue,
        }
    }

    fn handle_provider(&mut self, key: KeyEvent) -> WizardResult {
        match key.code {
            KeyCode::Esc => return WizardResult::Cancelled,
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.provider_list.selected().unwrap_or(0);
                let next = if i == 0 {
                    self.provider_order.len() - 1
                } else {
                    i - 1
                };
                self.provider_list.select(Some(next));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.provider_list.selected().unwrap_or(0);
                let next = (i + 1) % self.provider_order.len();
                self.provider_list.select(Some(next));
            }
            KeyCode::Enter => {
                if let Some(list_idx) = self.provider_list.selected() {
                    let Some(&prov_idx) = self.provider_order.get(list_idx) else {
                        return WizardResult::Continue;
                    };
                    let Some(p) = PROVIDERS.get(prov_idx) else {
                        return WizardResult::Continue;
                    };
                    self.selected_provider = Some(prov_idx);

                    if !p.needs_key {
                        // No key needed, skip to gateway auth
                        self.api_key_from_env = false;
                        self.model_input = p.default_model.to_string();
                        self.api_setup_phase = ApiSetupPhase::GatewayAuth;
                        self.step = WizardStep::ApiKey;
                    } else if std::env::var(p.env_var).is_ok() {
                        // Key already in env
                        self.api_key_from_env = true;
                        self.model_input = p.default_model.to_string();
                        self.api_setup_phase = ApiSetupPhase::GatewayAuth;
                        self.step = WizardStep::ApiKey;
                    } else {
                        self.api_key_from_env = false;
                        self.api_setup_phase = ApiSetupPhase::ProviderKey;
                        self.api_key_input.clear();
                        self.step = WizardStep::ApiKey;
                    }
                }
            }
            _ => {}
        }
        WizardResult::Continue
    }

    fn handle_api_key(&mut self, key: KeyEvent) -> WizardResult {
        match self.api_setup_phase {
            ApiSetupPhase::ProviderKey => match key.code {
                KeyCode::Esc => {
                    self.step = WizardStep::Provider;
                }
                KeyCode::Enter => {
                    if !self.api_key_input.is_empty() {
                        if let Some(p) = self.selected_provider_info() {
                            let _ = crate::dotenv::save_env_key(p.env_var, &self.api_key_input);
                            std::env::set_var(p.env_var, &self.api_key_input);
                            self.model_input = p.default_model.to_string();
                        }
                        self.api_setup_phase = ApiSetupPhase::GatewayAuth;
                    }
                }
                KeyCode::Char(c) => {
                    self.api_key_input.push(c);
                }
                KeyCode::Backspace => {
                    self.api_key_input.pop();
                }
                _ => {}
            },
            ApiSetupPhase::GatewayAuth => match key.code {
                KeyCode::Esc => {
                    if let Some(p) = self.selected_provider_info() {
                        if p.needs_key && !self.api_key_from_env {
                            self.api_setup_phase = ApiSetupPhase::ProviderKey;
                        } else {
                            self.step = WizardStep::Provider;
                        }
                    } else {
                        self.step = WizardStep::Provider;
                    }
                }
                KeyCode::Up => {
                    let i = self.gateway_auth_list.selected().unwrap_or(0);
                    let next = if i == 0 { 3 } else { i - 1 };
                    self.gateway_auth_list.select(Some(next));
                    self.gateway_auth_error.clear();
                }
                KeyCode::Down => {
                    let i = self.gateway_auth_list.selected().unwrap_or(0);
                    let next = (i + 1) % 4;
                    self.gateway_auth_list.select(Some(next));
                    self.gateway_auth_error.clear();
                }
                KeyCode::Enter => match self.persist_gateway_auth_secret() {
                    Ok(()) => {
                        self.gateway_auth_error.clear();
                        self.step = WizardStep::Model;
                    }
                    Err(e) => {
                        self.gateway_auth_error = e;
                    }
                },
                KeyCode::Char(c) => {
                    if self.gateway_auth_needs_secret() {
                        self.gateway_auth_secret_input.push(c);
                        self.gateway_auth_error.clear();
                    }
                }
                KeyCode::Backspace => {
                    if self.gateway_auth_needs_secret() {
                        self.gateway_auth_secret_input.pop();
                        self.gateway_auth_error.clear();
                    }
                }
                _ => {}
            },
        }
        WizardResult::Continue
    }

    fn handle_model(&mut self, key: KeyEvent) -> WizardResult {
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::ApiKey;
                self.api_setup_phase = ApiSetupPhase::GatewayAuth;
            }
            KeyCode::Enter => {
                self.step = WizardStep::Saving;
                self.save_config();
            }
            KeyCode::Char(c) => {
                self.model_input.push(c);
            }
            KeyCode::Backspace => {
                self.model_input.pop();
            }
            _ => {}
        }
        WizardResult::Continue
    }

    fn save_config(&mut self) {
        let p = match self.selected_provider_info() {
            Some(p) => p,
            None => {
                self.status_msg = "No provider selected".to_string();
                self.step = WizardStep::Provider;
                return;
            }
        };

        let home = match dirs::home_dir() {
            Some(h) => h,
            None => {
                self.status_msg = "Could not determine home directory".to_string();
                self.step = WizardStep::Done;
                return;
            }
        };

        let openfang_dir = home.join(".openfang");
        let _ = std::fs::create_dir_all(openfang_dir.join("agents"));
        let _ = std::fs::create_dir_all(openfang_dir.join("data"));
        crate::restrict_dir_permissions(&openfang_dir);

        let model = if self.model_input.is_empty() {
            p.default_model
        } else {
            &self.model_input
        };

        let api_auth_mode = match self.gateway_auth_mode {
            GatewayAuthMode::Token => "token",
            GatewayAuthMode::Password => "password",
            GatewayAuthMode::None => "none",
            GatewayAuthMode::TrustedProxy => "trusted_proxy",
        };
        let trusted_proxy_section =
            if matches!(self.gateway_auth_mode, GatewayAuthMode::TrustedProxy) {
                r#"
[api_auth.trusted_proxy]
user_header = "x-openfang-user"
trusted_ips = []
"#
            } else {
                ""
            };

        let config = format!(
            r#"# OpenFang Agent OS configuration
# Generated by setup wizard

api_listen = "127.0.0.1:4200"

[api_auth]
mode = "{api_auth_mode}"
token_env = "OPENFANG_API_TOKEN"
password_env = "OPENFANG_API_PASSWORD"
{trusted_proxy_section}

[default_model]
provider = "{provider}"
model = "{model}"
api_key_env = "{api_key_env}"

[memory]
decay_rate = 0.05
"#,
            provider = p.name,
            api_key_env = p.env_var,
        );

        let config_path = openfang_dir.join("config.toml");
        match std::fs::write(&config_path, &config) {
            Ok(()) => {
                crate::restrict_file_permissions(&config_path);
                self.status_msg = format!("Config saved \u{2014} {} / {}", p.name, model);
                self.created_config = Some(config_path);
            }
            Err(e) => {
                self.status_msg = format!("Failed to save config: {e}");
            }
        }
        self.step = WizardStep::Done;
    }
}

pub enum WizardResult {
    Continue,
    Cancelled,
}

/// Render the wizard screen.
pub fn draw(f: &mut Frame, area: Rect, state: &mut WizardState) {
    // Fill background
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(theme::BG_PRIMARY)),
        area,
    );

    let step_label = match state.step {
        WizardStep::Provider => "Step 1 of 3",
        WizardStep::ApiKey => "Step 2 of 3",
        WizardStep::Model => "Step 3 of 3",
        WizardStep::Saving => "Saving...",
        WizardStep::Done => "Complete",
    };

    // Left-aligned content area
    let content = if area.width < 10 || area.height < 5 {
        area
    } else {
        let margin = 3u16.min(area.width.saturating_sub(10));
        let w = 72u16.min(area.width.saturating_sub(margin));
        Rect {
            x: area.x.saturating_add(margin),
            y: area.y,
            width: w,
            height: area.height,
        }
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // top pad
        Constraint::Length(1), // header
        Constraint::Length(1), // separator
        Constraint::Min(1),    // step content
    ])
    .split(content);

    // Header
    let header = Line::from(vec![
        Span::styled(
            "Setup",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {step_label}"), theme::dim_style()),
    ]);
    f.render_widget(Paragraph::new(header), chunks[1]);

    // Separator
    let sep_w = content.width.min(60) as usize;
    let sep = Line::from(vec![Span::styled(
        "\u{2500}".repeat(sep_w),
        Style::default().fg(theme::BORDER),
    )]);
    f.render_widget(Paragraph::new(sep), chunks[2]);

    match state.step {
        WizardStep::Provider => draw_provider(f, chunks[3], state),
        WizardStep::ApiKey => draw_api_key(f, chunks[3], state),
        WizardStep::Model => draw_model(f, chunks[3], state),
        WizardStep::Saving | WizardStep::Done => draw_done(f, chunks[3], state),
    }
}

fn draw_provider(f: &mut Frame, area: Rect, state: &mut WizardState) {
    let chunks = Layout::vertical([
        Constraint::Length(2), // prompt
        Constraint::Min(3),    // list
        Constraint::Length(1), // hints
    ])
    .split(area);

    let prompt = Paragraph::new(Line::from(vec![Span::raw("  Choose your LLM provider:")]));
    f.render_widget(prompt, chunks[0]);

    let items: Vec<ListItem> = state
        .provider_order
        .iter()
        .map(|&idx| {
            let p = &PROVIDERS[idx];
            let hint = if !p.needs_key {
                "local, no key needed".to_string()
            } else if std::env::var(p.env_var).is_ok() {
                format!("{} detected", p.env_var)
            } else {
                format!("requires {}", p.env_var)
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("  {:<14}", p.name)),
                Span::styled(hint, theme::dim_style()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(theme::ACCENT)
                .bg(theme::BG_HOVER)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{25b8} ");

    f.render_stateful_widget(list, chunks[1], &mut state.provider_list);

    let hints = Paragraph::new(Line::from(vec![Span::styled(
        "    [\u{2191}\u{2193}] Navigate  [Enter] Select  [Esc] Cancel",
        theme::hint_style(),
    )]));
    f.render_widget(hints, chunks[2]);
}

fn draw_api_key(f: &mut Frame, area: Rect, state: &mut WizardState) {
    if state.api_setup_phase == ApiSetupPhase::GatewayAuth {
        draw_gateway_auth(f, area, state);
        return;
    }

    let p = match state.selected_provider_info() {
        Some(p) => p,
        None => return,
    };

    let chunks = Layout::vertical([
        Constraint::Length(2), // prompt
        Constraint::Length(1), // input
        Constraint::Length(2), // spacer + hint about env var
        Constraint::Min(0),    // spacer
        Constraint::Length(1), // hints
    ])
    .split(area);

    let prompt = Paragraph::new(Line::from(vec![Span::raw(format!(
        "  Enter your {} API key:",
        p.name
    ))]));
    f.render_widget(prompt, chunks[0]);

    // Masked input
    let masked: String = "\u{2022}".repeat(state.api_key_input.len());
    let input = Paragraph::new(Line::from(vec![
        Span::raw("  > "),
        Span::styled(&masked, theme::input_style()),
        Span::styled(
            "\u{2588}",
            Style::default()
                .fg(theme::GREEN)
                .add_modifier(Modifier::SLOW_BLINK),
        ),
    ]));
    f.render_widget(input, chunks[1]);

    let env_hint = Paragraph::new(Line::from(vec![Span::styled(
        format!("    Or set {} environment variable", p.env_var),
        theme::dim_style(),
    )]));
    f.render_widget(env_hint, chunks[2]);

    let hints = Paragraph::new(Line::from(vec![Span::styled(
        "    [Enter] Confirm  [Esc] Back",
        theme::hint_style(),
    )]));
    f.render_widget(hints, chunks[4]);
}

fn draw_gateway_auth(f: &mut Frame, area: Rect, state: &mut WizardState) {
    let needs_secret = state.gateway_auth_needs_secret();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::raw("  Gateway API auth mode:")])),
        chunks[0],
    );

    let items = vec![
        ListItem::new(Line::from(vec![
            Span::raw("  Token"),
            Span::styled("  (recommended) Bearer token", theme::dim_style()),
        ])),
        ListItem::new(Line::from(vec![
            Span::raw("  Password"),
            Span::styled("  x-openfang-password header", theme::dim_style()),
        ])),
        ListItem::new(Line::from(vec![
            Span::raw("  Localhost only"),
            Span::styled("  local access only", theme::dim_style()),
        ])),
        ListItem::new(Line::from(vec![
            Span::raw("  Trusted proxy"),
            Span::styled("  advanced (trusted IPs required)", theme::dim_style()),
        ])),
    ];
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(theme::ACCENT)
                .bg(theme::BG_HOVER)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{25b8} ");
    f.render_stateful_widget(list, chunks[1], &mut state.gateway_auth_list);

    if needs_secret {
        let env_var = match state.selected_gateway_auth_mode() {
            GatewayAuthMode::Token => "OPENFANG_API_TOKEN",
            GatewayAuthMode::Password => "OPENFANG_API_PASSWORD",
            GatewayAuthMode::None | GatewayAuthMode::TrustedProxy => "",
        };
        let masked: String = "\u{2022}".repeat(state.gateway_auth_secret_input.len());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  > "),
                Span::styled(masked, theme::input_style()),
                Span::styled(
                    "\u{2588}",
                    Style::default()
                        .fg(theme::GREEN)
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
            ])),
            chunks[2],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                format!("    Saved as {env_var} in ~/.openfang/.env"),
                theme::dim_style(),
            )])),
            chunks[3],
        );
    } else {
        let msg = match state.selected_gateway_auth_mode() {
            GatewayAuthMode::None => "    Localhost-only mode selected",
            GatewayAuthMode::TrustedProxy => {
                "    Configure api_auth.trusted_proxy.trusted_ips after setup"
            }
            _ => "",
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(msg, theme::dim_style())])),
            chunks[2],
        );
    }

    if !state.gateway_auth_error.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                format!("    \u{26a0} {}", state.gateway_auth_error),
                Style::default().fg(theme::YELLOW),
            )])),
            chunks[4],
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "    [\u{2191}\u{2193}] Navigate  [Enter] Confirm  [Esc] Back",
            theme::hint_style(),
        )])),
        chunks[5],
    );
}

fn draw_model(f: &mut Frame, area: Rect, state: &mut WizardState) {
    let p = match state.selected_provider_info() {
        Some(p) => p,
        None => return,
    };

    let chunks = Layout::vertical([
        Constraint::Length(2), // prompt
        Constraint::Length(1), // input
        Constraint::Length(2), // default hint
        Constraint::Min(0),
        Constraint::Length(1), // hints
    ])
    .split(area);

    let prompt = Paragraph::new(Line::from(vec![Span::raw("  Model name:")]));
    f.render_widget(prompt, chunks[0]);

    let display_text = if state.model_input.is_empty() {
        p.default_model
    } else {
        &state.model_input
    };
    let input = Paragraph::new(Line::from(vec![
        Span::raw("  > "),
        Span::styled(display_text, theme::input_style()),
        Span::styled(
            "\u{2588}",
            Style::default()
                .fg(theme::GREEN)
                .add_modifier(Modifier::SLOW_BLINK),
        ),
    ]));
    f.render_widget(input, chunks[1]);

    let default_hint = Paragraph::new(Line::from(vec![Span::styled(
        format!("    default: {}", p.default_model),
        theme::dim_style(),
    )]));
    f.render_widget(default_hint, chunks[2]);

    let hints = Paragraph::new(Line::from(vec![Span::styled(
        "    [Enter] Confirm  [Esc] Back",
        theme::hint_style(),
    )]));
    f.render_widget(hints, chunks[4]);
}

fn draw_done(f: &mut Frame, area: Rect, state: &WizardState) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);

    let icon = if state.created_config.is_some() {
        Span::styled("  \u{2714} ", Style::default().fg(theme::GREEN))
    } else {
        Span::styled("  \u{2718} ", Style::default().fg(theme::RED))
    };

    let msg = Paragraph::new(Line::from(vec![icon, Span::raw(&state.status_msg)]));
    f.render_widget(msg, chunks[0]);

    if state.created_config.is_some() {
        let cont = Paragraph::new(Line::from(vec![Span::styled(
            "    Continuing...",
            theme::dim_style(),
        )]));
        f.render_widget(cont, chunks[1]);
    }
}
