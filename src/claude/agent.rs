//! O chat do SDR com o Claude. O Claude responde e, quando precisa agir no
//! WhatsApp, chama ferramentas. Cada ferramenta que mexe no WhatsApp vira
//! passos do cursor, executados à vista do SDR.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::{Value, json};

use super::analysis::{self, Temperature};
use super::api;
use super::cursor::{Act, Effect, Outcome, Step, Target};
use super::followup::{self, FollowUp, Mode, Status};
use super::store::Permissions;
use super::ClaudeState;
use crate::app::App;
use crate::model::ChatId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub enum Item {
    User(String),
    Claude(String),
    Action { label: String, state: StepState },
    Notice(String),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Sdr,
    FollowUp(u64),
}

pub(crate) struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

enum Phase {
    Calling(Receiver<api::Reply>),
    Tools {
        pending: VecDeque<ToolUse>,
        results: Vec<Value>,
        current: Option<(ToolUse, usize)>,
    },
}

struct Run {
    messages: Vec<Value>,
    phase: Phase,
    origin: Origin,
    rounds: u32,
    request: String,
    last_text: String,
    /// O follow-up exige aprovação mesmo que o perfil permita enviar sozinho.
    force_approval: bool,
}

#[derive(Default)]
pub struct Agent {
    pub items: Vec<Item>,
    history: Vec<(String, String)>,
    run: Option<Run>,
    /// O Claude enviou alguma mensagem nesta execução.
    pub sent_in_run: bool,
}

impl Agent {
    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    pub fn origin(&self) -> Option<Origin> {
        self.run.as_ref().map(|run| run.origin)
    }

    /// Passo atual e total do plano, para a faixa "3 de 5".
    pub fn plan(&self) -> (usize, usize) {
        let start = self
            .items
            .iter()
            .rposition(|item| matches!(item, Item::User(_) | Item::Notice(_)))
            .unwrap_or(0);
        let actions: Vec<_> = self.items[start..]
            .iter()
            .filter_map(|item| match item {
                Item::Action { state, .. } => Some(*state),
                _ => None,
            })
            .collect();
        let done = actions.iter().filter(|state| **state != StepState::Running).count();
        (done + 1, actions.len().max(done + 1))
    }

    pub fn clear(&mut self) {
        if self.run.is_none() {
            self.items.clear();
            self.history.clear();
        }
    }
}

/// Começa um pedido do SDR ou um follow-up agendado.
pub fn start(state: &mut ClaudeState, app: &App, ctx: &egui::Context, request: String, origin: Origin, force_approval: bool) {
    let Some(key) = state.store.key() else {
        state.agent.items.push(Item::Error("Coloque a chave da API do Claude em Config.".into()));
        return;
    };
    if state.agent.run.is_some() || state.cursor.busy() {
        state.agent.items.push(Item::Notice("Espere o Claude terminar o que está fazendo.".into()));
        return;
    }
    match origin {
        Origin::Sdr => state.agent.items.push(Item::User(request.clone())),
        Origin::FollowUp(_) => state.agent.items.push(Item::Notice(request.lines().next().unwrap_or_default().to_owned())),
    }
    let mut messages = Vec::new();
    for (user, assistant) in state.agent.history.iter().rev().take(6).rev() {
        messages.push(json!({ "role": "user", "content": user }));
        messages.push(json!({ "role": "assistant", "content": assistant }));
    }
    let mut content = request.clone();
    if let Some(chat) = app.open_chat.clone().filter(|_| state.store.profile().permissions.read) {
        let text = analysis::transcript(app, &chat, 40);
        if !text.is_empty() {
            content += &format!(
                "\n\n<conversa_aberta nome=\"{}\" id=\"{chat}\">\n{text}\n</conversa_aberta>",
                app.chat_title_of(&chat)
            );
        }
    }
    messages.push(json!({ "role": "user", "content": content }));
    let body = body(state, app, &messages);
    state.agent.sent_in_run = false;
    state.agent.run = Some(Run {
        messages,
        phase: Phase::Calling(api::call(key, body, ctx.clone())),
        origin,
        rounds: 0,
        request,
        last_text: String::new(),
        force_approval,
    });
}

