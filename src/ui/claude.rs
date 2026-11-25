//! O painel do Claude ao lado da conversa, o botão na barra lateral, o selo
//! de temperatura nas linhas da lista e a camada do cursor por cima de tudo.

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, LayerId, Layout, Margin, Modifiers, Order,
    Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2, pos2, vec2,
};

use crate::app::App;
use crate::claude::agent::{self, Item, Origin, StepState};
use crate::claude::analysis::Temperature;
use crate::claude::cursor::{self, Look};
use crate::claude::followup::{self, FollowUp, Mode, Status};
use crate::claude::store::{CursorSpeed, Profile, Tone};
use crate::claude::{self as assistant, ClaudeState, Tab, connect};
use crate::model::{Action, Page};
use crate::theme::{self, Palette};

/// Coral do Claude, para tudo o que ele faz.
pub const CORAL: Color32 = Color32::from_rgb(0xD9, 0x77, 0x57);
const CORAL_SOFT: Color32 = Color32::from_rgb(0x3A, 0x26, 0x20);
const GREEN: Color32 = Color32::from_rgb(0x00, 0xA8, 0x84);
const PANEL_WIDTH: f32 = 400.0;
const PANEL_MIN: f32 = 330.0;

fn panel_rect_id() -> Id {
    Id::new("claude-panel-rect")
}

fn soft(palette: &Palette) -> Color32 {
    if palette.dark {
        CORAL_SOFT
    } else {
        Color32::from_rgb(0xFB, 0xEA, 0xE4)
    }
}

/// O painel, encaixado à direita da conversa.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if !app.claude.open || !matches!(app.page, Page::Chats | Page::Calls) {
        return;
    }
    super::pane::docked(app, ui, "claude-pane", PANEL_WIDTH, PANEL_MIN, 300.0, |app, ui| {
        let rect = ui.max_rect();
        let pass = ui.ctx().cumulative_pass_nr();
        ui.ctx().data_mut(|data| data.insert_temp(panel_rect_id(), (rect, pass)));
        let mut state = std::mem::take(&mut app.claude);
        panel(&mut state, app, ui);
        app.claude = state;
    });
}

fn panel(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
    Frame::new().inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 4 }).show(ui, |ui| {
        header(state, app, ui, &palette);
    });
    tabs(state, ui, &palette);
    let chat_tab = state.tab == Tab::Chat;
    if chat_tab {
        // O campo do chat fica preso embaixo.
        egui::Panel::bottom("claude-chat-input")
            .show_separator_line(false)
            .frame(Frame::new().fill(palette.panel).inner_margin(Margin::same(12)))
            .show(ui, |ui| chat_input(state, app, ui, &palette));
    }
    egui::ScrollArea::vertical()
        .id_salt(("claude-body", state.tab as u8))
        .auto_shrink([false, false])
        .stick_to_bottom(chat_tab)
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                match state.tab {
                    Tab::Analysis => analysis_tab(state, app, ui, &palette),
                    Tab::Chat => chat_tab_body(state, app, ui, &palette),
                    Tab::FollowUps => follow_ups_tab(state, app, ui, &palette),
                    Tab::Report => report_tab(state, app, ui, &palette),
                    Tab::Config => config_tab(state, ui, &palette),
                }
            });
        });
}

// ─── Pedaços reutilizados ────────────────────────────────────────────────────

/// A estrela do Claude, como dois losangos cruzados.
pub fn spark(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let thin = radius * 0.3;
    painter.add(Shape::convex_polygon(
        vec![
            center + vec2(0.0, -radius),
            center + vec2(thin, 0.0),
            center + vec2(0.0, radius),
            center + vec2(-thin, 0.0),
        ],
        color,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            center + vec2(-radius, 0.0),
            center + vec2(0.0, -thin),
            center + vec2(radius, 0.0),
            center + vec2(0.0, thin),
        ],
        color,
        Stroke::NONE,
    ));
}

fn text(ui: &mut egui::Ui, value: impl Into<String>, font: FontId, color: Color32) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(value.into()).font(font).color(color)).wrap())
}

fn caption(ui: &mut egui::Ui, palette: &Palette, value: &str) {
    ui.label(
        RichText::new(value.to_uppercase())
            .font(theme::semibold(10.5))
            .color(palette.secondary)
            .extra_letter_spacing(0.6),
    );
}

fn card<R>(ui: &mut egui::Ui, palette: &Palette, title: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 7.0;
            if let Some(title) = title {
                caption(ui, palette, title);
            }
            add(ui)
        })
        .inner
}

fn button(ui: &mut egui::Ui, palette: &Palette, label: &str, primary: bool) -> egui::Response {
    let (fill, color) = if primary {
        (CORAL, Color32::WHITE)
    } else {
        (palette.surface_hover, palette.text)
    };
    ui.add(
        egui::Button::new(RichText::new(label).font(theme::semibold(12.5)).color(color))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 30.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn pill(ui: &mut egui::Ui, label: &str, fill: Color32, color: Color32) {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), theme::semibold(11.0), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(16.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool(response.id, *on);
    let fill = if *on { GREEN } else { Color32::from_gray(0x55) };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill.gamma_multiply(0.4 + 0.6 * t.max(0.4)));
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    ui.painter().circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn setting(ui: &mut egui::Ui, palette: &Palette, title: &str, detail: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(ui.available_width() - 44.0, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            text(ui, title, theme::medium(13.0), palette.text);
            if !detail.is_empty() {
                text(ui, detail, theme::regular(11.5), palette.secondary);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = toggle(ui, on).changed();
        });
    });
    changed
}

fn field(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &mut String, rows: usize, hint: &str) -> bool {
    text(ui, label, theme::medium(12.0), palette.secondary);
    let edit = if rows <= 1 {
        egui::TextEdit::singleline(value)
    } else {
        egui::TextEdit::multiline(value).desired_rows(rows)
    };
    ui.add(edit.desired_width(f32::INFINITY).hint_text(hint).font(theme::regular(13.0)))
        .changed()
}

fn temperature_pill(ui: &mut egui::Ui, score: u8) {
    let temperature = Temperature::of(score);
    pill(ui, &format!("{} {score}", temperature.label()), temperature.color(), Color32::WHITE);
}

// ─── Cabeçalho e abas ────────────────────────────────────────────────────────

fn header(state: &mut ClaudeState, app: &App, ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal(|ui| {
        let (logo, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
        ui.painter().rect_filled(logo, CornerRadius::same(8), soft(palette));
        spark(ui.painter(), logo.center(), 9.0, CORAL);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("Claude SDR").font(theme::semibold(15.0)).color(palette.text));
            let profile = state.store.profile();
            let via = if state.store.key().is_some() { model_name(&state.store.model) } else { "Sua conta Claude".into() };
            ui.label(
                RichText::new(format!("{via} · perfil: {}", profile.name))
                    .font(theme::regular(11.0))
                    .color(palette.secondary),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(egui::Button::new(RichText::new("✕").color(palette.secondary)).frame(false))
                .on_hover_text("Fechar (Ctrl+Shift+K)")
                .clicked()
            {
                state.open = false;
            }
            let (label, color) = if state.cursor.busy() {
                ("Agindo", CORAL)
            } else if state.agent.running() {
                ("Pensando", CORAL)
            } else if app.open_chat.as_ref().is_some_and(|chat| state.analysis_jobs.contains_key(chat)) {
                ("Analisando", Temperature::Warm.color())
            } else if state.desktop_connected() {
                ("Claude Desktop", GREEN)
            } else if state.store.key().is_none() {
                ("Aguardando Claude", palette.secondary)
            } else {
                ("Pronto", GREEN)
            };
            Frame::new()
                .fill(palette.surface)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.0, color);
                        ui.label(RichText::new(label).font(theme::medium(11.0)).color(palette.secondary));
                    });
                });
        });
    });
}

