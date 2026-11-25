//! Assistente Claude para SDRs dentro do VerkzChat: lê as conversas, mede a
//! temperatura do lead, aponta objeções e como contorná-las, faz follow-ups
//! agendados e monta relatórios. Tudo o que ele faz no WhatsApp aparece na
//! tela como um cursor coral, e o SDR pode retomar o controle a qualquer hora.

pub mod agent;
pub mod analysis;
pub mod api;
pub mod bridge;
pub mod connect;
pub mod cursor;
pub mod followup;
pub mod mcp;
pub mod report;
pub mod store;

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::app::App;
use crate::model::ChatId;

/// Modelo padrão de todas as chamadas.
pub const MODEL: &str = "claude-opus-5-5";

/// Espera depois da última mensagem antes de analisar sozinho.
const SETTLE: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Analysis,
    Chat,
    FollowUps,
    Report,
    Config,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Analysis, Tab::Chat, Tab::FollowUps, Tab::Report, Tab::Config];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Analysis => "Análise",
            Tab::Chat => "Chat",
            Tab::FollowUps => "Follow-ups",
            Tab::Report => "Relatório",
            Tab::Config => "Config",
        }
    }
}

pub struct ClaudeState {
    pub open: bool,
    pub tab: Tab,
    pub store: store::Store,
    pub analysis_jobs: HashMap<ChatId, (Receiver<api::Reply>, String)>,
    pub analysis_errors: HashMap<ChatId, String>,
    pub agent: agent::Agent,
    pub cursor: cursor::Executor,
    pub report_job: Option<Receiver<api::Reply>>,
    pub report_error: Option<String>,
    pub chat_input: String,
    pub new_follow_up: followup::Draft,
    pub report_days: u32,
    pub show_key: bool,
    pub dirty: bool,
    loaded: bool,
    last_save: Option<Instant>,
    last_rules: Option<Instant>,
    /// A última mensagem vista em cada chat e quando, para o auto-análise.
    seen: HashMap<ChatId, (String, Instant)>,
    /// Arquivo do último relatório salvo, para abrir.
    pub saved_status: Option<String>,
    /// O canal com o Claude Desktop / Claude Code.
    bridge: Option<bridge::Bridge>,
    pub bridge_error: Option<String>,
    external: VecDeque<bridge::Request>,
    external_current: Option<(bridge::Request, agent::ToolUse, usize)>,
    /// Quando o Claude Desktop / Claude Code pediu algo pela última vez.
    pub last_external: Option<Instant>,
    /// Resultado do último botão de conexão (deu certo, mensagem).
    pub connect_status: Option<(bool, String)>,
    /// O login do Claude Code em andamento, que espera o navegador.
    pub login_job: Option<Receiver<Result<String, String>>>,
}

impl Default for ClaudeState {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Analysis,
            store: store::Store::default(),
            analysis_jobs: HashMap::new(),
            analysis_errors: HashMap::new(),
            agent: agent::Agent::default(),
            cursor: cursor::Executor::default(),
            report_job: None,
            report_error: None,
            chat_input: String::new(),
            new_follow_up: followup::Draft::default(),
            report_days: 7,
            show_key: false,
            dirty: false,
            loaded: false,
            last_save: None,
            last_rules: None,
            seen: HashMap::new(),
            saved_status: None,
            bridge: None,
            bridge_error: None,
            external: VecDeque::new(),
            external_current: None,
            last_external: None,
            connect_status: None,
            login_job: None,
        }
    }
}

impl ClaudeState {
    /// Se o Claude está agindo no WhatsApp agora.
    pub fn in_control(&self) -> bool {
        self.cursor.busy()
    }

    /// Se o Claude Desktop / Claude Code falou com o app nos últimos minutos.
    pub fn desktop_connected(&self) -> bool {
        self.external_current.is_some() || self.last_external.is_some_and(|at| at.elapsed() < Duration::from_secs(600))
    }

    pub fn save(&mut self, config: &std::path::Path) {
        if let Err(error) = self.store.save(config) {
            log::warn!("não consegui salvar claude.json: {error}");
        }
        self.dirty = false;
        self.last_save = Some(Instant::now());
    }
}

