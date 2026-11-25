//! Relatório do SDR: números calculados aqui a partir das análises e dos
//! follow-ups, e um relatório completo escrito pelo Claude.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::json;

use super::analysis::Temperature;
use super::api;
use super::followup::Status;
use super::store::Store;
use crate::app::App;
use crate::model::ChatId;

pub struct Priority {
    pub chat: ChatId,
    pub name: String,
    pub score: u8,
    pub why: String,
}

#[derive(Default)]
pub struct Stats {
    pub active: usize,
    pub analysed: usize,
    pub hot: usize,
    pub warm: usize,
    pub cold: usize,
    pub sent: usize,
    pub follow_ups_sent: usize,
    pub answered: usize,
    pub objections: Vec<(String, usize)>,
    pub priorities: Vec<Priority>,
    pub scheduled: usize,
}

impl Stats {
    pub fn response_rate(&self) -> Option<f32> {
        (self.follow_ups_sent > 0).then(|| self.answered as f32 / self.follow_ups_sent as f32)
    }
}

pub fn stats(store: &Store, app: &App, days: u32) -> Stats {
    let now = crate::util::now();
    let since = now - i64::from(days) * 24 * 3600;
    let mut stats = Stats {
        active: app
            .chats
            .iter()
            .filter(|chat| !chat.archived && !chat.is_group() && chat.last_activity >= since)
            .count(),
        ..Stats::default()
    };
    let mut objections: HashMap<String, (String, usize)> = HashMap::new();
    let mut priorities = Vec::new();
    for (chat, analysis) in &store.analyses {
        if analysis.analysed_at < since {
            continue;
        }
        stats.analysed += 1;
        match analysis.temperature() {
            Temperature::Hot => stats.hot += 1,
            Temperature::Warm => stats.warm += 1,
            Temperature::Cold => stats.cold += 1,
        }
        for objection in &analysis.objections {
            let key = objection.label.trim().to_lowercase();
            if key.is_empty() {
                continue;
            }
            objections
                .entry(key)
                .or_insert_with(|| (objection.label.trim().to_owned(), 0))
                .1 += 1;
        }
        // Prioridade: quente ou morno com a bola com o SDR, ou parado.
        let waiting_on_sdr = app
            .chat(chat)
            .and_then(|chat| chat.last.as_ref())
            .is_some_and(|last| !last.from_me);
        if analysis.score >= 50 {
            let why = if waiting_on_sdr {
                "Lead respondeu e está esperando você".to_owned()
            } else if let Some(objection) = analysis.objections.first() {
                format!("Objeção em aberto: {}", objection.label)
            } else {
                analysis.stage.clone()
            };
            priorities.push(Priority {
                chat: chat.clone(),
                name: analysis.chat_name.clone(),
                score: analysis.score + if waiting_on_sdr { 10 } else { 0 },
                why,
            });
        }
    }
    priorities.sort_by_key(|priority| std::cmp::Reverse(priority.score));
    for priority in &mut priorities {
        priority.score = priority.score.min(100);
        if let Some(analysis) = store.analyses.get(&priority.chat) {
            priority.score = analysis.score;
        }
    }
    priorities.truncate(5);
    stats.priorities = priorities;
    let mut objections: Vec<(String, usize)> = objections.into_values().collect();
    objections.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    objections.truncate(6);
    stats.objections = objections;
    for sent in store.sent.iter().filter(|sent| sent.at >= since) {
        stats.sent += 1;
        if sent.follow_up {
            stats.follow_ups_sent += 1;
            if super::followup::replied_since(app, &sent.chat, sent.at) {
                stats.answered += 1;
            }
        }
    }
    stats.scheduled = store
        .follow_ups
        .iter()
        .filter(|follow_up| follow_up.status == Status::Scheduled)
        .count();
    stats
}

pub type Job = std::sync::mpsc::Receiver<api::Reply>;

/// Pede ao Claude o relatório completo da carteira.
pub fn start(store: &Store, app: &App, days: u32, ctx: &egui::Context) -> Result<Job, String> {
    let key = store.key().ok_or("Coloque a chave da API do Claude em Config.")?;
    let stats = stats(store, app, days);
    let since = crate::util::now() - i64::from(days) * 24 * 3600;
    let mut leads: Vec<_> = store.analyses.values().filter(|analysis| analysis.analysed_at >= since).collect();
    leads.sort_by_key(|analysis| std::cmp::Reverse(analysis.score));
    if leads.is_empty() {
        return Err("Nenhum lead analisado no período. Abra as conversas para o Claude analisar.".into());
    }
    let leads: Vec<_> = leads
        .iter()
        .take(60)
        .map(|analysis| {
            json!({
                "lead": analysis.chat_name,
                "score": analysis.score,
                "etapa": analysis.stage,
                "resumo": analysis.summary,
                "objecoes": analysis.objections.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
                "sinais": analysis.buying_signals,
            })
        })
        .collect();
    let profile = store.profile();
    let prompt = format!(
        "Você é o gestor comercial de um SDR ({name}) que vende: {product}.\n\
         Escreva um relatório completo dos últimos {days} dias em Markdown, em português do Brasil, direto e útil.\n\n\
         Números: {active} conversas ativas, {analysed} leads analisados ({hot} quentes, {warm} mornos, {cold} frios), \
         {sent} mensagens enviadas pelo Claude, {fu} follow-ups enviados e {answered} respondidos, {scheduled} follow-ups agendados.\n\n\
         Leads:\n{leads}\n\n\
         Seções: 1) Resumo da semana em 3 linhas, 2) Leads para fechar agora (com o próximo passo de cada), \
         3) Objeções mais comuns e como contornar cada uma, 4) Leads em risco, 5) Três ajustes no processo do SDR. \
         Use só os dados acima; não invente números.",
        name = profile.name,
        product = if profile.product.trim().is_empty() { "(não informado)" } else { profile.product.trim() },
        active = stats.active,
        analysed = stats.analysed,
        hot = stats.hot,
        warm = stats.warm,
        cold = stats.cold,
        sent = stats.sent,
        fu = stats.follow_ups_sent,
        answered = stats.answered,
        scheduled = stats.scheduled,
        leads = serde_json::to_string_pretty(&leads).unwrap_or_default(),
    );
    let body = json!({
        "model": store.model,
        "max_tokens": 6000,
        "messages": [{ "role": "user", "content": prompt }],
    });
    Ok(api::call(key, body, ctx.clone()))
}

/// Salva o relatório em Markdown na pasta de downloads (ou na de estado).
pub fn save(markdown: &str, folder: Option<PathBuf>, state: &std::path::Path) -> Option<PathBuf> {
    let folder = folder
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")))
        .filter(|folder| folder.is_dir())
        .unwrap_or_else(|| state.join("relatorios"));
    std::fs::create_dir_all(&folder).ok()?;
    let name = format!("relatorio-verkzchat-{}.md", jiff::Zoned::now().strftime("%Y-%m-%d-%H%M"));
    let path = folder.join(name);
    std::fs::write(&path, markdown).ok()?;
    Some(path)
}