fn body(state: &ClaudeState, app: &App, messages: &[Value]) -> Value {
    json!({
        "model": state.store.model,
        "max_tokens": 4096,
        "system": system(state, app),
        "tools": tools(),
        "messages": messages,
    })
}

fn system(state: &ClaudeState, app: &App) -> String {
    let profile = state.store.profile();
    let now = jiff::Zoned::now().strftime("%A, %d/%m/%Y %H:%M").to_string();
    let open = app
        .open_chat
        .as_deref()
        .map(|chat| format!("{} (id {chat})", app.chat_title_of(chat)))
        .unwrap_or_else(|| "nenhuma".into());
    let mut s = format!(
        "Você é o Claude, assistente do SDR {name} dentro do VerkzChat, um cliente de WhatsApp. \
         Você atua no WhatsApp do SDR com ferramentas, e cada ação aparece na tela como um cursor \
         coral que o SDR acompanha.\n\nAgora: {now}\nConversa aberta: {open}\n",
        name = profile.name
    );
    if !profile.product.trim().is_empty() {
        s += &format!("O que o SDR vende: {}\n", profile.product.trim());
    }
    if !profile.objection_playbook.trim().is_empty() {
        s += &format!("Como o SDR contorna objeções: {}\n", profile.objection_playbook.trim());
    }
    s += &format!(
        "Tom das mensagens para leads: {}. {}\n\n",
        profile.tone.prompt(),
        profile.instructions.trim()
    );
    s += "Regras:\n\
          - Para mandar mensagem a um lead: type_message e depois send_message. Só diga que enviou depois que send_message confirmar.\n\
          - Não envie nada que o SDR não pediu, a não ser ao executar um follow-up agendado.\n\
          - O envio pode depender da aprovação do SDR; se ele cancelar, pare.\n\
          - Para agendar follow-ups use horário local no formato AAAA-MM-DDTHH:MM.\n\
          - Responda ao SDR em português do Brasil, curto e direto, sem enrolação.";
    s
}

