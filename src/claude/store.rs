//! O que o assistente guarda entre sessões: chave da API, perfis de SDR,
//! análises dos leads, follow-ups e registros de envio. Fica num JSON
//! próprio (`claude.json`) ao lado das configurações do app.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::analysis::Analysis;
use super::followup::FollowUp;
use crate::model::ChatId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    Formal,
    #[default]
    Balanced,
    Casual,
}

impl Tone {
    pub const ALL: [Tone; 3] = [Tone::Formal, Tone::Balanced, Tone::Casual];

    pub fn label(self) -> &'static str {
        match self {
            Tone::Formal => "Formal",
            Tone::Balanced => "Equilibrado",
            Tone::Casual => "Descontraído",
        }
    }

    pub fn prompt(self) -> &'static str {
        match self {
            Tone::Formal => "formal e cordial, sem gírias",
            Tone::Balanced => "próximo e profissional, como uma conversa de WhatsApp entre adultos",
            Tone::Casual => "descontraído, leve, pode usar expressões do dia a dia",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorSpeed {
    Fast,
    #[default]
    Human,
    Slow,
}

impl CursorSpeed {
    pub const ALL: [CursorSpeed; 3] = [CursorSpeed::Fast, CursorSpeed::Human, CursorSpeed::Slow];

    pub fn label(self) -> &'static str {
        match self {
            CursorSpeed::Fast => "Rápida",
            CursorSpeed::Human => "Humana",
            CursorSpeed::Slow => "Lenta",
        }
    }

    /// Pontos por segundo que o cursor percorre.
    pub fn travel(self) -> f32 {
        match self {
            CursorSpeed::Fast => 2400.0,
            CursorSpeed::Human => 1100.0,
            CursorSpeed::Slow => 600.0,
        }
    }

    /// Caracteres digitados por segundo.
    pub fn typing(self) -> f32 {
        match self {
            CursorSpeed::Fast => 70.0,
            CursorSpeed::Human => 24.0,
            CursorSpeed::Slow => 10.0,
        }
    }
}

/// O que o Claude pode fazer no WhatsApp com este perfil.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub read: bool,
    pub navigate: bool,
    pub type_messages: bool,
    pub send_without_approval: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            read: true,
            navigate: true,
            type_messages: true,
            send_without_approval: false,
        }
    }
}

/// Um jeito de trabalhar de um SDR. Cada SDR monta o seu.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub tone: Tone,
    /// O que o SDR vende, para o Claude ter contexto.
    pub product: String,
    /// Como o Claude deve escrever e se comportar.
    pub instructions: String,
    /// Sinais de compra e critérios que o SDR usa para medir a temperatura.
    pub temperature_criteria: String,
    /// Objeções comuns e como o SDR gosta de contorná-las.
    pub objection_playbook: String,
    pub permissions: Permissions,
    pub cursor_speed: CursorSpeed,
    /// Analisa sozinho a conversa aberta quando chega mensagem nova.
    pub auto_analyse: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "SDR".into(),
            tone: Tone::Balanced,
            product: String::new(),
            instructions: "Chamar o lead pelo primeiro nome, mensagens curtas, no máximo uma pergunta por mensagem.".into(),
            temperature_criteria: "Sinais de compra: pediu preço, perguntou prazo, pediu contrato ou link de pagamento, marcou reunião.".into(),
            objection_playbook: String::new(),
            permissions: Permissions::default(),
            cursor_speed: CursorSpeed::Human,
            auto_analyse: true,
        }
    }
}

/// Regras que criam follow-ups sozinhas a partir da temperatura.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// Quente sem resposta em 24 h.
    pub hot_after_day: bool,
    /// Morno sem resposta em 3 dias.
    pub warm_after_three_days: bool,
    /// Frio há 7 dias.
    pub cold_after_week: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            hot_after_day: true,
            warm_after_three_days: true,
            cold_after_week: false,
        }
    }
}

/// Uma mensagem enviada pelo Claude, para as métricas do relatório.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sent {
    pub chat: ChatId,
    pub at: i64,
    pub follow_up: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedReport {
    pub markdown: String,
    pub created: i64,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub api_key: String,
    pub model: String,
    pub profiles: Vec<Profile>,
    pub active_profile: usize,
    pub analyses: HashMap<ChatId, Analysis>,
    pub follow_ups: Vec<FollowUp>,
    pub next_follow_up: u64,
    pub rules: Rules,
    pub sent: Vec<Sent>,
    pub report: Option<SavedReport>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: super::MODEL.into(),
            profiles: vec![Profile::default()],
            active_profile: 0,
            analyses: HashMap::new(),
            follow_ups: Vec::new(),
            next_follow_up: 1,
            rules: Rules::default(),
            sent: Vec::new(),
            report: None,
        }
    }
}

impl Store {
    pub fn path(config: &Path) -> PathBuf {
        config.join("claude.json")
    }

    pub fn load(config: &Path) -> Self {
        let mut store: Store = std::fs::read_to_string(Self::path(config))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if store.profiles.is_empty() {
            store.profiles.push(Profile::default());
        }
        if store.active_profile >= store.profiles.len() {
            store.active_profile = 0;
        }
        if store.model.trim().is_empty() {
            store.model = super::MODEL.into();
        }
        // Um follow-up que estava rodando quando o app fechou volta para a fila.
        for follow_up in &mut store.follow_ups {
            if follow_up.status == super::followup::Status::Running {
                follow_up.status = super::followup::Status::Scheduled;
            }
        }
        store
    }

    /// Grava num arquivo temporário e troca, para nunca deixar o JSON pela
    /// metade. A chave da API fica só para o usuário ler.
    pub fn save(&self, config: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config)?;
        let path = Self::path(config);
        let temporary = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&temporary, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temporary, &path)
    }

    pub fn profile(&self) -> &Profile {
        &self.profiles[self.active_profile.min(self.profiles.len() - 1)]
    }

    pub fn profile_mut(&mut self) -> &mut Profile {
        let index = self.active_profile.min(self.profiles.len() - 1);
        &mut self.profiles[index]
    }

    /// A chave salva ou, se não houver, a da variável de ambiente.
    pub fn key(&self) -> Option<String> {
        let saved = self.api_key.trim();
        if !saved.is_empty() {
            return Some(saved.to_owned());
        }
        std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
    }
}