fn model_name(model: &str) -> String {
    match model {
        "claude-opus-5-5" => "Opus 5.5".into(),
        "claude-sonnet-5-5" => "Sonnet 5.5".into(),
        "claude-haiku-5-5" => "Haiku 5.5".into(),
        other => other.into(),
    }
}

fn tabs(state: &mut ClaudeState, ui: &mut egui::Ui, palette: &Palette) {
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        for tab in Tab::ALL {
            let active = state.tab == tab;
            let galley = ui.painter().layout_no_wrap(
                tab.label().into(),
                if active { theme::semibold(13.0) } else { theme::medium(13.0) },
                if active { palette.text } else { palette.secondary },
            );
            let size = vec2(galley.size().x + 18.0, 34.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            if response.clicked() {
                state.tab = tab;
            }
            if response.hovered() && !active {
                ui.painter().rect_filled(rect.shrink(2.0), CornerRadius::same(6), palette.surface_hover);
            }
            ui.painter().galley(rect.center() - galley.size() / 2.0 - vec2(0.0, 2.0), galley, palette.text);
            if active {
                let bar = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 1.0), vec2(28.0, 2.0));
                ui.painter().rect_filled(bar, CornerRadius::same(1), CORAL);
            }
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    });
    let line = ui.cursor().top();
    let width = ui.max_rect().x_range();
    let _ = top;
    ui.painter().hline(width, line, Stroke::new(1.0, palette.outline));
}

// ─── Análise ─────────────────────────────────────────────────────────────────

fn analysis_tab(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    let Some(chat) = app.open_chat.clone() else {
        empty(ui, palette, "Abra uma conversa para o Claude analisar o lead.");
        return;
    };
    let name = app.chat_title_of(&chat);
    let running = state.analysis_jobs.contains_key(&chat);
    let current = crate::claude::analysis::last_message_id(app, &chat);
    ui.horizontal(|ui| {
        text(ui, &name, theme::semibold(14.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if running {
                ui.add(egui::Spinner::new().size(14.0).color(CORAL));
            } else {
                let label = if state.store.analyses.contains_key(&chat) { "Atualizar" } else { "Analisar" };
                if state.store.key().is_some() {
                    if button(ui, palette, label, false).clicked() {
                        state.analysis_errors.remove(&chat);
                        assistant::analyse(state, app, &chat, ui.ctx());
                    }
                } else if button(ui, palette, "Copiar pedido", false)
                    .on_hover_text("Copia o pedido de análise para colar no Claude Desktop ou Claude Code")
                    .clicked()
                {
                    ui.ctx().copy_text(ANALYSE_PROMPT.into());
                }
            }
        });
    });
    if let Some(error) = state.analysis_errors.get(&chat) {
        card(ui, palette, None, |ui| {
            text(ui, error, theme::regular(12.5), palette.danger);
        });
    }
    let Some(analysis) = state.store.analyses.get(&chat).cloned() else {
        if running {
            empty(ui, palette, "Lendo a conversa e medindo a temperatura do lead…");
        } else if state.store.key().is_none() {
            empty(
                ui,
                palette,
                "Peça no Claude Desktop ou Claude Code: \"analise o lead aberto no VerkzChat\". A análise aparece aqui.",
            );
        } else {
            empty(ui, palette, "Clique em Analisar para o Claude ler esta conversa.");
        }
        return;
    };
    if current.as_deref().is_some_and(|last| last != analysis.last_message) && !running {
        text(ui, "Chegaram mensagens depois desta análise.", theme::regular(11.5), Temperature::Warm.color());
    }
    let temperature = analysis.temperature();
    card(ui, palette, Some("Temperatura do lead"), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(analysis.score.to_string()).font(theme::bold(34.0)).color(temperature.color()));
            ui.label(RichText::new("/100").font(theme::medium(13.0)).color(palette.secondary));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                pill(ui, temperature.label(), temperature.color(), Color32::WHITE);
            });
        });
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::same(3), palette.surface_hover);
        let filled = Rect::from_min_size(bar.min, vec2(bar.width() * f32::from(analysis.score) / 100.0, bar.height()));
        ui.painter().rect_filled(filled, CornerRadius::same(3), temperature.color());
        text(ui, &analysis.reason, theme::regular(13.0), palette.text);
    });
    card(ui, palette, Some("Resumo"), |ui| {
        if !analysis.stage.is_empty() {
            pill(ui, &analysis.stage, palette.surface_hover, palette.text);
        }
        text(ui, &analysis.summary, theme::regular(13.0), palette.text);
        if !analysis.buying_signals.is_empty() {
            text(ui, "Sinais de compra", theme::semibold(12.0), palette.secondary);
            for signal in &analysis.buying_signals {
                text(ui, format!("• {signal}"), theme::regular(12.5), palette.text);
            }
        }
    });
    let title = format!("Objeções detectadas · {}", analysis.objections.len());
    card(ui, palette, Some(&title), |ui| {
        if analysis.objections.is_empty() {
            text(ui, "Nenhuma objeção até agora.", theme::regular(12.5), palette.secondary);
        }
        for objection in &analysis.objections {
            let response = Frame::new()
                .fill(palette.panel)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin { left: 13, right: 10, top: 9, bottom: 9 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 4.0;
                    text(ui, &objection.label, theme::semibold(13.0), palette.text);
                    if !objection.quote.is_empty() {
                        text(ui, format!("“{}”", objection.quote), theme::regular(12.0), palette.secondary);
                    }
                    text(ui, format!("→ {}", objection.counter), theme::regular(12.5), palette.text);
                });
            let rect = response.response.rect;
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 },
                CORAL,
            );
        }
    });
    if !analysis.next_message.trim().is_empty() {
        card(ui, palette, Some("Próxima mensagem sugerida"), |ui| {
            Frame::new()
                .fill(palette.bubble_out)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    text(ui, &analysis.next_message, theme::regular(13.0), palette.text);
                });
            let ctx = ui.ctx().clone();
            ui.columns(2, |columns| {
                if button(&mut columns[0], palette, "Colar no campo", false).clicked() {
                    app.composer = analysis.next_message.clone();
                    app.focus_composer = true;
                }
                let busy = state.agent.running();
                if columns[1]
                    .add_enabled_ui(!busy, |ui| button(ui, palette, "Claude envia", true))
                    .inner
                    .on_hover_text("O Claude digita com o cursor e envia (pede sua aprovação, se o perfil exigir)")
                    .clicked()
                {
                    let request = format!(
                        "Digite e envie esta mensagem para {name} agora, sem mudar o texto: \"{}\"",
                        analysis.next_message
                    );
                    state.tab = Tab::Chat;
                    agent::start(state, app, &ctx, request, Origin::Sdr, false);
                }
            });
        });
    }
    let open = state
        .store
        .follow_ups
        .iter()
        .find(|follow_up| follow_up.chat == chat && follow_up.status == Status::Scheduled)
        .cloned();
    card(ui, palette, Some("Follow-up"), |ui| match (&open, &analysis.follow_up) {
        (Some(follow_up), _) => {
            ui.horizontal(|ui| {
                text(ui, followup::when(follow_up.due), theme::semibold(13.0), palette.text);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    pill(ui, follow_up.mode.label(), palette.surface_hover, palette.secondary);
                });
            });
            text(ui, &follow_up.draft, theme::regular(12.5), palette.secondary);
        }
        (None, Some(idea)) => {
            ui.horizontal(|ui| {
                text(ui, &idea.when, theme::semibold(13.0), palette.text);
            });
            if !idea.reason.is_empty() {
                text(ui, &idea.reason, theme::regular(12.0), palette.secondary);
            }
            text(ui, &idea.draft, theme::regular(12.5), palette.text);
            if button(ui, palette, "Agendar este follow-up", true).clicked() {
                let hours = if idea.in_hours > 0.0 { idea.in_hours } else { 24.0 };
                followup::schedule(
                    &mut state.store,
                    FollowUp {
                        chat: chat.clone(),
                        chat_name: name.clone(),
                        due: crate::util::now() + (hours * 3600.0) as i64,
                        draft: idea.draft.clone(),
                        reason: idea.reason.clone(),
                        mode: Mode::AskApproval,
                        only_if_no_reply: true,
                        ..FollowUp::default()
                    },
                );
                state.dirty = true;
            }
        }
        (None, None) => {
            text(ui, "Sem follow-up sugerido: a bola está com você.", theme::regular(12.5), palette.secondary);
        }
    });
    text(
        ui,
        format!("Analisado em {}", crate::claude::analysis::clock(analysis.analysed_at)),
        theme::regular(11.0),
        palette.dim,
    );
}