pub(crate) fn tools() -> Value {
    json!([
        {
            "name": "list_chats",
            "description": "Lista as conversas mais recentes do WhatsApp com a temperatura de cada lead (quando já analisado). O cursor passa pela lista.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "temperature": { "type": "string", "enum": ["quente", "morno", "frio", "todas"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 60 }
                }
            }
        },
        {
            "name": "open_chat",
            "description": "Abre uma conversa clicando nela com o cursor. Aceita o nome do contato ou o id.",
            "input_schema": {
                "type": "object",
                "properties": { "chat": { "type": "string" } },
                "required": ["chat"]
            }
        },
        {
            "name": "read_chat",
            "description": "Abre (se preciso) e lê as últimas mensagens de uma conversa. Sem chat, lê a conversa aberta.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "chat": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 5, "maximum": 120 }
                }
            }
        },
        {
            "name": "type_message",
            "description": "Digita uma mensagem no campo de texto da conversa (abre a conversa se preciso). Não envia.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "chat": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "send_message",
            "description": "Clica em enviar na conversa aberta, enviando o que está no campo. Pode esperar a aprovação do SDR.",
            "input_schema": {
                "type": "object",
                "properties": { "chat": { "type": "string" } }
            }
        },
        {
            "name": "schedule_follow_up",
            "description": "Agenda um follow-up. Na hora, o Claude abre a conversa, revisa o rascunho e digita.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "chat": { "type": "string" },
                    "when": { "type": "string", "description": "AAAA-MM-DDTHH:MM no horário local" },
                    "draft": { "type": "string" },
                    "reason": { "type": "string" },
                    "auto_send": { "type": "boolean", "description": "Enviar sem pedir aprovação (só se o perfil permitir)" },
                    "only_if_no_reply": { "type": "boolean" }
                },
                "required": ["chat", "when", "draft"]
            }
        },
        {
            "name": "list_follow_ups",
            "description": "Lista os follow-ups agendados.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_sdr_profile",
            "description": "Devolve o perfil do SDR (o que vende, tom, como escrever, critérios de temperatura, como contorna objeções e permissões). Chame antes de escrever para leads.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "save_lead_analysis",
            "description": "Salva a análise do lead no painel do VerkzChat (temperatura, objeções, próxima mensagem). Leia a conversa antes.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "chat": { "type": "string" },
                    "score": { "type": "integer", "minimum": 0, "maximum": 100, "description": "0-33 frio, 34-66 morno, 67-100 quente" },
                    "reason": { "type": "string" },
                    "summary": { "type": "string" },
                    "stage": { "type": "string" },
                    "buying_signals": { "type": "array", "items": { "type": "string" } },
                    "objections": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string" },
                                "quote": { "type": "string", "description": "trecho exato do lead" },
                                "counter": { "type": "string" }
                            },
                            "required": ["label", "counter"]
                        }
                    },
                    "next_message": { "type": "string" },
                    "follow_up": {
                        "type": "object",
                        "properties": {
                            "when": { "type": "string" },
                            "in_hours": { "type": "number" },
                            "draft": { "type": "string" },
                            "reason": { "type": "string" }
                        }
                    }
                },
                "required": ["chat", "score", "reason", "summary", "objections", "next_message"]
            }
        },
        {
            "name": "list_due_follow_ups",
            "description": "Lista os follow-ups que já venceram e esperam execução.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "complete_follow_up",
            "description": "Marca um follow-up como feito, pulado (o lead respondeu ou não fazia sentido) ou falhou.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "status": { "type": "string", "enum": ["feito", "pulado", "falhou"] },
                    "note": { "type": "string" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "get_lead_analysis",
            "description": "Devolve a última análise do lead: temperatura, objeções, resumo e próxima mensagem sugerida.",
            "input_schema": {
                "type": "object",
                "properties": { "chat": { "type": "string" } },
                "required": ["chat"]
            }
        }
    ])
}

/// Acha um chat pelo id ou pelo nome.
pub fn find_chat(app: &App, query: &str) -> Option<ChatId> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if app.chat(query).is_some() {
        return Some(query.to_owned());
    }
    let wanted = query.to_lowercase();
    let mut best: Option<(u8, i64, ChatId)> = None;
    for chat in &app.chats {
        let title = app.chat_title(chat).to_lowercase();
        let rank = if title == wanted {
            3
        } else if title.starts_with(&wanted) {
            2
        } else if title.contains(&wanted) || wanted.contains(&title) {
            1
        } else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((r, at, _)) => rank > *r || (rank == *r && chat.last_activity > *at),
        };
        if better {
            best = Some((rank, chat.last_activity, chat.id.clone()));
        }
    }
    best.map(|(_, _, id)| id)
}

pub(crate) enum Begin {
    Now(String, bool),
    Cursor(Vec<Step>),
}

fn chat_arg(app: &App, input: &Value) -> Result<ChatId, String> {
    match input["chat"].as_str().filter(|text| !text.trim().is_empty()) {
        Some(query) => find_chat(app, query).ok_or_else(|| format!("Não achei nenhuma conversa com \"{query}\".")),
        None => app.open_chat.clone().ok_or_else(|| "Nenhuma conversa aberta. Informe o chat.".into()),
    }
}

