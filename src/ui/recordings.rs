//! A janela das ligações gravadas: ouvir, transcrever, resumir com o Claude,
//! copiar e apagar. Abre pelo botão de gravação no trilho da esquerda.

use egui::{Align, CornerRadius, Frame, Id, Layout, Margin, Rect, RichText, Sense, Stroke, Vec2, vec2};

use crate::app::App;
use crate::model::Action;
use crate::theme::{self, Icon};

/// O botão do trilho, abaixo do Claude. Um ponto vermelho enquanto grava.
pub fn rail_button(app: &mut App, ui: &mut egui::Ui, size: f32) {
    let palette = app.palette;
    let (cell, _) = ui.allocate_exact_size(vec2(ui.available_width(), size), Sense::hover());
    let plate = Rect::from_center_size(cell.center(), Vec2::splat(size));
    let response = ui
        .interact(plate, Id::new("recordings-rail-button"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Ligações gravadas e transcrições");
    if response.clicked() {
        app.actions.push(Action::ToggleRecordings);
    }
    let lit = app.recordings.open;
    if lit || response.hovered() {
        ui.painter().rect_filled(
            plate,
            CornerRadius::same(10),
            if lit {
                palette.surface_active
            } else {
                palette.surface_hover
            },
        );
    }
    let recording = crate::transcribe::is_recording();
    theme::paint_icon(
        ui,
        Icon::Record,
        plate,
        size * 0.5,
        if recording { palette.danger } else { palette.secondary },
    );
    if recording {
        ui.painter()
            .circle_filled(plate.right_top() + vec2(-6.0, 6.0), 4.0, palette.danger);
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.recordings.open {
        return;
    }
    let palette = app.palette;
    let mut open = true;
    let frame = Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS + 4))
        .inner_margin(Margin::same(18))
        .shadow(palette.modal_shadow());
    let max_height = (ctx.content_rect().height() - 80.0).max(240.0);
    egui::Window::new("Ligações gravadas")
        .id(Id::new("verkzchat-recordings"))
        .title_bar(false)
        .resizable(true)
        .collapsible(false)
        .frame(frame)
        .default_width(560.0)
        .default_height(max_height.min(620.0))
        .max_height(max_height)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(420.0_f32.min(ctx.content_rect().width() - 64.0));
            ui.horizontal(|ui| {
                theme::text(ui, "Ligações gravadas", theme::bold(19.0), palette.text);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(
                        ui,
                        Icon::X,
                        18.0,
                        palette.secondary,
                        palette.text,
                        "Fechar",
                    )
                    .clicked()
                    {
                        open = false;
                    }
                    if theme::soft_button(ui, &palette, Some(Icon::ExternalLink), "Abrir pasta", false)
                        .clicked()
                    {
                        let folder = app.recordings.folder().to_path_buf();
                        let _ = std::fs::create_dir_all(&folder);
                        let _ = open::that_detached(&folder);
                    }
                });
            });
            ui.add_space(2.0);
            let hint = if app.transcription.key().is_some() {
                "Na ligação, toque no botão de gravar. Ao desligar, a gravação é salva e transcrita sozinha, separando a sua voz da do contato."
            } else {
                "Na ligação, toque no botão de gravar. Para transcrever, coloque a chave em Configurações › Transcrição e gravações."
            };
            ui.add(
                egui::Label::new(RichText::new(hint).font(theme::regular(12.5)).color(palette.secondary))
                    .wrap(),
            );
            if app.recordings.is_saving() {
                ui.horizontal(|ui| {
                    theme::spinner(ui, 14.0, palette.accent);
                    theme::text(ui, "Salvando a gravação…", theme::regular(12.5), palette.secondary);
                });
            }
            ui.add_space(10.0);
            if app.recordings.list.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    theme::text(ui, "Nenhuma ligação gravada ainda.", theme::medium(14.0), palette.secondary);
                });
                ui.add_space(20.0);
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("recordings-list")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let ids: Vec<String> = app.recordings.list.iter().map(|r| r.id.clone()).collect();
                    for id in ids {
                        entry(app, ui, ctx, &id);
                        ui.add_space(8.0);
                    }
                });
        });
    if !open {
        app.recordings.open = false;
    }
}

