//! `verkzchat mcp`: o conector que o Claude Desktop e o Claude Code iniciam.
//! Fala MCP (JSON-RPC por linhas no stdin/stdout) e repassa cada ferramenta
//! ao VerkzChat aberto, que executa com o cursor visível.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

const INSTRUCTIONS: &str = "Você controla o WhatsApp do SDR dentro do VerkzChat. Cada ação aparece na tela dele como um cursor coral. \
Comece chamando get_sdr_profile para saber o que ele vende, o tom e as permissões. \
Para mandar mensagem: type_message e depois send_message; o envio pode esperar a aprovação do SDR, e se ele cancelar, pare. \
Ao analisar um lead, leia a conversa com read_chat e salve com save_lead_analysis para aparecer no painel. \
Para follow-ups vencidos: list_due_follow_ups, leia cada conversa, ajuste o rascunho ao contexto, digite, envie e feche com complete_follow_up. \
Escreva para os leads em português do Brasil, no tom do perfil.";

/// Converte as ferramentas do agente para o formato do MCP.
fn tools() -> Value {
    let list: Vec<Value> = super::agent::tools()
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool["name"],
                "description": tool["description"],
                "inputSchema": tool["input_schema"],
            })
        })
        .collect();
    json!({ "tools": list })
}

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Responde a uma mensagem; `None` para notificações.
pub fn handle(message: &Value, call: &dyn Fn(&str, &Value) -> Result<String, String>) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or_default();
    Some(match method {
        "initialize" => {
            let version = message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "verkzchat", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => reply(&id, json!({})),
        "tools/list" => reply(&id, tools()),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or_default();
            let args = message["params"]["arguments"].clone();
            let args = if args.is_null() { json!({}) } else { args };
            let (text, failed) = match call(name, &args) {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            reply(&id, json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
        }
        "resources/list" => reply(&id, json!({ "resources": [] })),
        "prompts/list" => reply(&id, json!({ "prompts": [] })),
        _ => error(&id, -32601, "Método não suportado"),
    })
}

/// Roda o conector até o stdin fechar. Devolve o código de saída.
pub fn run() -> i32 {
    // VERKZCHAT_RUNTIME_DIR aponta para outra cópia (testes, modo demo).
    let runtime = std::env::var_os("VERKZCHAT_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::paths::AppDirs::discover().runtime);
    let call = |tool: &str, args: &Value| super::bridge::call(&runtime, tool, args);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let answers: Vec<Value> = batch.iter().filter_map(|message| handle(message, &call)).collect();
                (!answers.is_empty()).then(|| Value::Array(answers))
            }
            Ok(message) => handle(&message, &call),
            Err(_) => Some(error(&Value::Null, -32700, "JSON inválido")),
        };
        if let Some(answer) = answer {
            if writeln!(stdout, "{answer}").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_introduces_itself_and_lists_the_whatsapp_tools() {
        let none = |_: &str, _: &Value| -> Result<String, String> { unreachable!() };
        let init = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}), &none).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "verkzchat");
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), &none).is_none());
        let list = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), &none).unwrap();
        let names: Vec<_> = list["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap().to_owned()).collect();
        for wanted in ["read_chat", "type_message", "send_message", "save_lead_analysis", "list_due_follow_ups"] {
            assert!(names.iter().any(|name| name == wanted), "{wanted}: {names:?}");
        }
        assert!(list["result"]["tools"][0]["inputSchema"].is_object());
    }

    #[test]
    fn a_tool_call_goes_to_the_app_and_errors_come_back_marked() {
        let app = |tool: &str, args: &Value| -> Result<String, String> {
            if tool == "read_chat" { Ok(format!("lido {}", args["chat"])) } else { Err("negado".into()) }
        };
        let ok = handle(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_chat","arguments":{"chat":"Ana"}}}), &app).unwrap();
        assert_eq!(ok["result"]["isError"], false);
        assert_eq!(ok["result"]["content"][0]["text"], "lido \"Ana\"");
        let bad = handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"send_message"}}), &app).unwrap();
        assert_eq!(bad["result"]["isError"], true);
    }

    #[test]
    fn the_bridge_carries_a_call_to_the_app_and_back() {
        let folder = tempfile::tempdir().unwrap();
        let bridge = super::super::bridge::start(folder.path(), egui::Context::default()).unwrap();
        let runtime = folder.path().to_owned();
        let caller = std::thread::spawn(move || super::super::bridge::call(&runtime, "get_sdr_profile", &json!({})));
        let request = bridge.requests.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(request.tool, "get_sdr_profile");
        request.reply.send(Ok("perfil".into())).unwrap();
        assert_eq!(caller.join().unwrap(), Ok("perfil".into()));
    }
}