fn open_steps(app: &App, chat: &str, permissions: &Permissions) -> Result<Vec<Step>, String> {
    if app.open_chat.as_deref() == Some(chat) {
        return Ok(Vec::new());
    }
    if !permissions.navigate {
        return Err("O perfil não permite navegar entre conversas.".into());
    }
    let name = app.chat_title_of(chat);
    Ok(vec![
        Step::new(
            Target::Row(chat.to_owned()),
            Act::Click(Effect::OpenChat(chat.to_owned())),
            format!("Claude · abrindo {name}"),
        ),
        Step::new(Target::Messages, Act::Wait(chat.to_owned()), "Claude · carregando conversa"),
    ])
}

pub(crate) fn describe(app: &App, tool: &ToolUse) -> String {
    let chat = || {
        tool.input["chat"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| app.open_chat.as_deref().map(|id| app.chat_title_of(id)))
            .unwrap_or_else(|| "a conversa".into())
    };
    match tool.name.as_str() {
        "list_chats" => "Ler a lista de conversas".into(),
        "open_chat" => format!("Abrir a conversa com {}", chat()),
        "read_chat" => format!("Ler a conversa com {}", chat()),
        "type_message" => "Digitar a mensagem no campo".into(),
        "send_message" => "Enviar a mensagem".into(),
        "schedule_follow_up" => format!("Agendar follow-up para {}", chat()),
        "list_follow_ups" => "Ver follow-ups agendados".into(),
        "get_lead_analysis" => format!("Ver a análise de {}", chat()),
        "get_sdr_profile" => "Ler o perfil do SDR".into(),
        "save_lead_analysis" => format!("Salvar a análise de {}", chat()),
        "list_due_follow_ups" => "Ver follow-ups vencidos".into(),
        "complete_follow_up" => "Fechar um follow-up".into(),
        other => other.into(),
    }
}

