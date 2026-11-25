//! O cursor do Claude. Toda ação no WhatsApp (abrir conversa, ler, digitar,
//! enviar) vira uma fila de passos que um cursor coral executa na tela, à
//! vista do SDR. Qualquer clique ou tecla do SDR fora do painel, ou Esc,
//! devolve o controle na hora.

use std::collections::VecDeque;

use egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::store::CursorSpeed;
use crate::app::App;
use crate::model::{Action, ChatId};

/// Para onde o cursor vai.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A linha do chat na lista de conversas.
    Row(ChatId),
    /// A lista de conversas como um todo.
    List,
    /// O campo de mensagem.
    Composer,
    /// O botão de enviar.
    Send,
    /// A área das mensagens.
    Messages,
}

/// O que acontece quando o cursor chega.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Move,
    Click(Effect),
    /// Digita o texto no campo de mensagem deste chat.
    Type(ChatId, String),
    /// Rola a conversa, lendo.
    Scroll,
    /// Espera as mensagens do chat carregarem.
    Wait(ChatId),
    /// Espera o SDR aprovar o envio.
    Approve(ChatId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    None,
    OpenChat(ChatId),
    Send(ChatId),
}

#[derive(Clone, Debug)]
pub struct Step {
    pub target: Target,
    pub act: Act,
    /// O que o rótulo do cursor diz durante o passo.
    pub label: String,
}

impl Step {
    pub fn new(target: Target, act: Act, label: impl Into<String>) -> Self {
        Self {
            target,
            act,
            label: label.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done,
    Canceled(String),
}

/// Como o cursor aparece.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    #[default]
    Arrow,
    Click,
    Beam,
    Read,
}

enum Phase {
    Travel { from: Pos2, start: f64, duration: f32 },
    Act { start: f64 },
    Typing { chars: Vec<char>, done: usize, start: f64 },
}

/// Algo que aconteceu de fato no WhatsApp, para registro.
pub enum Event {
    Sent(ChatId),
}

#[derive(Default)]
pub struct Executor {
    pub pos: Option<Pos2>,
    queue: VecDeque<Step>,
    active: Option<(Step, Phase)>,
    busy: bool,
    pub paused: bool,
    /// O envio que espera o SDR aprovar.
    pub waiting_approval: Option<ChatId>,
    /// A resposta do SDR, posta pela interface.
    pub approval: Option<bool>,
    /// Como terminou o último trabalho, até alguém ler.
    pub outcome: Option<Outcome>,
    pub label: String,
    pub look: Look,
    /// Onde e quando foi o último clique, para o pulso.
    pub pulse: Option<(Pos2, f64)>,
    /// Posições recentes, para o rastro.
    pub trail: VecDeque<(Pos2, f64)>,
    pub step: usize,
    pub steps: usize,
    /// Quando o cursor ficou parado, para sumir depois de um tempo.
    pub idle_since: Option<f64>,
}

const PULSE: f64 = 0.35;
const SCROLL: f64 = 1.1;
const WAIT_LIMIT: f64 = 6.0;

pub fn row_id(chat: &str) -> egui::Id {
    egui::Id::new(("claude-row", chat))
}

pub fn list_id() -> egui::Id {
    egui::Id::new("claude-list")
}

pub fn conversation_id() -> egui::Id {
    egui::Id::new("claude-conversation")
}

/// A lista de conversas anota onde desenhou cada linha.
pub fn note_row(ctx: &egui::Context, chat: &str, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        data.insert_temp(row_id(chat), (rect, pass));
        let list = match data.get_temp::<(Rect, u64)>(list_id()) {
            Some((union, at)) if at == pass => union.union(rect),
            _ => rect,
        };
        data.insert_temp(list_id(), (list, pass));
    });
}

fn fresh(ctx: &egui::Context, id: egui::Id) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|data| data.get_temp::<(Rect, u64)>(id))
        .filter(|(_, at)| pass.saturating_sub(*at) <= 2)
        .map(|(rect, _)| rect)
}

fn conversation(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|data| data.get_temp::<Rect>(conversation_id()))
}

fn composer(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|data| data.get_temp::<Rect>(crate::ui::composer_rect_id()))
}

fn pill(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|data| data.get_temp::<Rect>(crate::ui::conversation::composer_pill_id()))
}