/// Pede uma análise do chat (se já não houver uma em andamento).
pub fn analyse(state: &mut ClaudeState, app: &App, chat: &ChatId, ctx: &egui::Context) {
    if state.analysis_jobs.contains_key(chat) {
        return;
    }
    let Some(key) = state.store.key() else {
        state.analysis_errors.insert(chat.clone(), "Coloque a chave da API do Claude em Config.".into());
        return;
    };
    if !state.store.profile().permissions.read {
        state.analysis_errors.insert(chat.clone(), "O perfil não permite ler conversas.".into());
        return;
    }
    let last = analysis::last_message_id(app, chat).unwrap_or_default();
    match analysis::start(key, &state.store.model, state.store.profile(), app, chat, ctx) {
        Some(job) => {
            state.analysis_errors.remove(chat);
            state.analysis_jobs.insert(chat.clone(), (job, last));
        }
        None => {
            state.analysis_errors.insert(chat.clone(), "A conversa ainda não tem mensagens carregadas.".into());
        }
    }
}

/// Uma volta do assistente por quadro: respostas da API, cursor, follow-ups.
pub fn tick(app: &mut App, ctx: &egui::Context) {
    let mut state = std::mem::take(&mut app.claude);
    if !state.loaded {
        state.store = store::Store::load(&app.dirs.config);
        state.loaded = true;
    }
    if state.bridge.is_none() && state.bridge_error.is_none() {
        match bridge::start(&app.dirs.runtime, ctx.clone()) {
            Ok(bridge) => state.bridge = Some(bridge),
            Err(error) => state.bridge_error = Some(format!("Não consegui abrir o canal para o Claude Desktop: {error}")),
        }
    }
    tick_with(&mut state, app, ctx);
    if state.dirty && state.last_save.is_none_or(|at| at.elapsed() > Duration::from_secs(2)) {
        state.save(&app.dirs.config);
    }
    app.claude = state;
}

fn tick_with(state: &mut ClaudeState, app: &mut App, ctx: &egui::Context) {
    // Análises que chegaram.
    collect_analyses(state, app);

    // O cursor e as ferramentas do agente.
    let speed = state.store.profile().cursor_speed;
    if let Some(cursor::Event::Sent(chat)) = state.cursor.tick(app, speed, ctx) {
        let follow_up = matches!(state.agent.origin(), Some(agent::Origin::FollowUp(_)));
        state.store.sent.push(store::Sent {
            chat,
            at: crate::util::now(),
            follow_up,
        });
        state.agent.sent_in_run = true;
        state.dirty = true;
    }
    agent::tick(state, app, ctx);
    external(state, app, ctx);

    // Relatório.
    if let Some(receiver) = &state.report_job {
        match receiver.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                state.report_job = None;
                state.report_error = Some("A chamada ao Claude caiu.".into());
            }
            Ok(Err(error)) => {
                state.report_job = None;
                state.report_error = Some(error);
            }
            Ok(Ok(reply)) => {
                state.report_job = None;
                let markdown = api::text_of(&reply);
                let path = report::save(&markdown, app.settings.download_folder.clone(), &app.dirs.state);
                state.saved_status = path.as_ref().map(|path| format!("Salvo em {}", path.display()));
                state.store.report = Some(store::SavedReport {
                    markdown,
                    created: crate::util::now(),
                    path,
                });
                state.dirty = true;
            }
        }
    }

    if !app.is_linked() {
        return;
    }
    let now = crate::util::now();

    // Auto-análise da conversa aberta quando chega mensagem nova.
    if state.store.profile().auto_analyse
        && state.store.key().is_some()
        && let Some(chat) = app.open_chat.clone()
        && let Some(last) = analysis::last_message_id(app, &chat)
    {
        let entry = state.seen.entry(chat.clone()).or_insert_with(|| (last.clone(), Instant::now()));
        if entry.0 != last {
            *entry = (last.clone(), Instant::now());
        }
        let analysed = state
            .store
            .analyses
            .get(&chat)
            .is_some_and(|analysis| analysis.last_message == last);
        let failed = state.analysis_errors.contains_key(&chat);
        if !analysed && !failed && entry.1.elapsed() >= SETTLE && !state.analysis_jobs.contains_key(&chat) {
            analyse(state, app, &chat, ctx);
        } else if !analysed && !failed {
            ctx.request_repaint_after(SETTLE);
        }
    }

    // Regras automáticas de follow-up, de minuto em minuto.
    if state.last_rules.is_none_or(|at| at.elapsed() > Duration::from_secs(60)) {
        state.last_rules = Some(Instant::now());
        let before = state.store.follow_ups.len();
        followup::apply_rules(&mut state.store, app, now);
        if state.store.follow_ups.len() != before {
            state.dirty = true;
        }
    }

    // Follow-up vencido: com chave de API o Claude do app assume sozinho. Sem
    // ela, quem executa é o Claude Desktop / Claude Code (list_due_follow_ups).
    if state.store.key().is_some() && !state.agent.running() && !state.cursor.busy() && state.external_current.is_none() {
        let before: Vec<_> = state.store.follow_ups.iter().map(|follow_up| follow_up.status).collect();
        if let Some(follow_up) = followup::due(&mut state.store, app, now) {
            followup::set_status(&mut state.store, follow_up.id, followup::Status::Running, "");
            let request = format!(
                "Follow-up agendado de {name} disparou ({when}).\n\
                 Execute o follow-up para {name} (id {chat}). Motivo: {reason}. Rascunho: \"{draft}\".\n\
                 Leia a conversa com read_chat. Se o rascunho não fizer mais sentido, ajuste ao contexto atual. \
                 Depois digite com type_message e envie com send_message. Se não fizer sentido mandar nada agora, \
                 explique em uma frase e não envie.",
                name = follow_up.chat_name,
                when = followup::when(follow_up.due),
                chat = follow_up.chat,
                reason = if follow_up.reason.is_empty() { "retomar o contato" } else { &follow_up.reason },
                draft = follow_up.draft,
            );
            let force = follow_up.mode == followup::Mode::AskApproval;
            state.open = true;
            state.tab = Tab::Chat;
            agent::start(state, app, ctx, request, agent::Origin::FollowUp(follow_up.id), force);
            state.dirty = true;
        } else if state.store.follow_ups.iter().map(|follow_up| follow_up.status).ne(before) {
            state.dirty = true;
        }
        // Acorda a tempo para o próximo agendamento.
        if let Some(next) = state
            .store
            .follow_ups
            .iter()
            .filter(|follow_up| follow_up.status == followup::Status::Scheduled)
            .map(|follow_up| follow_up.due)
            .min()
        {
            let wait = (next - now).clamp(1, 3600) as u64;
            ctx.request_repaint_after(Duration::from_secs(wait));
        }
    }
}