pub(crate) fn begin(state: &mut ClaudeState, app: &App, force_approval: bool, tool: &ToolUse) -> Begin {
    let permissions = state.store.profile().permissions.clone();
    let input = &tool.input;
    let denied = |what: &str| Begin::Now(format!("O perfil do SDR não permite {what}."), false);
    match tool.name.as_str() {
        "list_chats" => {
            if !permissions.read {
                return denied("ler conversas");
            }
            Begin::Cursor(vec![Step::new(Target::List, Act::Scroll, "Claude · lendo a lista")])
        }
        "open_chat" => {
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            match open_steps(app, &chat, &permissions) {
                Ok(steps) if steps.is_empty() => Begin::Now("A conversa já está aberta.".into(), true),
                Ok(steps) => Begin::Cursor(steps),
                Err(error) => Begin::Now(error, false),
            }
        }
        "read_chat" => {
            if !permissions.read {
                return denied("ler conversas");
            }
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            let mut steps = match open_steps(app, &chat, &permissions) {
                Ok(steps) => steps,
                Err(error) => return Begin::Now(error, false),
            };
            steps.push(Step::new(Target::Messages, Act::Wait(chat.clone()), "Claude · carregando conversa"));
            steps.push(Step::new(Target::Messages, Act::Scroll, "Claude · lendo conversa"));
            Begin::Cursor(steps)
        }
        "type_message" => {
            if !permissions.type_messages {
                return denied("digitar mensagens");
            }
            let Some(text) = input["text"].as_str().filter(|text| !text.trim().is_empty()) else {
                return Begin::Now("Faltou o texto da mensagem.".into(), false);
            };
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            let mut steps = match open_steps(app, &chat, &permissions) {
                Ok(steps) => steps,
                Err(error) => return Begin::Now(error, false),
            };
            steps.push(Step::new(Target::Composer, Act::Click(Effect::None), "Claude · indo ao campo"));
            steps.push(Step::new(Target::Composer, Act::Type(chat, text.trim().to_owned()), "Claude · digitando"));
            Begin::Cursor(steps)
        }
        "send_message" => {
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            if app.open_chat.as_deref() != Some(chat.as_str()) {
                return Begin::Now("Essa conversa não está aberta. Use type_message nela antes.".into(), false);
            }
            if app.composer.trim().is_empty() {
                return Begin::Now("O campo de mensagem está vazio. Use type_message antes.".into(), false);
            }
            let mut steps = vec![Step::new(Target::Send, Act::Move, "Claude · indo enviar")];
            if force_approval || !permissions.send_without_approval {
                steps.push(Step::new(Target::Send, Act::Approve(chat.clone()), "Claude · esperando sua aprovação"));
            }
            steps.push(Step::new(Target::Send, Act::Click(Effect::Send(chat)), "Claude · enviando"));
            Begin::Cursor(steps)
        }
        "schedule_follow_up" => {
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            let when = input["when"].as_str().unwrap_or_default();
            let Some(due) = followup::parse_local(when) else {
                return Begin::Now(format!("Não entendi o horário \"{when}\". Use AAAA-MM-DDTHH:MM."), false);
            };
            let auto = input["auto_send"].as_bool().unwrap_or(false) && permissions.send_without_approval;
            let follow_up = FollowUp {
                chat: chat.clone(),
                chat_name: app.chat_title_of(&chat),
                due,
                draft: input["draft"].as_str().unwrap_or_default().to_owned(),
                reason: input["reason"].as_str().unwrap_or_default().to_owned(),
                mode: if auto { Mode::AutoSend } else { Mode::AskApproval },
                only_if_no_reply: input["only_if_no_reply"].as_bool().unwrap_or(true),
                ..FollowUp::default()
            };
            followup::schedule(&mut state.store, follow_up);
            state.dirty = true;
            Begin::Now(format!("Follow-up agendado para {}.", followup::when(due)), true)
        }
        "list_follow_ups" => {
            let open: Vec<String> = state
                .store
                .follow_ups
                .iter()
                .filter(|follow_up| follow_up.status.open())
                .map(|follow_up| {
                    format!(
                        "#{} {} · {} · {} · \"{}\"",
                        follow_up.id,
                        follow_up.chat_name,
                        followup::when(follow_up.due),
                        follow_up.mode.label(),
                        follow_up.draft
                    )
                })
                .collect();
            if open.is_empty() {
                Begin::Now("Nenhum follow-up agendado.".into(), true)
            } else {
                Begin::Now(open.join("\n"), true)
            }
        }
        "get_lead_analysis" => {
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            match state.store.analyses.get(&chat) {
                Some(analysis) => Begin::Now(serde_json::to_string(analysis).unwrap_or_default(), true),
                None => Begin::Now("Esse lead ainda não foi analisado.".into(), true),
            }
        }
        "get_sdr_profile" => {
            let profile = state.store.profile();
            Begin::Now(
                serde_json::to_string_pretty(&json!({
                    "sdr": profile.name,
                    "tom": profile.tone.prompt(),
                    "o_que_vende": profile.product,
                    "como_escrever": profile.instructions,
                    "criterios_de_temperatura": profile.temperature_criteria,
                    "como_contorna_objecoes": profile.objection_playbook,
                    "faixas": "0-33 frio, 34-66 morno, 67-100 quente",
                    "pode_ler": permissions.read,
                    "pode_navegar": permissions.navigate,
                    "pode_digitar": permissions.type_messages,
                    "envia_sem_aprovacao": permissions.send_without_approval,
                    "conversa_aberta": app.open_chat.as_deref().map(|chat| app.chat_title_of(chat)),
                    "agora": jiff::Zoned::now().strftime("%A, %d/%m/%Y %H:%M").to_string(),
                }))
                .unwrap_or_default(),
                true,
            )
        }
        "save_lead_analysis" => {
            let chat = match chat_arg(app, input) {
                Ok(chat) => chat,
                Err(error) => return Begin::Now(error, false),
            };
            let mut value = input.clone();
            if let Some(object) = value.as_object_mut() {
                object.remove("chat");
            }
            let mut analysis: analysis::Analysis = match serde_json::from_value(value) {
                Ok(analysis) => analysis,
                Err(error) => return Begin::Now(format!("Análise em formato inválido: {error}"), false),
            };
            analysis.score = analysis.score.min(100);
            analysis.analysed_at = crate::util::now();
            analysis.last_message = analysis::last_message_id(app, &chat).unwrap_or_default();
            analysis.chat_name = app.chat_title_of(&chat);
            if analysis.follow_up.as_ref().is_some_and(|idea| idea.draft.trim().is_empty()) {
                analysis.follow_up = None;
            }
            let label = format!("{} {}", analysis.temperature().label(), analysis.score);
            state.store.analyses.insert(chat.clone(), analysis);
            state.analysis_errors.remove(&chat);
            state.dirty = true;
            Begin::Now(format!("Análise salva no painel do VerkzChat ({label})."), true)
        }
        "list_due_follow_ups" => {
            let now = crate::util::now();
            let due: Vec<Value> = state
                .store
                .follow_ups
                .iter()
                .filter(|follow_up| follow_up.status == Status::Scheduled && follow_up.due <= now)
                .map(|follow_up| {
                    json!({
                        "id": follow_up.id,
                        "chat": follow_up.chat,
                        "lead": follow_up.chat_name,
                        "quando": followup::when(follow_up.due),
                        "rascunho": follow_up.draft,
                        "motivo": follow_up.reason,
                        "lead_respondeu_desde_o_agendamento": followup::replied_since(app, &follow_up.chat, follow_up.created),
                        "so_se_nao_responder": follow_up.only_if_no_reply,
                        "pede_aprovacao": follow_up.mode == Mode::AskApproval,
                    })
                })
                .collect();
            if due.is_empty() {
                Begin::Now("Nenhum follow-up vencido agora.".into(), true)
            } else {
                Begin::Now(serde_json::to_string_pretty(&due).unwrap_or_default(), true)
            }
        }
        "complete_follow_up" => {
            let Some(id) = input["id"].as_u64() else {
                return Begin::Now("Informe o id do follow-up.".into(), false);
            };
            let status = match input["status"].as_str().unwrap_or("feito") {
                "pulado" | "skipped" => Status::Skipped,
                "falhou" | "failed" => Status::Failed,
                "cancelado" | "canceled" => Status::Canceled,
                _ => Status::Done,
            };
            followup::set_status(&mut state.store, id, status, input["note"].as_str().unwrap_or_default());
            if status == Status::Done
                && let Some(chat) = state.store.follow_ups.iter().find(|item| item.id == id).map(|item| item.chat.clone())
                && let Some(sent) = state
                    .store
                    .sent
                    .iter_mut()
                    .rev()
                    .find(|sent| sent.chat == chat && crate::util::now() - sent.at < 3600)
            {
                sent.follow_up = true;
            }
            state.dirty = true;
            Begin::Now(format!("Follow-up #{id} marcado como {}.", status.label()), true)
        }
        other => Begin::Now(format!("Ferramenta desconhecida: {other}"), false),
    }
}