fn empty(ui: &mut egui::Ui, palette: &Palette, message: &str) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 22.0, soft(palette));
        spark(ui.painter(), rect.center(), 11.0, CORAL);
        ui.add_space(6.0);
        text(ui, message, theme::regular(13.0), palette.secondary);
    });
}

// ─── Chat ────────────────────────────────────────────────────────────────────

const SUGGESTIONS: [&str; 4] = [
    "Responda o lead aberto com a próxima mensagem sugerida",
    "Quais leads estão quentes hoje? Liste e diga o próximo passo de cada um",
    "Leia esta conversa e me diga como quebrar a objeção dele",
    "Agende um follow-up com este lead para amanhã às 10h",
];

fn chat_tab_body(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    if state.agent.items.is_empty() {
        let local = state.store.key().is_some();
        empty(
            ui,
            palette,
            if local {
                "Peça algo ao Claude. Quando ele agir no WhatsApp, você vê o cursor dele na tela."
            } else {
                "Peça pelo Claude Desktop ou Claude Code. Clique numa sugestão para copiar e colar lá."
            },
        );
        ui.add_space(10.0);
        for suggestion in SUGGESTIONS {
            let response = Frame::new()
                .fill(palette.surface)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    text(ui, suggestion, theme::regular(12.5), palette.text);
                })
                .response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                if local {
                    agent::start(state, app, ui.ctx(), suggestion.into(), Origin::Sdr, false);
                } else {
                    ui.ctx().copy_text(format!("No VerkzChat: {suggestion}"));
                }
            }
        }
        return;
    }
    let items = state.agent.items.clone();
    for item in &items {
        match item {
            Item::User(message) => {
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    Frame::new()
                        .fill(palette.surface_hover)
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(11, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width() * 0.85);
                            text(ui, message, theme::regular(13.0), palette.text);
                        });
                });
            }
            Item::Claude(message) => {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    spark(ui.painter(), rect.center(), 6.0, CORAL);
                    ui.label(RichText::new("Claude").font(theme::semibold(12.0)).color(CORAL));
                });
                markdown(ui, palette, message);
            }
            Item::Action { label, state: step } => {
                Frame::new()
                    .fill(palette.surface)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            match step {
                                StepState::Running => {
                                    ui.add(egui::Spinner::new().size(14.0).color(CORAL));
                                }
                                StepState::Done => {
                                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    ui.painter().circle_filled(rect.center(), 8.0, GREEN);
                                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "✓", theme::bold(10.0), palette.panel);
                                }
                                StepState::Failed => {
                                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    ui.painter().circle_stroke(rect.center(), 7.0, Stroke::new(1.5, palette.danger));
                                }
                            }
                            let font = if *step == StepState::Running { theme::semibold(12.5) } else { theme::regular(12.5) };
                            text(ui, label, font, palette.text);
                        });
                    });
            }
            Item::Notice(message) => {
                Frame::new()
                    .fill(soft(palette))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        text(ui, message, theme::regular(12.5), CORAL);
                    });
            }
            Item::Error(message) => {
                text(ui, message, theme::regular(12.5), palette.danger);
            }
        }
    }
    if state.agent.running() && !state.cursor.busy() {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(13.0).color(CORAL));
            text(ui, "Claude está pensando…", theme::regular(12.0), palette.secondary);
        });
    } else if !state.agent.running()
        && ui
            .add(egui::Button::new(RichText::new("Limpar conversa").font(theme::regular(11.0)).color(palette.secondary)).frame(false))
            .clicked()
    {
        state.agent.clear();
    }
}

/// Markdown simples: títulos, listas e negrito em linha inteira.
fn markdown(ui: &mut egui::Ui, palette: &Palette, source: &str) {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(2.0);
        } else if let Some(title) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")) {
            text(ui, title, theme::semibold(13.5), palette.text);
        } else if let Some(title) = trimmed.strip_prefix("# ") {
            text(ui, title, theme::bold(15.0), palette.text);
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            text(ui, format!("• {}", item.replace("**", "")), theme::regular(13.0), palette.text);
        } else {
            let bold = trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 4;
            let clean = trimmed.replace("**", "");
            text(ui, clean, if bold { theme::semibold(13.0) } else { theme::regular(13.0) }, palette.text);
        }
    }
}