/// Pedidos do Claude Desktop / Claude Code: um por vez, com o mesmo cursor e a
/// mesma aprovação de envio do chat do painel.
fn external(state: &mut ClaudeState, app: &App, ctx: &egui::Context) {
    if let Some(bridge) = &state.bridge {
        while let Ok(request) = bridge.requests.try_recv() {
            state.external.push_back(request);
        }
    }
    if let Some((_, tool, index)) = &state.external_current {
        let Some(outcome) = state.cursor.outcome.take() else { return };
        let (tool, index) = (agent::ToolUse { id: String::new(), name: tool.name.clone(), input: tool.input.clone() }, *index);
        let (request, _, _) = state.external_current.take().expect("pedido atual");
        match outcome {
            cursor::Outcome::Done => {
                agent::set_item(state, index, agent::StepState::Done);
                let _ = request.reply.send(Ok(agent::complete(state, app, &tool)));
            }
            cursor::Outcome::Canceled(reason) => {
                agent::set_item(state, index, agent::StepState::Failed);
                state.agent.items.push(agent::Item::Notice(reason.clone()));
                let _ = request.reply.send(Err(reason));
            }
        }
    }
    if state.external_current.is_some() || state.agent.running() || state.cursor.busy() {
        return;
    }
    let Some(request) = state.external.pop_front() else { return };
    state.last_external = Some(Instant::now());
    let tool = agent::ToolUse { id: String::new(), name: request.tool.clone(), input: request.args.clone() };
    let index = state.agent.items.len();
    state.agent.items.push(agent::Item::Action {
        label: format!("Claude Desktop · {}", agent::describe(app, &tool)),
        state: agent::StepState::Running,
    });
    match agent::begin(state, app, false, &tool) {
        agent::Begin::Now(text, ok) => {
            agent::set_item(state, index, if ok { agent::StepState::Done } else { agent::StepState::Failed });
            let _ = request.reply.send(if ok { Ok(text) } else { Err(text) });
        }
        agent::Begin::Cursor(steps) => {
            state.open = true;
            state.tab = Tab::Chat;
            state.cursor.start(steps);
            state.external_current = Some((request, tool, index));
            ctx.request_repaint();
        }
    }
}