/// O resultado de uma ferramenta de cursor, depois que o cursor terminou.
pub(crate) fn complete(state: &ClaudeState, app: &App, tool: &ToolUse) -> String {
    match tool.name.as_str() {
        "list_chats" => {
            let filter = tool.input["temperature"].as_str().unwrap_or("todas");
            let limit = tool.input["limit"].as_u64().unwrap_or(25) as usize;
            let mut chats: Vec<_> = app.chats.iter().filter(|chat| !chat.archived).collect();
            chats.sort_by_key(|chat| std::cmp::Reverse(chat.last_activity));
            let lines: Vec<String> = chats
                .into_iter()
                .filter_map(|chat| {
                    let analysis = state.store.analyses.get(&chat.id);
                    let temperature = analysis.map(|analysis| analysis.temperature());
                    let keep = match filter {
                        "quente" => temperature == Some(Temperature::Hot),
                        "morno" => temperature == Some(Temperature::Warm),
                        "frio" => temperature == Some(Temperature::Cold),
                        _ => true,
                    };
                    keep.then(|| {
                        let last = chat
                            .last
                            .as_ref()
                            .map(|last| format!("{}: {}", if last.from_me { "SDR" } else { "lead" }, last.summary))
                            .unwrap_or_default();
                        let temp = analysis
                            .map(|analysis| format!("{} {}", analysis.temperature().label(), analysis.score))
                            .unwrap_or_else(|| "sem análise".into());
                        format!(
                            "{} | id {} | {} | {} | {}",
                            app.chat_title(chat),
                            chat.id,
                            analysis::clock(chat.last_activity),
                            last,
                            temp
                        )
                    })
                })
                .take(limit)
                .collect();
            if lines.is_empty() {
                "Nenhuma conversa com esse filtro.".into()
            } else {
                lines.join("\n")
            }
        }
        "open_chat" => format!(
            "Conversa com {} aberta.",
            app.open_chat.as_deref().map(|chat| app.chat_title_of(chat)).unwrap_or_default()
        ),
        "read_chat" => {
            let chat = app.open_chat.clone().unwrap_or_default();
            let limit = tool.input["limit"].as_u64().unwrap_or(40) as usize;
            let text = analysis::transcript(app, &chat, limit);
            if text.is_empty() {
                "A conversa não tem mensagens carregadas.".into()
            } else {
                format!("Conversa com {}:\n{text}", app.chat_title_of(&chat))
            }
        }
        "type_message" => "Mensagem digitada no campo, ainda não enviada.".into(),
        "send_message" => "Mensagem enviada.".into(),
        _ => "Feito.".into(),
    }
}