fn chat_input(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    if state.store.key().is_none() {
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                text(
                    ui,
                    "Converse com o Claude pelo Claude Desktop ou Claude Code. O que ele fizer aqui aparece nesta lista, com o cursor na tela.",
                    theme::regular(12.0),
                    palette.secondary,
                );
                if !state.desktop_connected() && ui.small_button("Como conectar").clicked() {
                    state.tab = Tab::Config;
                }
            });
        return;
    }
    let running = state.agent.running();
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin { left: 10, right: 6, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let id = Id::new("claude-chat-field");
                let field = ui.add_sized(
                    vec2(ui.available_width() - 40.0, 32.0),
                    egui::TextEdit::multiline(&mut state.chat_input)
                        .id(id)
                        .desired_rows(1)
                        .frame(Frame::NONE)
                        .font(theme::regular(13.0))
                        .hint_text("Peça algo ao Claude sobre esta conversa…"),
                );
                let enter = field.has_focus()
                    && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
                if running {
                    let stop = ui
                        .add(egui::Button::new(RichText::new("■").color(Color32::WHITE)).fill(palette.danger).min_size(Vec2::splat(30.0)))
                        .on_hover_text("Parar o Claude");
                    if stop.clicked() {
                        agent::interrupt(state, "Você parou o Claude.");
                    }
                } else {
                    let send = ui
                        .add(egui::Button::new(RichText::new("➤").color(Color32::WHITE)).fill(CORAL).min_size(Vec2::splat(30.0)))
                        .on_hover_text("Enviar ao Claude (Enter)");
                    if (send.clicked() || enter) && !state.chat_input.trim().is_empty() {
                        let request = std::mem::take(&mut state.chat_input).trim().to_owned();
                        agent::start(state, app, ui.ctx(), request, Origin::Sdr, false);
                        ui.memory_mut(|memory| memory.request_focus(id));
                    }
                }
            });
        });
}

// ─── Follow-ups ──────────────────────────────────────────────────────────────

fn follow_ups_tab(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            text(ui, "Tarefas agendadas", theme::semibold(15.0), palette.text);
            text(ui, "Na hora marcada o Claude executa com o cursor visível", theme::regular(11.5), palette.secondary);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if app.open_chat.is_some() && !state.new_follow_up.open && button(ui, palette, "+ Novo", true).clicked() {
                state.new_follow_up = followup::Draft::new();
            }
        });
    });
    if state.new_follow_up.open {
        new_follow_up(state, app, ui, palette);
    }
    let mut list: Vec<FollowUp> = state.store.follow_ups.clone();
    list.sort_by_key(|follow_up| (!follow_up.status.open(), if follow_up.status.open() { follow_up.due } else { -follow_up.due }));
    let mut shown_closed = 0;
    if list.is_empty() {
        empty(ui, palette, "Nenhum follow-up ainda. Agende pela aba Análise, pelo chat ou em + Novo.");
    }
    for follow_up in list {
        if !follow_up.status.open() {
            shown_closed += 1;
            if shown_closed > 12 {
                continue;
            }
        }
        follow_up_card(state, app, ui, palette, &follow_up);
    }
    ui.add_space(4.0);
    if state.store.key().is_none() {
        card(ui, palette, Some("Executar no horário"), |ui| {
            text(
                ui,
                "Quem executa os follow-ups é o Claude Desktop ou o Claude Code. Para rodar sozinho, crie lá uma tarefa agendada (por exemplo, de hora em hora, em horário comercial) com este pedido:",
                theme::regular(12.0),
                palette.text,
            );
            copy_block(ui, palette, FOLLOW_UP_PROMPT, "Copiar pedido");
            text(ui, "O VerkzChat e o Claude precisam estar abertos no computador.", theme::regular(11.0), palette.secondary);
        });
    }
    card(ui, palette, Some("Regras automáticas"), |ui| {
        let rules = &mut state.store.rules;
        let mut changed = false;
        changed |= setting(ui, palette, "Quente sem resposta em 24 h", "Follow-up com case ou prova", &mut rules.hot_after_day);
        changed |= setting(ui, palette, "Morno sem resposta em 3 dias", "Conteúdo útil para reaquecer", &mut rules.warm_after_three_days);
        changed |= setting(ui, palette, "Frio há 7 dias", "Reengajamento leve", &mut rules.cold_after_week);
        text(ui, "As regras criam follow-ups que pedem sua aprovação antes de enviar.", theme::regular(11.0), palette.secondary);
        if changed {
            state.dirty = true;
        }
    });
}

fn new_follow_up(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    let Some(chat) = app.open_chat.clone() else {
        state.new_follow_up.open = false;
        return;
    };
    let name = app.chat_title_of(&chat);
    let can_auto = state.store.profile().permissions.send_without_approval;
    let draft = &mut state.new_follow_up;
    let mut schedule = false;
    let title = format!("Novo follow-up · {name}");
    card(ui, palette, Some(&title), |ui| {
        ui.horizontal(|ui| {
            text(ui, "Daqui a", theme::regular(12.5), palette.text);
            ui.add(egui::DragValue::new(&mut draft.hours).range(0.25..=720.0).speed(0.5).suffix(" h"));
            for (label, hours) in [("1 h", 1.0), ("Amanhã", 24.0), ("3 dias", 72.0)] {
                if ui.small_button(label).clicked() {
                    draft.hours = hours;
                }
            }
        });
        text(ui, format!("Dispara {}", followup::when(crate::util::now() + (draft.hours * 3600.0) as i64)), theme::regular(11.5), palette.secondary);
        field(ui, palette, "Rascunho (o Claude ajusta ao contexto na hora)", &mut draft.text, 3, "Ex.: Oi! Conseguiu ver com seu sócio?");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut draft.mode, Mode::AskApproval, Mode::AskApproval.label());
            ui.add_enabled_ui(can_auto, |ui| {
                ui.selectable_value(&mut draft.mode, Mode::AutoSend, Mode::AutoSend.label())
                    .on_disabled_hover_text("Ative \"Enviar sem minha aprovação\" no perfil");
            });
        });
        ui.checkbox(&mut draft.only_if_no_reply, "Só se o lead não responder antes");
        ui.horizontal(|ui| {
            if button(ui, palette, "Agendar", true).clicked() {
                schedule = true;
            }
            if button(ui, palette, "Cancelar", false).clicked() {
                draft.open = false;
            }
        });
    });
    if schedule {
        let draft = std::mem::take(&mut state.new_follow_up);
        followup::schedule(
            &mut state.store,
            FollowUp {
                chat,
                chat_name: name,
                due: crate::util::now() + (draft.hours * 3600.0) as i64,
                draft: draft.text,
                reason: "Agendado pelo SDR".into(),
                mode: draft.mode,
                only_if_no_reply: draft.only_if_no_reply,
                ..FollowUp::default()
            },
        );
        state.dirty = true;
    }
}