fn collect_analyses(state: &mut ClaudeState, app: &App) {
    let mut done = Vec::new();
    for (chat, (receiver, last)) in &state.analysis_jobs {
        match receiver.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => done.push((chat.clone(), Err("A chamada ao Claude caiu.".to_owned()), last.clone())),
            Ok(result) => done.push((chat.clone(), result, last.clone())),
        }
    }
    for (chat, result, last) in done {
        state.analysis_jobs.remove(&chat);
        let outcome = result.and_then(|reply| analysis::finish(&reply, app.chat_title_of(&chat), last));
        match outcome {
            Ok(analysis) => {
                state.analysis_errors.remove(&chat);
                state.store.analyses.insert(chat, analysis);
                state.dirty = true;
            }
            Err(error) => {
                state.analysis_errors.insert(chat, error);
            }
        }
    }
}

/// Estados de exemplo do painel para capturas de tela (`--demo-page claude…`).
#[cfg(any(test, feature = "demo"))]
pub fn demo(app: &mut App, page: &str) {
    use analysis::{Analysis, FollowUpIdea, Objection};
    let Some(chat) = app.open_chat.clone().or_else(|| {
        app.chats
            .iter()
            .find(|chat| !chat.is_group() && app.conversations.get(&chat.id).is_some_and(|c| !c.messages.is_empty()))
            .map(|chat| chat.id.clone())
    }) else {
        return;
    };
    app.open_chat = Some(chat.clone());
    let name = app.chat_title_of(&chat);
    let now = crate::util::now();
    let mut state = ClaudeState {
        open: true,
        loaded: true,
        ..ClaudeState::default()
    };
    let profile = state.store.profile_mut();
    profile.name = "Lucas".into();
    profile.product = "Gestão de tráfego pago para lojas de roupa física".into();
    profile.objection_playbook = "Preço: mostrar case de loja parecida. Sócio: chamar os dois para uma call de 15 min.".into();
    state.store.analyses.insert(
        chat.clone(),
        Analysis {
            score: 82,
            reason: "Pediu preço e engajou rápido. A objeção é de momento e decisão compartilhada, não de interesse.".into(),
            summary: "Loja de roupa física, sem anúncios hoje. Quer resultado, mas achou o valor alto e precisa alinhar com o sócio.".into(),
            stage: "Negociação".into(),
            buying_signals: vec!["Perguntou o preço".into(), "Respondeu em minutos".into()],
            objections: vec![
                Objection {
                    label: "Preço".into(),
                    quote: "tá um pouco caro pra mim agora".into(),
                    counter: "Ancore no retorno: mostre o resultado de uma loja parecida no 1º mês e ofereça começar com verba menor.".into(),
                },
                Objection {
                    label: "Decisão com sócio".into(),
                    quote: "preciso ver com meu sócio antes".into(),
                    counter: "Convide o sócio para uma call de 15 min e mande um resumo de 1 página para encaminhar.".into(),
                },
            ],
            next_message: "Super entendo! Que tal uma call rápida de 15 min com você e seu sócio amanhã? Mostro o resultado de uma loja parecida com a sua.".into(),
            follow_up: Some(FollowUpIdea {
                when: "Amanhã, 10:00".into(),
                in_hours: 20.0,
                draft: "Oi! Conseguiu falar com seu sócio? Separei o case daquela loja para mostrar na call.".into(),
                reason: "Dar tempo para a conversa com o sócio".into(),
            }),
            analysed_at: now - 120,
            last_message: analysis::last_message_id(app, &chat).unwrap_or_default(),
            chat_name: name.clone(),
        },
    );
    for (index, other) in app.chats.iter().filter(|other| other.id != chat).take(5).enumerate() {
        let score = [74, 54, 41, 22, 15][index];
        state.store.analyses.insert(
            other.id.clone(),
            Analysis {
                score,
                stage: "Descoberta".into(),
                objections: if index % 2 == 0 {
                    vec![Objection { label: "Preço".into(), ..Objection::default() }]
                } else {
                    vec![Objection { label: "Já tenho agência".into(), ..Objection::default() }]
                },
                analysed_at: now - 3600,
                chat_name: app.chat_title(other),
                ..Analysis::default()
            },
        );
        followup::schedule(
            &mut state.store,
            followup::FollowUp {
                chat: other.id.clone(),
                chat_name: app.chat_title(other),
                due: now + 3600 * (6 + 20 * index as i64),
                draft: "Oi! Separei dois criativos de exemplo para lojas como a sua, posso te mandar?".into(),
                reason: "Reaquecer".into(),
                mode: if index == 1 { followup::Mode::AutoSend } else { followup::Mode::AskApproval },
                only_if_no_reply: index != 2,
                created: now,
                ..followup::FollowUp::default()
            },
        );
    }
    state.store.sent = (0..9)
        .map(|index| store::Sent { chat: chat.clone(), at: now - 3600 * index, follow_up: index % 3 != 0 })
        .collect();
    state.tab = match page {
        "claude-control" | "claude-approve" => Tab::Chat,
        "claude-followups" => Tab::FollowUps,
        "claude-report" => Tab::Report,
        "claude-config" | "claude-connect" => Tab::Config,
        _ => Tab::Analysis,
    };
    if page == "claude-connect" {
        state.last_external = Some(std::time::Instant::now());
    }
    if matches!(page, "claude-control" | "claude-approve") {
        use agent::{Item, StepState};
        state.agent.items = vec![
            Item::User(format!("Responde {name} chamando ela e o sócio pra uma call amanhã e agenda um follow-up às 10h se ela não responder.")),
            Item::Claude("Certo. Vou fazer assim, e você acompanha cada passo no WhatsApp:".into()),
            Item::Action { label: "Ler a conversa".into(), state: StepState::Done },
            Item::Action { label: format!("Abrir a conversa com {name}"), state: StepState::Done },
            Item::Action { label: "Digitar a mensagem no campo".into(), state: StepState::Running },
        ];
        let text = "Super entendo! Que tal uma call rápida de 15 min com você e seu sócio amanhã? Mostro o resultado de uma loja parecida com a sua e vocês decidem com calma.";
        let mut steps = Vec::new();
        if page == "claude-approve" {
            app.composer = text.into();
            steps.push(cursor::Step::new(cursor::Target::Send, cursor::Act::Approve(chat.clone()), "Claude · esperando sua aprovação"));
        } else {
            steps.push(cursor::Step::new(cursor::Target::Composer, cursor::Act::Type(chat.clone(), text.into()), "Claude · digitando"));
        }
        state.cursor.start(steps);
        state.cursor.step = 3;
        state.cursor.steps = 5;
    }
    app.claude = state;
}