fn tool_result(id: &str, text: String, error: bool) -> Value {
    json!({ "type": "tool_result", "tool_use_id": id, "content": text, "is_error": error })
}

pub(crate) fn set_item(state: &mut ClaudeState, index: usize, new: StepState) {
    if let Some(Item::Action { state: step, .. }) = state.agent.items.get_mut(index) {
        *step = new;
    }
}

/// Avança a execução: lê respostas da API e dispara as ferramentas.
pub fn tick(state: &mut ClaudeState, app: &App, ctx: &egui::Context) {
    let Some(mut run) = state.agent.run.take() else {
        return;
    };
    loop {
        match &mut run.phase {
            Phase::Calling(receiver) => match receiver.try_recv() {
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return end(state, run, Err("A chamada ao Claude caiu.".into()));
                }
                Ok(Err(error)) => return end(state, run, Err(error)),
                Ok(Ok(reply)) => {
                    let content = reply["content"].clone();
                    run.messages.push(json!({ "role": "assistant", "content": content }));
                    let text = api::text_of(&reply);
                    if !text.trim().is_empty() {
                        state.agent.items.push(Item::Claude(text.trim().to_owned()));
                        run.last_text = text.trim().to_owned();
                    }
                    let pending: VecDeque<ToolUse> = content
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|block| block["type"] == "tool_use")
                        .map(|block| ToolUse {
                            id: block["id"].as_str().unwrap_or_default().to_owned(),
                            name: block["name"].as_str().unwrap_or_default().to_owned(),
                            input: block["input"].clone(),
                        })
                        .collect();
                    if pending.is_empty() {
                        return end(state, run, Ok(()));
                    }
                    run.phase = Phase::Tools {
                        pending,
                        results: Vec::new(),
                        current: None,
                    };
                }
            },
            Phase::Tools { .. } => {
                // A ferramenta em andamento terminou?
                let finished = match &run.phase {
                    Phase::Tools { current: Some(_), .. } => state.cursor.outcome.take(),
                    _ => None,
                };
                if let Phase::Tools { current: Some(_), .. } = &run.phase {
                    let Some(outcome) = finished else { break };
                    let Phase::Tools { results, current, .. } = &mut run.phase else { unreachable!() };
                    let (tool, index) = current.take().expect("ferramenta atual");
                    match outcome {
                        Outcome::Done => {
                            set_item(state, index, StepState::Done);
                            results.push(tool_result(&tool.id, complete(state, app, &tool), false));
                        }
                        Outcome::Canceled(reason) => {
                            set_item(state, index, StepState::Failed);
                            state.agent.items.push(Item::Notice(reason.clone()));
                            return end(state, run, Err(reason));
                        }
                    }
                    continue;
                }
                let next = match &mut run.phase {
                    Phase::Tools { pending, .. } => pending.pop_front(),
                    _ => None,
                };
                match next {
                    Some(tool) => {
                        let index = state.agent.items.len();
                        state.agent.items.push(Item::Action {
                            label: describe(app, &tool),
                            state: StepState::Running,
                        });
                        match begin(state, app, run.force_approval, &tool) {
                            Begin::Now(text, ok) => {
                                set_item(state, index, if ok { StepState::Done } else { StepState::Failed });
                                if let Phase::Tools { results, .. } = &mut run.phase {
                                    results.push(tool_result(&tool.id, text, !ok));
                                }
                            }
                            Begin::Cursor(steps) => {
                                state.cursor.start(steps);
                                if let Phase::Tools { current, .. } = &mut run.phase {
                                    *current = Some((tool, index));
                                }
                                break;
                            }
                        }
                    }
                    None => {
                        let Phase::Tools { results, .. } = std::mem::replace(
                            &mut run.phase,
                            Phase::Tools {
                                pending: VecDeque::new(),
                                results: Vec::new(),
                                current: None,
                            },
                        ) else {
                            unreachable!()
                        };
                        run.messages.push(json!({ "role": "user", "content": results }));
                        run.rounds += 1;
                        if run.rounds >= 16 {
                            return end(state, run, Err("O pedido passou do limite de etapas.".into()));
                        }
                        let Some(key) = state.store.key() else {
                            return end(state, run, Err("Sem chave da API.".into()));
                        };
                        let body = body(state, app, &run.messages);
                        run.phase = Phase::Calling(api::call(key, body, ctx.clone()));
                        break;
                    }
                }
            }
        }
    }
    state.agent.run = Some(run);
}