fn follow_up_card(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette, follow_up: &FollowUp) {
    let running = follow_up.status == Status::Running;
    let frame = Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(11))
        .stroke(if running { Stroke::new(1.0, CORAL) } else { Stroke::NONE });
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            let name = ui.add(
                egui::Label::new(RichText::new(&follow_up.chat_name).font(theme::semibold(13.0)).color(palette.text))
                    .sense(Sense::click()),
            );
            if name.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.actions.push(Action::OpenChat(follow_up.chat.clone()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(analysis) = state.store.analyses.get(&follow_up.chat) {
                    temperature_pill(ui, analysis.score);
                }
                if follow_up.automatic {
                    pill(ui, "Regra", palette.surface_hover, palette.secondary);
                }
            });
        });
        ui.horizontal(|ui| {
            if running {
                ui.add(egui::Spinner::new().size(12.0).color(CORAL));
            }
            text(ui, followup::when(follow_up.due), theme::medium(12.0), palette.secondary);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (fill, color) = match follow_up.status {
                    Status::Running => (CORAL, Color32::WHITE),
                    Status::Done => (GREEN, Color32::WHITE),
                    Status::Failed => (palette.danger, Color32::WHITE),
                    Status::Scheduled if follow_up.only_if_no_reply => (Temperature::Warm.color(), Color32::WHITE),
                    _ => (palette.surface_hover, palette.secondary),
                };
                let label = if follow_up.status == Status::Scheduled && follow_up.only_if_no_reply {
                    "Se não responder"
                } else {
                    follow_up.status.label()
                };
                pill(ui, label, fill, color);
            });
        });
        if !follow_up.draft.is_empty() {
            Frame::new()
                .fill(palette.panel)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(9, 7))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    text(ui, &follow_up.draft, theme::regular(12.0), palette.text);
                });
        }
        if !follow_up.note.is_empty() && !follow_up.status.open() {
            text(ui, &follow_up.note, theme::regular(11.0), palette.secondary);
        }
        ui.horizontal(|ui| {
            let color = if follow_up.mode == Mode::AutoSend { CORAL } else { palette.secondary };
            ui.label(RichText::new(format!("Modo: {}", follow_up.mode.label())).font(theme::medium(11.0)).color(color));
            if follow_up.status == Status::Scheduled {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Cancelar").clicked() {
                        followup::set_status(&mut state.store, follow_up.id, Status::Canceled, "Cancelado pelo SDR");
                        state.dirty = true;
                    }
                    if ui.small_button("Executar agora").clicked()
                        && let Some(item) = state.store.follow_ups.iter_mut().find(|item| item.id == follow_up.id)
                    {
                        item.due = crate::util::now();
                        item.only_if_no_reply = false;
                        state.dirty = true;
                    }
                });
            }
        });
    });
}

// ─── Relatório ───────────────────────────────────────────────────────────────

fn report_tab(state: &mut ClaudeState, app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal(|ui| {
        text(ui, "Relatório do SDR", theme::semibold(15.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            egui::ComboBox::from_id_salt("claude-report-days")
                .selected_text(format!("Últimos {} dias", state.report_days))
                .width(130.0)
                .show_ui(ui, |ui| {
                    for days in [1, 7, 15, 30, 90] {
                        ui.selectable_value(&mut state.report_days, days, format!("Últimos {days} dias"));
                    }
                });
        });
    });
    let stats = crate::claude::report::stats(&state.store, app, state.report_days);
    let kpis = [
        ("Conversas ativas", stats.active.to_string(), palette.text),
        ("Leads quentes", stats.hot.to_string(), Temperature::Hot.color()),
        ("Follow-ups enviados", stats.follow_ups_sent.to_string(), CORAL),
        (
            "Taxa de resposta",
            stats.response_rate().map(|rate| format!("{:.0}%", rate * 100.0)).unwrap_or_else(|| "—".into()),
            GREEN,
        ),
    ];
    for row in kpis.chunks(2) {
        ui.columns(2, |columns| {
            for (column, (label, value, color)) in columns.iter_mut().zip(row) {
                card(column, palette, None, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(value).font(theme::bold(22.0)).color(*color));
                    ui.label(RichText::new(*label).font(theme::medium(11.0)).color(palette.secondary));
                });
            }
        });
    }
    card(ui, palette, Some("Temperatura da carteira"), |ui| {
        let total = (stats.hot + stats.warm + stats.cold).max(1) as f32;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
        let mut x = bar.left();
        for (count, temperature) in [(stats.hot, Temperature::Hot), (stats.warm, Temperature::Warm), (stats.cold, Temperature::Cold)] {
            let width = bar.width() * count as f32 / total;
            if width > 0.0 {
                ui.painter().rect_filled(Rect::from_min_size(pos2(x, bar.top()), vec2((width - 2.0).max(1.0), 8.0)), CornerRadius::same(2), temperature.color());
            }
            x += width;
        }
        if stats.analysed == 0 {
            ui.painter().rect_filled(bar, CornerRadius::same(2), palette.surface_hover);
        }
        ui.horizontal(|ui| {
            for (count, temperature) in [(stats.hot, Temperature::Hot), (stats.warm, Temperature::Warm), (stats.cold, Temperature::Cold)] {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, temperature.color());
                ui.label(RichText::new(format!("{}s {count}", temperature.label())).font(theme::regular(12.0)).color(palette.text));
            }
        });
        text(ui, format!("{} leads analisados no período", stats.analysed), theme::regular(11.0), palette.secondary);
    });
    card(ui, palette, Some("Objeções mais frequentes"), |ui| {
        if stats.objections.is_empty() {
            text(ui, "Nenhuma objeção registrada no período.", theme::regular(12.0), palette.secondary);
        }
        let top = stats.objections.first().map(|(_, count)| *count).unwrap_or(1).max(1) as f32;
        for (label, count) in &stats.objections {
            ui.horizontal(|ui| {
                text(ui, label, theme::regular(12.5), palette.text);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(count.to_string()).font(theme::semibold(12.0)).color(palette.secondary));
                });
            });
            let (track, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
            ui.painter().rect_filled(track, CornerRadius::same(3), palette.surface_hover);
            ui.painter().rect_filled(Rect::from_min_size(track.min, vec2(track.width() * *count as f32 / top, 6.0)), CornerRadius::same(3), CORAL);
        }
    });
    card(ui, palette, Some("Priorizar hoje"), |ui| {
        if stats.priorities.is_empty() {
            text(ui, "Sem leads quentes ou mornos no período.", theme::regular(12.0), palette.secondary);
        }
        for priority in &stats.priorities {
            let response = ui
                .horizontal(|ui| {
                    ui.allocate_ui_with_layout(vec2(ui.available_width() - 70.0, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        text(ui, &priority.name, theme::medium(13.0), palette.text);
                        text(ui, &priority.why, theme::regular(11.0), palette.secondary);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| temperature_pill(ui, priority.score));
                })
                .response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Abrir conversa");
            if response.clicked() {
                app.actions.push(Action::OpenChat(priority.chat.clone()));
            }
        }
    });
    let generating = state.report_job.is_some();
    ui.horizontal(|ui| {
        let label = if generating { "Gerando relatório…" } else { "Gerar relatório completo" };
        if ui.add_enabled_ui(!generating, |ui| button(ui, palette, label, true)).inner.clicked() {
            match crate::claude::report::start(&state.store, app, state.report_days, ui.ctx()) {
                Ok(job) => {
                    state.report_job = Some(job);
                    state.report_error = None;
                }
                Err(error) => state.report_error = Some(error),
            }
        }
        if generating {
            ui.add(egui::Spinner::new().size(14.0).color(CORAL));
        }
    });
    if let Some(error) = &state.report_error {
        text(ui, error, theme::regular(12.0), palette.danger);
    }
    if let Some(report) = state.store.report.clone() {
        card(ui, palette, Some("Último relatório completo"), |ui| {
            text(ui, format!("Gerado em {}", crate::claude::analysis::clock(report.created)), theme::regular(11.0), palette.secondary);
            if let Some(path) = &report.path {
                ui.horizontal(|ui| {
                    if button(ui, palette, "Abrir arquivo", false).clicked() {
                        app.actions.push(Action::OpenFile(path.clone()));
                    }
                    if button(ui, palette, "Abrir pasta", false).clicked()
                        && let Some(folder) = path.parent()
                    {
                        app.actions.push(Action::OpenFolder(folder.to_path_buf()));
                    }
                });
            }
            if let Some(status) = &state.saved_status {
                text(ui, status, theme::regular(11.0), palette.secondary);
            }
            ui.separator();
            markdown(ui, palette, &report.markdown);
        });
    }
}