fn fallback(ctx: &egui::Context) -> Pos2 {
    conversation(ctx)
        .map(|rect| rect.center())
        .unwrap_or_else(|| ctx.content_rect().center())
}

/// Onde fica o alvo agora, e se ele está visível de verdade.
fn locate(ctx: &egui::Context, target: &Target) -> (Pos2, bool) {
    match target {
        Target::Row(chat) => match fresh(ctx, row_id(chat)) {
            // Um pouco à direita do avatar, sobre o nome.
            Some(rect) => (pos2(rect.left() + 150.0_f32.min(rect.width() / 2.0), rect.center().y), true),
            None => (
                fresh(ctx, list_id()).map(|rect| rect.center()).unwrap_or_else(|| fallback(ctx)),
                false,
            ),
        },
        Target::List => (
            fresh(ctx, list_id()).map(|rect| rect.center()).unwrap_or_else(|| fallback(ctx)),
            true,
        ),
        Target::Composer => match composer(ctx) {
            Some(rect) => (pos2(rect.left() + 18.0, rect.center().y), true),
            None => (fallback(ctx), false),
        },
        Target::Send => match pill(ctx).or_else(|| composer(ctx)) {
            Some(rect) => (rect.right_center() - vec2(22.0, 0.0), true),
            None => (fallback(ctx), false),
        },
        Target::Messages => match (conversation(ctx), composer(ctx)) {
            (Some(area), _) => (area.center() + vec2(area.width() * 0.18, 0.0), true),
            (None, Some(rect)) => (rect.center_top() - vec2(0.0, 220.0), true),
            (None, None) => (fallback(ctx), false),
        },
    }
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Executor {
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn start(&mut self, steps: Vec<Step>) {
        self.steps = steps.len();
        self.step = 0;
        self.queue = steps.into();
        self.active = None;
        self.busy = true;
        self.paused = false;
        self.outcome = None;
        self.approval = None;
        self.waiting_approval = None;
        self.idle_since = None;
    }

    pub fn cancel(&mut self, reason: &str) {
        if !self.busy {
            return;
        }
        self.queue.clear();
        self.active = None;
        self.busy = false;
        self.paused = false;
        self.waiting_approval = None;
        self.approval = None;
        self.outcome = Some(Outcome::Canceled(reason.into()));
        self.label = "Claude · parado".into();
        self.look = Look::Arrow;
    }

    fn finish_step(&mut self) {
        self.active = None;
    }

    /// Avança a animação e aplica os efeitos no app.
    pub fn tick(&mut self, app: &mut App, speed: CursorSpeed, ctx: &egui::Context) -> Option<Event> {
        let now = ctx.input(|input| input.time);
        while self.trail.front().is_some_and(|(_, at)| now - at > 0.6) {
            self.trail.pop_front();
        }
        if !self.busy {
            if self.idle_since.is_none() && self.pos.is_some() {
                self.idle_since = Some(now);
            }
            // O cursor some devagar depois de parar.
            if let Some(since) = self.idle_since {
                if now - since > 2.5 {
                    self.pos = None;
                    self.idle_since = None;
                } else {
                    ctx.request_repaint();
                }
            }
            return None;
        }
        ctx.request_repaint();
        if self.paused {
            return None;
        }
        if self.active.is_none() {
            let Some(step) = self.queue.pop_front() else {
                self.busy = false;
                self.outcome = Some(Outcome::Done);
                self.look = Look::Arrow;
                self.label = "Claude".into();
                return None;
            };
            self.step += 1;
            self.label = step.label.clone();
            let from = self.pos.unwrap_or_else(|| locate(ctx, &step.target).0 + vec2(-120.0, -80.0));
            let (to, _) = locate(ctx, &step.target);
            let duration = ((to - from).length() / speed.travel()).clamp(0.18, 1.1);
            self.look = Look::Arrow;
            self.active = Some((step, Phase::Travel { from, start: now, duration }));
        }
        let (step, phase) = self.active.as_mut()?;
        let (target, _) = locate(ctx, &step.target);
        match phase {
            Phase::Travel { from, start, duration } => {
                let t = ((now - *start) as f32 / *duration).min(1.0);
                let pos = *from + (target - *from) * ease(t);
                self.pos = Some(pos);
                self.trail.push_back((pos, now));
                if t >= 1.0 {
                    *phase = Phase::Act { start: now };
                    if let Act::Click(_) = step.act {
                        self.pulse = Some((target, now));
                    }
                }
                None
            }
            Phase::Act { start } => {
                let elapsed = now - *start;
                self.pos = Some(target);
                match step.act.clone() {
                    Act::Move => {
                        self.finish_step();
                        None
                    }
                    Act::Click(effect) => {
                        self.look = Look::Click;
                        if elapsed < PULSE {
                            return None;
                        }
                        self.look = Look::Arrow;
                        self.finish_step();
                        match effect {
                            Effect::None => None,
                            Effect::OpenChat(chat) => {
                                if app.open_chat.as_deref() != Some(chat.as_str()) {
                                    app.actions.push(Action::OpenChat(chat));
                                }
                                None
                            }
                            Effect::Send(chat) => {
                                let text = std::mem::take(&mut app.composer);
                                if text.trim().is_empty() {
                                    self.cancel("O campo de mensagem estava vazio");
                                    return None;
                                }
                                app.actions.push(Action::SendText {
                                    chat: chat.clone(),
                                    text,
                                    quoting: None,
                                });
                                Some(Event::Sent(chat))
                            }
                        }
                    }
                    Act::Type(chat, text) => {
                        if app.open_chat.as_deref() != Some(chat.as_str()) {
                            // Pede para abrir uma vez e espera a conversa abrir.
                            if elapsed == 0.0 {
                                app.actions.push(Action::OpenChat(chat));
                            } else if elapsed > 4.0 {
                                self.cancel("Não consegui abrir a conversa para digitar.");
                            }
                            return None;
                        }
                        app.composer.clear();
                        self.look = Look::Beam;
                        *phase = Phase::Typing {
                            chars: text.chars().collect(),
                            done: 0,
                            start: now,
                        };
                        None
                    }
                    Act::Scroll => {
                        self.look = Look::Read;
                        // O cursor desce devagar pela conversa, lendo.
                        let wobble = ((elapsed * 5.0).sin() * 6.0) as f32;
                        self.pos = Some(target + vec2(0.0, wobble + (elapsed as f32 * 40.0)));
                        if elapsed >= SCROLL {
                            app.scroll_to_bottom = true;
                            self.look = Look::Arrow;
                            self.finish_step();
                        }
                        None
                    }
                    Act::Wait(chat) => {
                        self.look = Look::Read;
                        let loaded = app
                            .conversations
                            .get(&chat)
                            .is_some_and(|conversation| !conversation.messages.is_empty() || conversation.complete);
                        if loaded || elapsed > WAIT_LIMIT {
                            self.look = Look::Arrow;
                            self.finish_step();
                        }
                        None
                    }
                    Act::Approve(chat) => {
                        self.look = Look::Arrow;
                        match self.approval.take() {
                            Some(true) => {
                                self.waiting_approval = None;
                                self.finish_step();
                            }
                            Some(false) => {
                                self.waiting_approval = None;
                                self.cancel("Você cancelou o envio. A mensagem ficou no campo para você editar.");
                            }
                            None => self.waiting_approval = Some(chat),
                        }
                        None
                    }
                }
            }
            Phase::Typing { chars, done, start } => {
                let wanted = (((now - *start) as f32) * speed.typing()) as usize;
                while *done < wanted.min(chars.len()) {
                    app.composer.push(chars[*done]);
                    *done += 1;
                }
                if let Some(rect) = composer(ctx) {
                    let line = (*done as f32 * 7.2) % (rect.width() - 30.0).max(40.0);
                    self.pos = Some(pos2(rect.left() + 10.0 + line, rect.center().y));
                } else {
                    self.pos = Some(target);
                }
                if *done >= chars.len() {
                    self.look = Look::Arrow;
                    self.finish_step();
                }
                None
            }
        }
    }

    /// O rótulo com o progresso ("3/5").
    pub fn progress_label(&self) -> String {
        self.label.clone()
    }

    /// Um deslocamento de onde o cursor deve aparecer, para o rastro.
    pub fn offset() -> Vec2 {
        Vec2::ZERO
    }
}