#[cfg(test)]
mod tests {
    use super::analysis::Temperature;

    #[test]
    fn score_bands_match_the_panel() {
        assert_eq!(Temperature::of(0), Temperature::Cold);
        assert_eq!(Temperature::of(33), Temperature::Cold);
        assert_eq!(Temperature::of(34), Temperature::Warm);
        assert_eq!(Temperature::of(66), Temperature::Warm);
        assert_eq!(Temperature::of(67), Temperature::Hot);
    }

    #[test]
    fn json_is_found_inside_a_fenced_reply() {
        let value = super::api::json_in("Claro!\n```json\n{\"score\": 70}\n```").unwrap();
        assert_eq!(value["score"], 70);
    }

    #[test]
    fn local_times_parse_for_follow_ups() {
        assert!(super::followup::parse_local("2030-01-02T10:00").is_some());
        assert!(super::followup::parse_local("amanhã").is_none());
    }

    #[test]
    fn an_analysis_reply_becomes_an_analysis() {
        let reply = serde_json::json!({
            "content": [{"type": "text", "text": "{\"score\": 120, \"reason\": \"r\", \"summary\": \"s\", \"objections\": [{\"label\": \"Preço\", \"quote\": \"caro\", \"counter\": \"c\"}], \"next_message\": \"oi\", \"follow_up\": null}"}]
        });
        let analysis = super::analysis::finish(&reply, "Ana".into(), "m1".into()).unwrap();
        assert_eq!(analysis.score, 100);
        assert_eq!(analysis.objections[0].label, "Preço");
        assert!(analysis.follow_up.is_none());
    }

    #[test]
    fn the_store_survives_a_round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let mut store = super::store::Store::default();
        store.profile_mut().name = "Lucas".into();
        store.save(folder.path()).unwrap();
        let loaded = super::store::Store::load(folder.path());
        assert_eq!(loaded.profile().name, "Lucas");
        assert_eq!(loaded.model, super::MODEL);
    }
}