// ─── Conexão com o Claude Desktop / Claude Code ───────────────────────────────

const ANALYSE_PROMPT: &str = "Analise o lead da conversa aberta no VerkzChat e salve a análise no painel.";
const FOLLOW_UP_PROMPT: &str = "Execute os follow-ups vencidos do VerkzChat: veja os vencidos, leia cada conversa, ajuste o rascunho ao contexto, digite, envie e feche cada follow-up.";

fn desktop_config() -> String {
    let config = serde_json::json!({ "mcpServers": { "verkzchat": { "command": connect::executable().display().to_string(), "args": ["mcp"] } } });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// Um texto pronto para copiar, com o botão ao lado.
fn copy_block(ui: &mut egui::Ui, palette: &Palette, value: &str, label: &str) {
    Frame::new()
        .fill(palette.panel)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(9, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(value).font(FontId::monospace(11.5)).color(palette.text));
        });
    if button(ui, palette, label, false).clicked() {
        ui.ctx().copy_text(value.to_owned());
    }
}

fn step(ui: &mut egui::Ui, palette: &Palette, number: &str, title: &str, detail: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 10.0, soft(palette));
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, number, theme::bold(11.0), CORAL);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            text(ui, title, theme::semibold(13.0), palette.text);
            if !detail.is_empty() {
                text(ui, detail, theme::regular(11.5), palette.secondary);
            }
        });
    });
}

/// Roda o login do Claude Code fora da tela, que não pode travar esperando o navegador.
fn start_sign_in(switch: bool) -> std::sync::mpsc::Receiver<Result<String, String>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(connect::sign_in(switch));
    });
    receiver
}

fn connection_card(state: &mut ClaudeState, ui: &mut egui::Ui, palette: &Palette) {
    let ready = connect::desktop_ready();
    card(ui, palette, Some("Conectar ao Claude"), |ui| {
        text(
            ui,
            "O Claude roda no Claude Desktop (ou no Claude Code) com a conta Claude de quem usa este computador. O VerkzChat só se registra como conector lá; o login é sempre no app oficial da Anthropic.",
            theme::regular(12.5),
            palette.text,
        );
        ui.horizontal(|ui| {
            let (color, label) = if state.desktop_connected() {
                (GREEN, "Conectado: o Claude está usando o VerkzChat.")
            } else if state.bridge_error.is_some() {
                (palette.danger, "O canal para o Claude não abriu.")
            } else if ready {
                (Temperature::Warm.color(), "Registrado no Claude Desktop. Falta entrar na conta e pedir algo.")
            } else {
                (Temperature::Warm.color(), "Ainda não conectado.")
            };
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, color);
            text(ui, label, theme::medium(12.0), palette.text);
        });
        if let Some(error) = &state.bridge_error {
            text(ui, error, theme::regular(11.5), palette.danger);
        }
        ui.add_space(4.0);
        step(ui, palette, "1", "Conectar ao Claude Desktop", "Registra o VerkzChat no Claude Desktop e abre o app.");
        let label = if ready { "Conectar de novo e abrir o Claude Desktop" } else { "Conectar ao Claude Desktop" };
        if button(ui, palette, label, true).clicked() {
            state.connect_status = Some(match connect::connect_desktop() {
                Ok(_) => match connect::open_desktop() {
                    Ok(()) => (
                        true,
                        "Pronto. Se o Claude Desktop já estava aberto, feche ele de vez (inclusive na bandeja, perto do relógio) e abra de novo para carregar o VerkzChat.".into(),
                    ),
                    Err(error) => (false, format!("Registrado, mas: {error}")),
                },
                Err(error) => (false, error),
            });
        }
        step(
            ui,
            palette,
            "2",
            "Entrar com a sua conta Claude",
            "Abre o login do Claude no navegador e volta sozinho para o Claude Code. Cada SDR usa a conta dele.",
        );
        let signing_in = state.login_job.is_some();
        ui.horizontal(|ui| {
            let label = if signing_in { "Aguardando o navegador…" } else { "Entrar com a conta Claude" };
            if button(ui, palette, label, true).clicked() && !signing_in {
                state.login_job = Some(start_sign_in(false));
            }
            if button(ui, palette, "Trocar de conta", false).clicked() && !signing_in {
                state.login_job = Some(start_sign_in(true));
            }
        });
        if let Some(job) = &state.login_job {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
            if let Ok(result) = job.try_recv() {
                state.connect_status = Some(match result {
                    Ok(message) => (true, message),
                    Err(error) => (false, error),
                });
                state.login_job = None;
            }
        }
        step(
            ui,
            palette,
            "3",
            "Pedir ao Claude",
            "Numa conversa nova do Claude Desktop, peça, por exemplo: \"analise o lead aberto no VerkzChat\". Na primeira vez ele pede permissão para usar as ferramentas do VerkzChat.",
        );
        if let Some((ok, message)) = &state.connect_status {
            text(ui, message, theme::regular(11.5), if *ok { GREEN } else { palette.danger });
        }
        ui.add_space(6.0);
        ui.separator();
        text(ui, "Prefere o Claude Code (terminal)?", theme::semibold(12.5), palette.text);
        if button(ui, palette, "Conectar ao Claude Code", false).clicked() {
            state.connect_status = Some(match connect::connect_code() {
                Ok(message) => (true, format!("{message} Se ele pedir login, rode claude no terminal e use /login com a sua conta.")),
                Err(error) => (false, error),
            });
        }
        egui::CollapsingHeader::new(RichText::new("Fazer manualmente").font(theme::medium(12.0)).color(palette.secondary))
            .id_salt("claude-manual-connect")
            .show(ui, |ui| {
                text(ui, "Claude Desktop: Configurações → Desenvolvedor → Editar configuração, cole e reinicie.", theme::regular(11.5), palette.secondary);
                copy_block(ui, palette, &desktop_config(), "Copiar configuração");
                text(ui, "Claude Code: rode no terminal.", theme::regular(11.5), palette.secondary);
                copy_block(ui, palette, &connect::code_command(), "Copiar comando");
            });
        text(ui, "O VerkzChat precisa estar aberto enquanto o Claude trabalha.", theme::regular(11.0), palette.secondary);
    });
}

// ─── Configuração ────────────────────────────────────────────────────────────