/// Uma ligação: o cartão com nome, data e duração; aberto, a transcrição e o resumo.
fn entry(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context, id: &str) {
    let palette = app.palette;
    let Some(recording) = app.recordings.list.iter().find(|r| r.id == id).cloned() else {
        return;
    };
    let selected = app.recordings.selected.as_deref() == Some(id);
    let transcribing = app.recordings.is_transcribing(id);
    let summarizing = app.recordings.is_summarizing(id);
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                let (slot, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                theme::paint_icon(ui, Icon::Phone, slot, 18.0, palette.accent);
                ui.vertical(|ui| {
                    theme::text(ui, &recording.name, theme::semibold(14.5), palette.text);
                    let when = jiff::Timestamp::from_second(recording.started_at)
                        .map(|time| {
                            time.to_zoned(jiff::tz::TimeZone::system())
                                .strftime("%d/%m/%Y às %H:%M")
                                .to_string()
                        })
                        .unwrap_or_default();
                    let status = if transcribing {
                        " · transcrevendo…"
                    } else if recording.transcript.is_some() {
                        " · transcrita"
                    } else {
                        ""
                    };
                    theme::text(
                        ui,
                        format!("{when} · {}{status}", crate::transcribe::clock(recording.seconds as f32)),
                        theme::regular(12.0),
                        palette.secondary,
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let toggle = if selected { Icon::ChevronUp } else { Icon::ChevronDown };
                    if theme::icon_button(ui, toggle, 16.0, palette.secondary, palette.text, "Detalhes")
                        .clicked()
                    {
                        app.recordings.selected = if selected { None } else { Some(id.to_owned()) };
                    }
                    if theme::icon_button(ui, Icon::Play, 16.0, palette.secondary, palette.text, "Ouvir")
                        .clicked()
                    {
                        let _ = open::that_detached(&recording.path);
                    }
                });
            });
            let header_rect = header.response.rect;
            if ui
                .interact(header_rect, Id::new(("recording-row", id)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app.recordings.selected = if selected { None } else { Some(id.to_owned()) };
            }
            if let Some(error) = app.recordings.errors.get(id) {
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(RichText::new(error).font(theme::regular(12.0)).color(palette.danger))
                        .wrap(),
                );
            }
            if !selected {
                return;
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if transcribing {
                    theme::spinner(ui, 14.0, palette.accent);
                    theme::text(ui, "Transcrevendo…", theme::regular(12.5), palette.secondary);
                } else if theme::soft_button(
                    ui,
                    &palette,
                    Some(Icon::Captions),
                    if recording.transcript.is_some() { "Transcrever de novo" } else { "Transcrever" },
                    recording.transcript.is_none(),
                )
                .clicked()
                {
                    let config = app.transcription.clone();
                    app.recordings.transcribe(&config, id, ctx);
                }
                if recording.transcript.is_some() {
                    if summarizing {
                        theme::spinner(ui, 14.0, palette.accent);
                        theme::text(ui, "O Claude está resumindo…", theme::regular(12.5), palette.secondary);
                    } else if theme::soft_button(
                        ui,
                        &palette,
                        None,
                        if recording.summary.is_some() { "Resumir de novo" } else { "Resumir com o Claude" },
                        recording.summary.is_none(),
                    )
                    .clicked()
                    {
                        let key = app.claude.store.key();
                        let model = app.claude.store.model.clone();
                        app.recordings.summarize(key, &model, id, ctx);
                    }
                    if theme::soft_button(ui, &palette, Some(Icon::Copy), "Copiar transcrição", false).clicked()
                        && let Some(text) = recording.transcript_text()
                    {
                        ui.ctx().copy_text(text);
                        app.toast("Transcrição copiada.".to_owned());
                    }
                }
                if theme::soft_button(ui, &palette, Some(Icon::Trash), "Apagar", false).clicked() {
                    app.recordings.delete(id);
                }
            });
            if let Some(summary) = &recording.summary {
                ui.add_space(10.0);
                theme::text(ui, "Resumo do Claude", theme::semibold(13.0), palette.text);
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(RichText::new(summary).font(theme::regular(13.0)).color(palette.text))
                        .wrap()
                        .selectable(true),
                );
                if theme::link(ui, "Copiar resumo", theme::medium(12.0), palette.link).clicked() {
                    ui.ctx().copy_text(summary.clone());
                }
            }
            if let Some(lines) = &recording.transcript {
                ui.add_space(10.0);
                theme::text(ui, "Transcrição", theme::semibold(13.0), palette.text);
                ui.add_space(4.0);
                if lines.is_empty() {
                    theme::text(ui, "(sem fala reconhecida)", theme::regular(12.5), palette.secondary);
                }
                for line in lines {
                    ui.horizontal_top(|ui| {
                        theme::text(
                            ui,
                            crate::transcribe::clock(line.start),
                            theme::tabular(fastframe_fonts::Weight::Regular, 11.5),
                            palette.dim,
                        );
                        let (who, colour) = if line.mine {
                            ("Você", palette.accent)
                        } else {
                            (recording.name.as_str(), palette.link)
                        };
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{who}: "))
                                    .font(theme::semibold(13.0))
                                    .color(colour),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&line.text).font(theme::regular(13.0)).color(palette.text))
                                .wrap()
                                .selectable(true),
                        );
                    });
                }
            }
        });
}
