//! Chamadas à Messages API da Anthropic, sempre numa thread própria para a
//! janela nunca travar. O resultado volta por um canal e a janela é acordada.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::Value;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2023-06-01";

pub type Reply = Result<Value, String>;

/// Envia `body` (já com `model`, `messages` etc.) e devolve a resposta JSON.
pub fn call(key: String, body: Value, ctx: egui::Context) -> Receiver<Reply> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("claude-api".into())
        .spawn(move || {
            let result = send(&key, &body);
            let _ = sender.send(result);
            ctx.request_repaint();
        })
        .expect("thread do Claude");
    receiver
}

fn send(key: &str, body: &Value) -> Reply {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;
    let mut last_error = String::new();
    // Sobrecarga e limite de taxa costumam passar em segundos.
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2 * attempt));
        }
        let response = match client
            .post(ENDPOINT)
            .header("x-api-key", key)
            .header("anthropic-version", VERSION)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body).unwrap_or_default())
            .send()
        {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("Sem conexão com a API do Claude: {error}");
                continue;
            }
        };
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str(&text).map_err(|error| format!("Resposta inválida: {error}"));
        }
        last_error = describe(status.as_u16(), &text);
        if !matches!(status.as_u16(), 429 | 500 | 502 | 503 | 529) {
            break;
        }
    }
    Err(last_error)
}

fn describe(status: u16, body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        401 => "Chave da API inválida. Confira em Config.".into(),
        403 => format!("A chave não tem acesso a este modelo: {message}"),
        429 => "Limite de uso da API atingido. Tente de novo em instantes.".into(),
        529 => "A API do Claude está sobrecarregada. Tente de novo em instantes.".into(),
        _ => format!("Erro {status} da API: {message}"),
    }
}

/// Junta os blocos de texto de uma resposta.
pub fn text_of(reply: &Value) -> String {
    reply["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Extrai o primeiro objeto JSON de um texto, com ou sem bloco ```json.
pub fn json_in(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}