fn config_tab(state: &mut ClaudeState, ui: &mut egui::Ui, palette: &Palette) {
    let mut changed = false;
    connection_card(state, ui, palette);
    card(ui, palette, Some("Opcional: chave da API"), |ui| {
        text(
            ui,
            "Só se quiserem que o Claude também trabalhe sozinho dentro do VerkzChat (análise automática a cada mensagem e follow-ups no horário, mesmo sem o Claude Desktop aberto). É cobrada à parte, por uso, em console.anthropic.com.",
            theme::regular(11.5),
            palette.secondary,
        );
        text(ui, "Chave da API", theme::medium(12.0), palette.secondary);
        ui.horizontal(|ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.store.api_key)
                        .password(!state.show_key)
                        .desired_width(ui.available_width() - 60.0)
                        .hint_text("sk-ant-…"),
                )
                .changed();
            if ui.small_button(if state.show_key { "Ocultar" } else { "Mostrar" }).clicked() {
                state.show_key = !state.show_key;
            }
        });
        if state.store.api_key.trim().is_empty() && std::env::var("ANTHROPIC_API_KEY").is_ok() {
            text(ui, "Usando a chave da variável ANTHROPIC_API_KEY.", theme::regular(11.0), palette.secondary);
        }
        text(ui, "Modelo", theme::medium(12.0), palette.secondary);
        egui::ComboBox::from_id_salt("claude-model")
            .selected_text(model_name(&state.store.model))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"] {
                    changed |= ui.selectable_value(&mut state.store.model, model.to_owned(), model_name(model)).changed();
                }
            });
    });

    card(ui, palette, Some("Perfil do SDR"), |ui| {
        ui.horizontal(|ui| {
            let names: Vec<String> = state.store.profiles.iter().map(|profile| profile.name.clone()).collect();
            egui::ComboBox::from_id_salt("claude-profile")
                .selected_text(names[state.store.active_profile.min(names.len() - 1)].clone())
                .width(ui.available_width() - 120.0)
                .show_ui(ui, |ui| {
                    for (index, name) in names.iter().enumerate() {
                        changed |= ui.selectable_value(&mut state.store.active_profile, index, name).changed();
                    }
                });
            if ui.small_button("+ Novo").clicked() {
                state.store.profiles.push(Profile {
                    name: format!("SDR {}", state.store.profiles.len() + 1),
                    ..Profile::default()
                });
                state.store.active_profile = state.store.profiles.len() - 1;
                changed = true;
            }
            if state.store.profiles.len() > 1 && ui.small_button("Excluir").clicked() {
                state.store.profiles.remove(state.store.active_profile);
                state.store.active_profile = 0;
                changed = true;
            }
        });
        let profile = state.store.profile_mut();
        changed |= field(ui, palette, "Nome", &mut profile.name, 1, "Seu nome");
        text(ui, "Tom de voz", theme::medium(12.0), palette.secondary);
        ui.horizontal(|ui| {
            for tone in Tone::ALL {
                changed |= ui.selectable_value(&mut profile.tone, tone, tone.label()).changed();
            }
        });
    });

    card(ui, palette, Some("Contexto e instruções"), |ui| {
        let profile = state.store.profile_mut();
        changed |= field(ui, palette, "O que eu vendo", &mut profile.product, 3, "Ex.: gestão de tráfego pago para lojas de roupa física");
        changed |= field(ui, palette, "Como o Claude deve escrever", &mut profile.instructions, 3, "Ex.: mensagens curtas, uma pergunta por vez");
        changed |= field(ui, palette, "Critérios de temperatura", &mut profile.temperature_criteria, 3, "Sinais que mostram que o lead está quente");
        changed |= field(ui, palette, "Como eu contorno objeções", &mut profile.objection_playbook, 4, "Ex.: preço → mostrar case de loja parecida…");
    });

    card(ui, palette, Some("O que o Claude pode fazer no WhatsApp"), |ui| {
        let profile = state.store.profile_mut();
        let permissions = &mut profile.permissions;
        changed |= setting(ui, palette, "Ler conversas", "Para analisar, sugerir e montar relatórios", &mut permissions.read);
        changed |= setting(ui, palette, "Navegar entre chats", "Abre conversas com o cursor visível", &mut permissions.navigate);
        changed |= setting(ui, palette, "Digitar mensagens", "Escreve no campo; você vê em tempo real", &mut permissions.type_messages);
        changed |= setting(
            ui,
            palette,
            "Enviar sem minha aprovação",
            "Desligado: o Claude sempre espera você aprovar o envio",
            &mut permissions.send_without_approval,
        );
        changed |= setting(ui, palette, "Analisar sozinho", "Mede a temperatura quando chega mensagem nova", &mut profile.auto_analyse);
        ui.horizontal(|ui| {
            text(ui, "Velocidade do cursor", theme::medium(13.0), palette.text);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                egui::ComboBox::from_id_salt("claude-speed")
                    .selected_text(profile.cursor_speed.label())
                    .show_ui(ui, |ui| {
                        for speed in CursorSpeed::ALL {
                            changed |= ui.selectable_value(&mut profile.cursor_speed, speed, speed.label()).changed();
                        }
                    });
            });
        });
    });
    text(
        ui,
        "Durante uma ação, Esc ou qualquer clique seu fora do painel devolve o controle na hora.",
        theme::regular(11.5),
        palette.secondary,
    );
    if changed {
        state.dirty = true;
    }
}

// ─── Barra lateral e lista ───────────────────────────────────────────────────

