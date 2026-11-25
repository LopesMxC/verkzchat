//! Follow-ups agendados. Na hora marcada o Claude abre a conversa com o
//! cursor, revisa o rascunho com o contexto atual e digita (e envia, se o
//! perfil e o modo permitirem).

use serde::{Deserialize, Serialize};

use super::analysis::Temperature;
use super::store::Store;
use crate::app::App;
use crate::model::ChatId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Digita e espera o SDR aprovar o envio.
    #[default]
    AskApproval,
    /// Envia sozinho (só se o perfil permitir envio sem aprovação).
    AutoSend,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::AskApproval => "Pede aprovação",
            Mode::AutoSend => "Claude envia",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Scheduled,
    Running,
    Done,
    /// O lead respondeu antes, então não precisou.
    Skipped,
    Canceled,
    Failed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Scheduled => "Agendado",
            Status::Running => "Executando",
            Status::Done => "Feito",
            Status::Skipped => "Lead respondeu",
            Status::Canceled => "Cancelado",
            Status::Failed => "Falhou",
        }
    }

    pub fn open(self) -> bool {
        matches!(self, Status::Scheduled | Status::Running)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FollowUp {
    pub id: u64,
    pub chat: ChatId,
    pub chat_name: String,
    /// Unix, segundos.
    pub due: i64,
    pub draft: String,
    pub reason: String,
    pub mode: Mode,
    /// Só executa se o lead não tiver respondido desde o agendamento.
    pub only_if_no_reply: bool,
    pub created: i64,
    pub status: Status,
    pub note: String,
    /// Criado por uma regra automática.
    pub automatic: bool,
}

/// Formulário de novo follow-up no painel.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub open: bool,
    pub hours: f32,
    pub text: String,
    pub mode: Mode,
    pub only_if_no_reply: bool,
}

impl Draft {
    pub fn new() -> Self {
        Self {
            open: true,
            hours: 24.0,
            text: String::new(),
            mode: Mode::AskApproval,
            only_if_no_reply: true,
        }
    }
}

pub fn schedule(store: &mut Store, mut follow_up: FollowUp) -> u64 {
    follow_up.id = store.next_follow_up;
    store.next_follow_up += 1;
    if follow_up.created == 0 {
        follow_up.created = crate::util::now();
    }
    let id = follow_up.id;
    // Um agendamento novo para o mesmo lead substitui o que estava aberto.
    for other in store.follow_ups.iter_mut() {
        if other.chat == follow_up.chat && other.status == Status::Scheduled {
            other.status = Status::Canceled;
            other.note = "Substituído por um agendamento mais novo".into();
        }
    }
    store.follow_ups.push(follow_up);
    id
}

/// Se o lead mandou mensagem depois de `since`.
pub fn replied_since(app: &App, chat: &str, since: i64) -> bool {
    app.chat(chat).is_some_and(|chat| {
        chat.last.as_ref().is_some_and(|last| !last.from_me) && chat.last_activity > since
    })
}

/// O próximo follow-up vencido, pronto para rodar.
pub fn due(store: &mut Store, app: &App, now: i64) -> Option<FollowUp> {
    let mut chosen = None;
    for follow_up in store.follow_ups.iter_mut() {
        if follow_up.status != Status::Scheduled || follow_up.due > now {
            continue;
        }
        if follow_up.only_if_no_reply && replied_since(app, &follow_up.chat, follow_up.created) {
            follow_up.status = Status::Skipped;
            follow_up.note = "O lead respondeu antes do horário".into();
            continue;
        }
        if chosen.is_none() {
            chosen = Some(follow_up.clone());
        }
    }
    chosen
}

pub fn set_status(store: &mut Store, id: u64, status: Status, note: &str) {
    if let Some(follow_up) = store.follow_ups.iter_mut().find(|follow_up| follow_up.id == id) {
        follow_up.status = status;
        if !note.is_empty() {
            follow_up.note = note.into();
        }
    }
}

/// Cria follow-ups pelas regras de temperatura para leads parados.
pub fn apply_rules(store: &mut Store, app: &App, now: i64) {
    let rules = store.rules.clone();
    let mut new = Vec::new();
    for (chat_id, analysis) in &store.analyses {
        let Some(chat) = app.chat(chat_id) else { continue };
        // Só quando a última mensagem é do SDR: o lead está devendo resposta.
        if !chat.last.as_ref().is_some_and(|last| last.from_me) {
            continue;
        }
        let quiet = now - chat.last_activity;
        let wait = match analysis.temperature() {
            Temperature::Hot if rules.hot_after_day => 24 * 3600,
            Temperature::Warm if rules.warm_after_three_days => 3 * 24 * 3600,
            Temperature::Cold if rules.cold_after_week => 7 * 24 * 3600,
            _ => continue,
        };
        if quiet < wait {
            continue;
        }
        let already = store.follow_ups.iter().any(|follow_up| {
            follow_up.chat == *chat_id && (follow_up.status.open() || follow_up.created > chat.last_activity)
        });
        if already {
            continue;
        }
        let draft = analysis
            .follow_up
            .as_ref()
            .map(|idea| idea.draft.clone())
            .unwrap_or_default();
        new.push(FollowUp {
            chat: chat_id.clone(),
            chat_name: analysis.chat_name.clone(),
            due: now,
            draft,
            reason: format!("Regra automática: lead {} sem resposta", analysis.temperature().label().to_lowercase()),
            mode: Mode::AskApproval,
            only_if_no_reply: true,
            created: now,
            automatic: true,
            ..FollowUp::default()
        });
    }
    for follow_up in new {
        schedule(store, follow_up);
    }
}

pub fn when(due: i64) -> String {
    let now = jiff::Zoned::now();
    let Ok(at) = jiff::Timestamp::from_second(due) else {
        return String::new();
    };
    let at = at.to_zoned(jiff::tz::TimeZone::system());
    let hour = at.strftime("%H:%M").to_string();
    let days = (at.date() - now.date()).get_days();
    match days {
        ..=-1 => format!("{} · {hour}", at.strftime("%d/%m")),
        0 if due <= now.timestamp().as_second() => format!("Agora · {hour}"),
        0 => format!("Hoje · {hour}"),
        1 => format!("Amanhã · {hour}"),
        2..=6 => format!("{} · {hour}", weekday(at.weekday())),
        _ => format!("{} · {hour}", at.strftime("%d/%m")),
    }
}

fn weekday(day: jiff::civil::Weekday) -> &'static str {
    use jiff::civil::Weekday::*;
    match day {
        Monday => "Seg",
        Tuesday => "Ter",
        Wednesday => "Qua",
        Thursday => "Qui",
        Friday => "Sex",
        Saturday => "Sáb",
        Sunday => "Dom",
    }
}

/// Converte "2026-11-21T10:00" (hora local) em Unix.
pub fn parse_local(text: &str) -> Option<i64> {
    let text = text.trim();
    let civil: jiff::civil::DateTime = text
        .parse()
        .ok()
        .or_else(|| text.get(..16).and_then(|short| short.parse().ok()))?;
    civil
        .to_zoned(jiff::tz::TimeZone::system())
        .ok()
        .map(|zoned| zoned.timestamp().as_second())
}
