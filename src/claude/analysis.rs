//! Análise de um lead: temperatura, objeções, próxima mensagem e follow-up.

use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::api;
use super::store::Profile;
use crate::app::App;
use crate::model::{ChatId, Content, Message};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temperature {
    Cold,
    Warm,
    Hot,
}

impl Temperature {
    pub fn of(score: u8) -> Self {
        match score {
            0..=33 => Temperature::Cold,
            34..=66 => Temperature::Warm,
            _ => Temperature::Hot,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Temperature::Cold => "Frio",
            Temperature::Warm => "Morno",
            Temperature::Hot => "Quente",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Temperature::Cold => egui::Color32::from_rgb(0x58, 0x94, 0xDC),
            Temperature::Warm => egui::Color32::from_rgb(0xE6, 0xA0, 0x28),
            Temperature::Hot => egui::Color32::from_rgb(0xE5, 0x53, 0x3D),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Objection {
    /// Nome curto ("Preço", "Decisão com sócio").
    pub label: String,
    /// Trecho do lead que mostra a objeção.
    pub quote: String,
    /// Como contornar.
    pub counter: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FollowUpIdea {
    /// Quando fazer, em linguagem natural ("Amanhã às 10h").
    pub when: String,
    /// Daqui a quantas horas, para agendar.
    pub in_hours: f32,
    pub draft: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Analysis {
    pub score: u8,
    pub reason: String,
    pub summary: String,
    /// Etapa da negociação ("Descoberta", "Proposta", "Negociação"...).
    pub stage: String,
    pub buying_signals: Vec<String>,
    pub objections: Vec<Objection>,
    pub next_message: String,
    pub follow_up: Option<FollowUpIdea>,
    /// Unix, segundos.
    pub analysed_at: i64,
    /// A última mensagem que a análise viu, para saber quando refazer.
    pub last_message: String,
    pub chat_name: String,
}

impl Analysis {
    pub fn temperature(&self) -> Temperature {
        Temperature::of(self.score)
    }
}

/// O texto de uma mensagem, inteiro quando é texto.
pub fn message_text(message: &Message) -> String {
    match &message.content {
        Content::Text { text, .. } | Content::Interactive { text, .. } => text.clone(),
        other => format!("[{}]", other.summary()),
    }
}

pub fn clock(timestamp: i64) -> String {
    jiff::Timestamp::from_second(timestamp)
        .map(|at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%d/%m %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// As últimas `limit` mensagens carregadas de um chat, da mais antiga para a
/// mais nova, uma por linha.
pub fn transcript(app: &App, chat: &str, limit: usize) -> String {
    let Some(conversation) = app.conversations.get(chat) else {
        return String::new();
    };
    let title = app.chat_title_of(chat);
    let messages = &conversation.messages;
    let start = messages.len().saturating_sub(limit);
    messages[start..]
        .iter()
        .map(|message| {
            let who = if message.from_me {
                "SDR".to_owned()
            } else {
                message
                    .sender_name
                    .clone()
                    .filter(|_| app.chat(chat).is_some_and(|chat| chat.is_group()))
                    .unwrap_or_else(|| title.clone())
            };
            format!("[{}] {who}: {}", clock(message.timestamp), message_text(message))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Id da última mensagem carregada do chat.
pub fn last_message_id(app: &App, chat: &str) -> Option<String> {
    app.conversations
        .get(chat)
        .and_then(|conversation| conversation.messages.last())
        .map(|message| message.id.clone())
}

pub type Job = Receiver<api::Reply>;

pub fn start(key: String, model: &str, profile: &Profile, app: &App, chat: &ChatId, ctx: &egui::Context) -> Option<Job> {
    let text = transcript(app, chat, 80);
    if text.trim().is_empty() {
        return None;
    }
    let name = app.chat_title_of(chat);
    let prompt = prompt(profile, &name, &text);
    let body = json!({
        "model": model,
        "max_tokens": 2500,
        "messages": [{ "role": "user", "content": prompt }],
    });
    Some(api::call(key, body, ctx.clone()))
}

pub fn finish(reply: &Value, chat_name: String, last_message: String) -> Result<Analysis, String> {
    let text = api::text_of(reply);
    let value = api::json_in(&text).ok_or_else(|| "O Claude não devolveu a análise no formato esperado.".to_owned())?;
    let mut analysis: Analysis =
        serde_json::from_value(value).map_err(|error| format!("Análise inválida: {error}"))?;
    analysis.score = analysis.score.min(100);
    analysis.analysed_at = crate::util::now();
    analysis.last_message = last_message;
    analysis.chat_name = chat_name;
    if analysis.follow_up.as_ref().is_some_and(|idea| idea.draft.trim().is_empty()) {
        analysis.follow_up = None;
    }
    Ok(analysis)
}

fn prompt(profile: &Profile, name: &str, transcript: &str) -> String {
    let now = jiff::Zoned::now().strftime("%A, %d/%m/%Y %H:%M").to_string();
    let mut p = String::from(
        "Você é o assistente de vendas de um SDR que atende leads pelo WhatsApp. \
         Analise a conversa e responda só com um objeto JSON.\n\n",
    );
    p += &format!("Agora: {now}\nSDR: {}\n", profile.name);
    if !profile.product.trim().is_empty() {
        p += &format!("O que o SDR vende: {}\n", profile.product.trim());
    }
    if !profile.temperature_criteria.trim().is_empty() {
        p += &format!("Critérios de temperatura do SDR: {}\n", profile.temperature_criteria.trim());
    }
    if !profile.objection_playbook.trim().is_empty() {
        p += &format!("Como o SDR contorna objeções: {}\n", profile.objection_playbook.trim());
    }
    p += &format!(
        "Tom das mensagens sugeridas: {}. {}\n\n",
        profile.tone.prompt(),
        profile.instructions.trim()
    );
    p += &format!("Lead: {name}\nConversa (SDR = o vendedor):\n<conversa>\n{transcript}\n</conversa>\n\n");
    p += r#"Formato:
{
  "score": 0-100,
  "reason": "uma frase explicando a temperatura",
  "summary": "duas frases sobre onde a negociação está",
  "stage": "Primeiro contato | Descoberta | Apresentação | Proposta | Negociação | Fechamento | Perdido",
  "buying_signals": ["sinais de compra que apareceram"],
  "objections": [{"label": "nome curto", "quote": "trecho exato do lead", "counter": "como contornar, em uma ou duas frases"}],
  "next_message": "a próxima mensagem que o SDR deve mandar agora, pronta para enviar",
  "follow_up": {"when": "quando, em português", "in_hours": horas a partir de agora, "draft": "mensagem do follow-up", "reason": "por que esse momento"} ou null
}
Regras: 0-33 é frio, 34-66 é morno, 67-100 é quente. Só liste objeções que o lead realmente fez, com o trecho exato. Se a última mensagem é do lead, follow_up é null (a resposta é a next_message). Escreva em português do Brasil e não invente fatos sobre o produto."#;
    p
}