/// O botão do Claude na barra lateral.
pub fn rail_button(app: &mut App, ui: &mut egui::Ui, size: f32) {
    let palette = app.palette;
    let (cell, _) = ui.allocate_exact_size(vec2(ui.available_width(), size), Sense::hover());
    let plate = Rect::from_center_size(cell.center(), Vec2::splat(size));
    let response = ui
        .interact(plate, Id::new("claude-rail-button"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(super::keys::label("Claude SDR (Ctrl+Shift+K)"));
    if response.clicked() {
        app.claude.open = !app.claude.open;
    }
    let lit = app.claude.open;
    if lit || response.hovered() {
        ui.painter().rect_filled(plate, CornerRadius::same(10), if lit { soft(&palette) } else { palette.surface_hover });
    }
    spark(ui.painter(), plate.center(), size * 0.3, CORAL);
    if app.claude.in_control() {
        ui.painter().circle_filled(plate.right_top() + vec2(-6.0, 6.0), 4.0, CORAL);
    }
}

/// O selo de temperatura sobre o avatar de uma linha da lista.
pub fn row_badge(app: &App, ui: &egui::Ui, avatar: Rect, chat: &str) {
    let Some(analysis) = app.claude.store.analyses.get(chat) else {
        return;
    };
    let temperature = analysis.temperature();
    let label = analysis.score.to_string();
    let galley = ui.painter().layout_no_wrap(label, theme::bold(9.5), Color32::WHITE);
    let size = vec2(galley.size().x + 8.0, 15.0);
    let rect = Rect::from_min_size(avatar.right_bottom() - vec2(size.x - 4.0, size.y - 2.0), size);
    ui.painter().rect(rect, CornerRadius::same(8), temperature.color(), Stroke::new(1.5, app.palette.panel), egui::StrokeKind::Outside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

// ─── A camada do cursor ──────────────────────────────────────────────────────

/// Ctrl+Shift+K abre e fecha o painel. Lido antes dos atalhos do app, que
/// tomariam o Ctrl+K da busca.
pub fn shortcut(app: &mut App, ctx: &egui::Context) {
    if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::K)) {
        app.claude.open = !app.claude.open;
    }
}

/// Atalho, faixa de controle, aprovação de envio e o cursor, por cima de tudo.
pub fn overlay(app: &mut App, ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    let panel = ctx
        .data(|data| data.get_temp::<(Rect, u64)>(panel_rect_id()))
        .filter(|(_, at)| pass.saturating_sub(*at) <= 1)
        .map(|(rect, _)| rect);
    let conversation = ctx.data(|data| data.get_temp::<Rect>(cursor::conversation_id()));
    let mut safe: Vec<Rect> = panel.into_iter().collect();
    let palette = app.palette;
    let mut state = std::mem::take(&mut app.claude);

    if state.cursor.busy() {
        let foreground = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("claude-control-outline")));
        if let Some(area) = conversation {
            foreground.rect_stroke(area.shrink(1.0), CornerRadius::ZERO, Stroke::new(2.0, CORAL), egui::StrokeKind::Inside);
            // A faixa "Claude no controle".
            let (step, total) = state.agent.plan();
            let banner = egui::Area::new(Id::new("claude-control-banner"))
                .order(Order::Foreground)
                .fixed_pos(area.left_top() + vec2(0.0, 64.0))
                .show(ctx, |ui| {
                    ui.set_width(area.width());
                    Frame::new()
                        .fill(CORAL)
                        .inner_margin(Margin::symmetric(14, 7))
                        .show(ui, |ui| {
                            ui.set_width(area.width() - 28.0);
                            ui.horizontal(|ui| {
                                spark(ui.painter(), ui.cursor().min + vec2(7.0, 9.0), 7.0, Color32::WHITE);
                                ui.add_space(18.0);
                                let what = state.cursor.label.trim_start_matches("Claude · ").to_owned();
                                ui.label(
                                    RichText::new(format!("Claude está no controle · {what} ({step} de {total})"))
                                        .font(theme::semibold(12.5))
                                        .color(Color32::WHITE),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let stop = ui.add(
                                        egui::Button::new(RichText::new("Parar  Esc").font(theme::semibold(12.0)).color(Color32::WHITE))
                                            .fill(Color32::from_white_alpha(45)),
                                    );
                                    if stop.clicked() {
                                        agent::interrupt(&mut state, "Você parou o Claude e retomou o controle.");
                                    }
                                    let label = if state.cursor.paused { "Continuar" } else { "Pausar" };
                                    if ui
                                        .add(egui::Button::new(RichText::new(label).font(theme::semibold(12.0)).color(Color32::WHITE)).fill(Color32::from_white_alpha(45)))
                                        .clicked()
                                    {
                                        state.cursor.paused = !state.cursor.paused;
                                    }
                                });
                            });
                        });
                });
            safe.push(banner.response.rect);
        }
        // A pergunta de aprovação do envio, sobre o campo.
        if state.cursor.waiting_approval.is_some()
            && let Some(composer) = ctx.data(|data| data.get_temp::<Rect>(super::composer_rect_id()))
        {
            let bar = egui::Area::new(Id::new("claude-approval"))
                .order(Order::Foreground)
                .pivot(Align2::RIGHT_BOTTOM)
                .fixed_pos(composer.right_top() - vec2(0.0, 12.0))
                .show(ctx, |ui| {
                    Frame::new()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, CORAL))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(12, 8))
                        .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("O Claude quer enviar esta mensagem").font(theme::medium(12.5)).color(palette.text));
                                if button(ui, &palette, "Aprovar envio", true).clicked() {
                                    state.cursor.approval = Some(true);
                                }
                                if button(ui, &palette, "Cancelar", false).clicked() {
                                    state.cursor.approval = Some(false);
                                }
                            });
                        });
                });
            safe.push(bar.response.rect);
        }
        // O SDR retoma o controle com Esc ou com um clique fora do painel.
        let escape = ctx.input(|input| input.key_pressed(Key::Escape));
        let clicked_outside = ctx.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !safe.iter().any(|rect| rect.contains(pos)))
        });
        if escape || clicked_outside {
            agent::interrupt(&mut state, "Você retomou o controle. O Claude parou.");
        }
    }

    paint_cursor(&state, ctx);
    app.claude = state;
}

fn paint_cursor(state: &ClaudeState, ctx: &egui::Context) {
    let executor = &state.cursor;
    let Some(pos) = executor.pos else { return };
    let now = ctx.input(|input| input.time);
    let fade = match executor.idle_since {
        Some(since) => (1.0 - ((now - since - 1.5) / 1.0).clamp(0.0, 1.0)) as f32,
        None => 1.0,
    };
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("claude-cursor")));
    // Rastro.
    for (index, (point, at)) in executor.trail.iter().enumerate() {
        if index % 3 != 0 {
            continue;
        }
        let age = ((now - at) / 0.6).clamp(0.0, 1.0) as f32;
        painter.circle_filled(*point, 2.0, CORAL.gamma_multiply((1.0 - age) * 0.5 * fade));
    }
    // Pulso do clique.
    if let Some((center, at)) = executor.pulse {
        let t = ((now - at) / 0.45) as f32;
        if (0.0..1.0).contains(&t) {
            painter.circle_stroke(center, 6.0 + 16.0 * t, Stroke::new(2.0, CORAL.gamma_multiply((1.0 - t) * fade)));
        }
    }
    let white = Color32::WHITE.gamma_multiply(fade);
    let coral = CORAL.gamma_multiply(fade);
    match executor.look {
        Look::Beam => {
            let stroke = Stroke::new(2.5, coral);
            painter.line_segment([pos + vec2(0.0, -11.0), pos + vec2(0.0, 11.0)], stroke);
            painter.line_segment([pos + vec2(-4.0, -11.0), pos + vec2(4.0, -11.0)], stroke);
            painter.line_segment([pos + vec2(-4.0, 11.0), pos + vec2(4.0, 11.0)], stroke);
        }
        Look::Arrow | Look::Click | Look::Read => {
            let points = vec![pos, pos + vec2(0.0, 19.0), pos + vec2(13.5, 13.0)];
            painter.add(Shape::convex_polygon(points.clone(), coral, Stroke::NONE));
            painter.add(Shape::closed_line(points, Stroke::new(1.6, white)));
            if executor.look == Look::Read {
                // Roda do mouse girando: o Claude está lendo.
                let wheel = pos + vec2(-12.0, 10.0);
                let angle = (now * 6.0) as f32;
                painter.circle_stroke(wheel, 5.0, Stroke::new(1.5, coral));
                painter.line_segment([wheel, wheel + vec2(angle.cos(), angle.sin()) * 4.0], Stroke::new(1.5, coral));
            }
        }
    }
    // Rótulo.
    let label = executor.progress_label();
    let galley = painter.layout_no_wrap(label, theme::semibold(11.0), white);
    let rect = Rect::from_min_size(pos + vec2(16.0, 22.0), galley.size() + vec2(22.0, 8.0));
    painter.rect_filled(rect.translate(vec2(0.0, 2.0)), CornerRadius::same(6), Color32::from_black_alpha((70.0 * fade) as u8));
    painter.rect_filled(rect, CornerRadius::same(6), coral);
    painter.circle_filled(rect.left_center() + vec2(8.0, 0.0), 2.5, white);
    painter.galley(rect.min + vec2(15.0, 4.0), galley, white);
    if executor.busy() || executor.idle_since.is_some() {
        ctx.request_repaint();
    }
}
