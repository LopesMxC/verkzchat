//! Canal local entre o VerkzChat aberto e o conector `verkzchat mcp` que o
//! Claude Desktop ou o Claude Code iniciam. Só aceita conexões da própria
//! máquina (127.0.0.1) e com a senha gravada num arquivo que só o usuário lê.
//! Cada conexão leva um pedido de ferramenta e espera a resposta, que pode
//! demorar (o cursor anda, o SDR aprova o envio).

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Quanto o conector espera por uma ferramenta (inclui a aprovação do SDR).
pub const REPLY_TIME: Duration = Duration::from_secs(15 * 60);

/// Um pedido do Claude, à espera da resposta do app.
pub struct Request {
    pub tool: String,
    pub args: Value,
    pub reply: Sender<Result<String, String>>,
}

pub struct Bridge {
    pub requests: Receiver<Request>,
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    token: String,
    tool: String,
    #[serde(default)]
    args: Value,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    ok: bool,
    text: String,
}

/// Onde o app anota a porta e a senha do canal.
pub fn endpoint_file(runtime: &Path) -> PathBuf {
    runtime.join("claude-bridge.json")
}

fn token() -> String {
    let mut bytes = [0u8; 24];
    if getrandom::fill(&mut bytes).is_err() {
        // Sem fonte de acaso, a hora e o processo ainda diferenciam execuções.
        let seed = format!("{:?}{}", std::time::SystemTime::now(), std::process::id());
        for (index, byte) in seed.bytes().enumerate() {
            bytes[index % bytes.len()] ^= byte;
        }
    }
    crate::settings::hex(&bytes)
}

/// Abre o canal e anota o endereço. O app chama uma vez.
pub fn start(runtime: &Path, ctx: egui::Context) -> std::io::Result<Bridge> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    let port = listener.local_addr()?.port();
    let secret = token();
    std::fs::create_dir_all(runtime)?;
    let file = endpoint_file(runtime);
    let text = serde_json::to_string(&Endpoint { port, token: secret.clone() }).map_err(std::io::Error::other)?;
    let temporary = file.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, &file)?;
    let (sender, requests) = mpsc::channel();
    std::thread::Builder::new()
        .name("claude-bridge".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let sender = sender.clone();
                let secret = secret.clone();
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new()
                    .name("claude-bridge-call".into())
                    .spawn(move || serve(stream, &secret, &sender, &ctx));
            }
        })?;
    Ok(Bridge { requests })
}

fn serve(stream: TcpStream, secret: &str, sender: &Sender<Request>, ctx: &egui::Context) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let answer = match serde_json::from_str::<Wire>(&line) {
        Ok(wire) if constant_eq(&wire.token, secret) => {
            let (reply, waiting) = mpsc::channel();
            if sender
                .send(Request {
                    tool: wire.tool,
                    args: wire.args,
                    reply,
                })
                .is_err()
            {
                Answer { ok: false, text: "O VerkzChat está fechando.".into() }
            } else {
                ctx.request_repaint();
                match waiting.recv_timeout(REPLY_TIME) {
                    Ok(Ok(text)) => Answer { ok: true, text },
                    Ok(Err(text)) => Answer { ok: false, text },
                    Err(_) => Answer { ok: false, text: "O VerkzChat não respondeu a tempo.".into() },
                }
            }
        }
        Ok(_) => Answer { ok: false, text: "Senha do canal inválida.".into() },
        Err(_) => Answer { ok: false, text: "Pedido inválido.".into() },
    };
    let mut stream = stream;
    if let Ok(text) = serde_json::to_string(&answer) {
        let _ = writeln!(stream, "{text}");
    }
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Lado do conector: manda um pedido ao app aberto e espera a resposta.
pub fn call(runtime: &Path, tool: &str, args: &Value) -> Result<String, String> {
    let file = endpoint_file(runtime);
    let endpoint: Endpoint = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or_else(|| "O VerkzChat não está aberto. Abra o VerkzChat e tente de novo.".to_owned())?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port)),
        Duration::from_secs(3),
    )
    .map_err(|_| "O VerkzChat não está aberto. Abra o VerkzChat e tente de novo.".to_owned())?;
    let wire = Wire {
        token: endpoint.token,
        tool: tool.to_owned(),
        args: args.clone(),
    };
    let text = serde_json::to_string(&wire).map_err(|error| error.to_string())?;
    writeln!(stream, "{text}").map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(REPLY_TIME + Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|_| "O VerkzChat não respondeu.".to_owned())?;
    let answer: Answer = serde_json::from_str(&line).map_err(|_| "Resposta inválida do VerkzChat.".to_owned())?;
    if answer.ok { Ok(answer.text) } else { Err(answer.text) }
}