/// Interrompe o que estiver rodando (o SDR retomou o controle).
pub fn interrupt(state: &mut ClaudeState, reason: &str) {
    state.cursor.cancel(reason);
    if let Some(run) = state.agent.run.take() {
        for item in state.agent.items.iter_mut() {
            if let Item::Action { state: step, .. } = item
                && *step == StepState::Running
            {
                *step = StepState::Failed;
            }
        }
        state.agent.items.push(Item::Notice(reason.into()));
        end(state, run, Err(reason.into()));
    }
}

fn end(state: &mut ClaudeState, run: Run, result: Result<(), String>) {
    if let Err(error) = &result
        && !state.agent.items.iter().rev().take(1).any(|item| matches!(item, Item::Notice(_)))
    {
        state.agent.items.push(Item::Error(error.clone()));
    }
    state
        .agent
        .history
        .push((run.request.clone(), if run.last_text.is_empty() { "(sem resposta)".into() } else { run.last_text.clone() }));
    if state.agent.history.len() > 12 {
        state.agent.history.remove(0);
    }
    if let Origin::FollowUp(id) = run.origin {
        let (status, note) = match &result {
            Ok(()) if state.agent.sent_in_run => (Status::Done, "Mensagem enviada".to_owned()),
            Ok(()) => (Status::Done, run.last_text.chars().take(160).collect()),
            Err(error) => (Status::Failed, error.clone()),
        };
        followup::set_status(&mut state.store, id, status, &note);
        state.dirty = true;
    }
}
