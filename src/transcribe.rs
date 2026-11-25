//! Transcrição de áudio e gravação de ligações.
//!
//! A transcrição usa a API de transcrição no formato da OpenAI
//! (`/v1/audio/transcriptions`, modelo Whisper), que a Groq e a OpenAI
//! oferecem. O áudio é decodificado e regravado como OGG/Opus mono antes de
//! sair, então qualquer formato que o VerkzChat toca serve e o arquivo fica
//! pequeno (cerca de 15 MB por hora).
//!
//! As ligações gravadas ficam em `<dados>/gravacoes`, com três arquivos por
//! ligação: a mistura (para ouvir), a sua voz e a do contato. As duas trilhas
//! separadas deixam a transcrição dizer quem falou cada frase.
//!
//! Tudo o que fala com a rede ou codifica áudio roda numa thread própria; a
//! janela só pergunta, a cada quadro, se já chegou a resposta.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// O maior arquivo que as APIs de transcrição aceitam.
const MAX_UPLOAD: usize = 24 * 1024 * 1024;

// ─── Configuração ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Groq: Whisper large v3 turbo, rápido e barato.
    #[default]
    Groq,
    /// OpenAI: Whisper 1.
    OpenAi,
    /// Qualquer serviço com a API de transcrição no formato da OpenAI: o
    /// endereço, o modelo e a chave vêm da configuração.
    Custom,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Self::Groq, Self::OpenAi, Self::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Self::Groq => "Groq (Whisper large v3 turbo)",
            Self::OpenAi => "OpenAI (Whisper)",
            Self::Custom => "Personalizado (qualquer API)",
        }
    }

    /// O endereço padrão; o personalizado não tem.
    pub fn default_endpoint(self) -> &'static str {
        match self {
            Self::Groq => "https://api.groq.com/openai/v1/audio/transcriptions",
            Self::OpenAi => "https://api.openai.com/v1/audio/transcriptions",
            Self::Custom => "",
        }
    }

    /// O modelo padrão; o personalizado não tem.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Groq => "whisper-large-v3-turbo",
            Self::OpenAi => "whisper-1",
            Self::Custom => "",
        }
    }

    fn env(self) -> &'static str {
        match self {
            Self::Groq => "GROQ_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Custom => "TRANSCRIPTION_API_KEY",
        }
    }

    pub fn key_hint(self) -> &'static str {
        match self {
            Self::Groq => "gsk_… (console.groq.com/keys)",
            Self::OpenAi => "sk-… (platform.openai.com/api-keys)",
            Self::Custom => "a chave do serviço",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: Provider,
    pub api_key: String,
    /// Endereço da API de transcrição. Vazio usa o padrão do provedor.
    pub endpoint: String,
    /// Nome do modelo. Vazio usa o padrão do provedor.
    pub model: String,
    /// Idioma falado, em ISO 639-1. Vazio deixa a API descobrir.
    pub language: String,
    /// Transcreve sozinho as mensagens de voz recebidas, assim que baixam.
    pub auto_voice_notes: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: Provider::Groq,
            api_key: String::new(),
            endpoint: String::new(),
            model: String::new(),
            language: "pt".into(),
            auto_voice_notes: false,
        }
    }
}

impl Config {
    fn path(config: &Path) -> PathBuf {
        config.join("transcricao.json")
    }

    pub fn load(config: &Path) -> Self {
        std::fs::read_to_string(Self::path(config))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, config: &Path) {
        if let Err(error) = write_private(&Self::path(config), self) {
            log::warn!("não deu para salvar a configuração da transcrição: {error}");
        }
    }

    /// O endereço em uso: o escrito ou o padrão do provedor. Aceita também só
    /// a base (`https://…/v1`), completando com `/audio/transcriptions`.
    pub fn endpoint(&self) -> String {
        let written = self.endpoint.trim().trim_end_matches('/');
        if written.is_empty() {
            return self.provider.default_endpoint().to_owned();
        }
        if written.ends_with("/audio/transcriptions") || written.contains('?') {
            written.to_owned()
        } else if written.ends_with("/v1") || written.ends_with("/openai/v1") {
            format!("{written}/audio/transcriptions")
        } else {
            written.to_owned()
        }
    }

    /// O modelo em uso: o escrito ou o padrão do provedor.
    pub fn model(&self) -> String {
        let written = self.model.trim();
        if written.is_empty() {
            self.provider.default_model().to_owned()
        } else {
            written.to_owned()
        }
    }

    /// O que falta para transcrever, em uma frase, ou `None` se está pronto.
    pub fn missing(&self) -> Option<&'static str> {
        if self.endpoint().is_empty() {
            Some("Coloque o endereço da API em Configurações › Transcrição e gravações.")
        } else if self.model().is_empty() {
            Some("Coloque o nome do modelo em Configurações › Transcrição e gravações.")
        } else {
            None
        }
    }

    /// A chave salva ou a da variável de ambiente do provedor.
    pub fn key(&self) -> Option<String> {
        let saved = self.api_key.trim();
        if !saved.is_empty() {
            return Some(saved.to_owned());
        }
        std::env::var(self.provider.env())
            .ok()
            .filter(|key| !key.trim().is_empty())
            // Um servidor próprio (Whisper local, por exemplo) pode não pedir chave.
            .or_else(|| (self.provider == Provider::Custom).then(String::new))
    }
}

/// Grava JSON por um arquivo temporário, legível só pelo usuário.
fn write_private(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&temporary, path)
}

// ─── Chamada à API ───────────────────────────────────────────────────────────

/// Um trecho falado, com o segundo em que começa.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f32,
    pub text: String,
}

/// O áudio de um arquivo como OGG/Opus mono, pronto para enviar.
fn prepared(path: &Path) -> Result<Vec<u8>, String> {
    let samples = crate::audio::decode_file(path)?;
    if samples.is_empty() {
        return Err("O áudio está vazio.".into());
    }
    let bytes = crate::voice::encode(&samples)?;
    if bytes.len() > MAX_UPLOAD {
        return Err("O áudio passa de 24 MB, o limite da transcrição.".into());
    }
    Ok(bytes)
}

/// Envia o áudio e devolve os trechos (com o tempo de cada um).
fn request(config: &Config, key: &str, audio: Vec<u8>) -> Result<Vec<Segment>, String> {
    if let Some(missing) = config.missing() {
        return Err(missing.into());
    }
    // Nem todo serviço devolve os trechos com tempo (verbose_json); esses
    // recebem o texto simples.
    match request_as(config, key, &audio, "verbose_json") {
        Err(error) if error.starts_with("Erro 400") || error.starts_with("Erro 422") => {
            request_as(config, key, &audio, "json")
        }
        other => other,
    }
}

fn request_as(
    config: &Config,
    key: &str,
    audio: &[u8],
    format: &str,
) -> Result<Vec<Segment>, String> {
    let boundary = format!("verkzchat-{:x}", rand::random::<u64>());
    let mut body = Vec::with_capacity(audio.len() + 1024);
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    };
    let model = config.model();
    field("model", &model);
    field("response_format", format);
    field("temperature", "0");
    let language = config.language.trim();
    if !language.is_empty() {
        field("language", language);
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.ogg\"\r\nContent-Type: audio/ogg\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let mut last_error = String::new();
    for attempt in 0..3u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2 * attempt));
        }
        let response = match client
            .post(config.endpoint())
            .bearer_auth(key)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body.clone())
            .send()
        {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("Sem conexão com o serviço de transcrição: {error}");
                continue;
            }
        };
        let status = response.status().as_u16();
        let text = response.text().unwrap_or_default();
        if (200..300).contains(&status) {
            return parse(&text);
        }
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| text.chars().take(200).collect());
        last_error = match status {
            401 => "Chave de transcrição inválida. Confira em Configurações › Transcrição.".into(),
            413 => "O áudio é grande demais para a transcrição.".into(),
            429 => "Limite de uso da transcrição atingido. Tente de novo em instantes.".into(),
            _ => format!("Erro {status} na transcrição: {message}"),
        };
        if !matches!(status, 429 | 500 | 502 | 503) {
            break;
        }
    }
    Err(last_error)
}

fn parse(text: &str) -> Result<Vec<Segment>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("Resposta inválida: {error}"))?;
    let segments: Vec<Segment> = value["segments"]
        .as_array()
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    let text = segment["text"].as_str()?.trim();
                    (!text.is_empty()).then(|| Segment {
                        start: segment["start"].as_f64().unwrap_or(0.0) as f32,
                        text: text.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if !segments.is_empty() {
        return Ok(segments);
    }
    let whole = value["text"].as_str().unwrap_or_default().trim();
    Ok(if whole.is_empty() {
        Vec::new()
    } else {
        vec![Segment {
            start: 0.0,
            text: whole.to_owned(),
        }]
    })
}

/// Transcreve um arquivo de áudio, juntando os trechos num texto só.
pub fn transcribe_file(config: &Config, key: &str, path: &Path) -> Result<String, String> {
    let segments = request(config, key, prepared(path)?)?;
    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        Ok("(sem fala reconhecida)".into())
    } else {
        Ok(text)
    }
}

// ─── Mensagens de voz ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// Texto por id de mensagem.
    texts: HashMap<String, String>,
}

/// As transcrições das mensagens de voz: as prontas, as em andamento e os erros.
pub struct Transcriber {
    path: PathBuf,
    texts: HashMap<String, String>,
    running: HashMap<String, Receiver<Result<String, String>>>,
    errors: HashMap<String, String>,
    /// Mensagens já pedidas pela transcrição automática, para não repetir.
    asked: std::collections::HashSet<String>,
}

impl Transcriber {
    pub fn new(data: &Path) -> Self {
        let path = data.join("transcricoes.json");
        let saved: Saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            texts: saved.texts,
            running: HashMap::new(),
            errors: HashMap::new(),
            asked: Default::default(),
        }
    }

    pub fn text(&self, message: &str) -> Option<&str> {
        self.texts.get(message).map(String::as_str)
    }

    pub fn is_running(&self, message: &str) -> bool {
        self.running.contains_key(message)
    }

    pub fn error(&self, message: &str) -> Option<&str> {
        self.errors.get(message).map(String::as_str)
    }

    /// Esquece a transcrição de uma mensagem (para transcrever de novo).
    pub fn forget(&mut self, message: &str) {
        if self.texts.remove(message).is_some() {
            self.save();
        }
        self.errors.remove(message);
    }

    /// Começa a transcrever. Sem chave, guarda o erro para a bolha mostrar.
    pub fn start(&mut self, config: &Config, message: String, path: PathBuf, ctx: &egui::Context) {
        if self.running.contains_key(&message) {
            return;
        }
        self.asked.insert(message.clone());
        self.errors.remove(&message);
        let Some(key) = config.key() else {
            self.errors.insert(
                message,
                "Coloque a chave da transcrição em Configurações › Transcrição e gravações.".into(),
            );
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let config = config.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("transcricao".into())
            .spawn(move || {
                let _ = sender.send(transcribe_file(&config, &key, &path));
                ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.running.insert(message, receiver);
            }
            Err(error) => {
                self.errors.insert(message, error.to_string());
            }
        }
    }

    /// Se a transcrição automática ainda não pediu esta mensagem.
    pub fn wants_auto(&self, message: &str) -> bool {
        !self.asked.contains(message) && !self.texts.contains_key(message)
    }

    /// Recolhe as respostas que chegaram. Devolve se algo mudou.
    pub fn poll(&mut self) -> bool {
        let mut finished = Vec::new();
        for (message, receiver) in &self.running {
            match receiver.try_recv() {
                Ok(result) => finished.push((message.clone(), result)),
                Err(TryRecvError::Disconnected) => finished.push((
                    message.clone(),
                    Err("A transcrição parou no meio.".into()),
                )),
                Err(TryRecvError::Empty) => {}
            }
        }
        let changed = !finished.is_empty();
        let mut save = false;
        for (message, result) in finished {
            self.running.remove(&message);
            match result {
                Ok(text) => {
                    self.texts.insert(message, text);
                    save = true;
                }
                Err(error) => {
                    self.errors.insert(message, error);
                }
            }
        }
        if save {
            self.save();
        }
        changed
    }

    pub fn busy(&self) -> bool {
        !self.running.is_empty()
    }

    fn save(&self) {
        let saved = Saved {
            texts: self.texts.clone(),
        };
        if let Err(error) = write_private(&self.path, &saved) {
            log::warn!("não deu para salvar as transcrições: {error}");
        }
    }
}

// ─── Gravação de ligações ────────────────────────────────────────────────────

/// O relógio das ligações: 16 kHz mono, como o motor de chamadas.
const CALL_RATE: u32 = crate::call_audio::RATE;

struct Recorder {
    started: Instant,
    started_at: i64,
    chat: String,
    name: String,
    me: Vec<i16>,
    them: Vec<i16>,
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

fn recorder() -> std::sync::MutexGuard<'static, Option<Recorder>> {
    RECORDER.lock().unwrap_or_else(|error| error.into_inner())
}

/// Põe um quadro na trilha, no ponto do relógio em que ele soou: um silêncio
/// (microfone mudo, rede parada) vira silêncio na gravação, e as duas trilhas
/// continuam alinhadas.
fn place(track: &mut Vec<i16>, started: Instant, frame: &[i16]) {
    let now = (started.elapsed().as_secs_f64() * f64::from(CALL_RATE)) as usize;
    let begin = now.saturating_sub(frame.len());
    // Uma folga de 100 ms evita picotar quando os quadros chegam em rajada.
    if track.len() + (CALL_RATE as usize / 10) < begin {
        track.resize(begin, 0);
    }
    track.extend_from_slice(frame);
}

/// O microfone, já sem eco, a caminho do motor.
pub fn record_mine(frame: &[i16]) {
    if let Some(recorder) = recorder().as_mut() {
        let started = recorder.started;
        place(&mut recorder.me, started, frame);
    }
}

/// A voz do contato, a caminho do alto-falante.
pub fn record_theirs(frame: &[i16]) {
    if let Some(recorder) = recorder().as_mut() {
        let started = recorder.started;
        place(&mut recorder.them, started, frame);
    }
}

pub fn is_recording() -> bool {
    recorder().is_some()
}

/// Há quanto tempo a gravação começou.
pub fn recording_for() -> Option<Duration> {
    recorder().as_ref().map(|recorder| recorder.started.elapsed())
}

fn start_recorder(chat: String, name: String) {
    *recorder() = Some(Recorder {
        started: Instant::now(),
        started_at: crate::util::now(),
        chat,
        name,
        me: Vec::new(),
        them: Vec::new(),
    });
}

fn stop_recorder() -> Option<Recorder> {
    recorder().take()
}

/// Uma linha da transcrição de uma ligação.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub start: f32,
    /// `true` para você, `false` para o contato.
    pub mine: bool,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub chat: String,
    pub name: String,
    pub started_at: i64,
    pub seconds: u32,
    /// A mistura das duas vozes, para ouvir.
    pub path: PathBuf,
    pub mine: PathBuf,
    pub theirs: PathBuf,
    #[serde(default)]
    pub transcript: Option<Vec<Line>>,
    #[serde(default)]
    pub summary: Option<String>,
}

impl Recording {
    /// A transcrição como texto corrido, "Você: …" e "Contato: …".
    pub fn transcript_text(&self) -> Option<String> {
        let lines = self.transcript.as_ref()?;
        Some(
            lines
                .iter()
                .map(|line| {
                    format!(
                        "[{}] {}: {}",
                        clock(line.start),
                        if line.mine { "Você" } else { self.name.as_str() },
                        line.text
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// "03:27".
pub fn clock(seconds: f32) -> String {
    let seconds = seconds.max(0.0) as u32;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

enum Job {
    /// Codificando os arquivos de uma ligação que acabou.
    Saving(Receiver<Result<Recording, String>>),
    /// Transcrevendo a ligação `id`.
    Transcribing(String, Receiver<Result<Vec<Line>, String>>),
    /// O Claude resumindo a ligação `id`.
    Summarizing(String, Receiver<crate::claude::api::Reply>),
}

/// As ligações gravadas e o que está sendo feito com elas.
pub struct Recordings {
    dir: PathBuf,
    pub list: Vec<Recording>,
    jobs: Vec<Job>,
    pub errors: HashMap<String, String>,
    /// A janela das gravações está aberta.
    pub open: bool,
    /// A ligação aberta na janela.
    pub selected: Option<String>,
    /// Um aviso para a janela mostrar uma vez ("Gravação salva").
    pub notice: Option<String>,
}

impl Recordings {
    pub fn new(data: &Path) -> Self {
        let dir = data.join("gravacoes");
        let list = std::fs::read_to_string(dir.join("gravacoes.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            dir,
            list,
            jobs: Vec::new(),
            errors: HashMap::new(),
            open: false,
            selected: None,
            notice: None,
        }
    }

    pub fn folder(&self) -> &Path {
        &self.dir
    }

    fn save(&self) {
        if let Err(error) = write_private(&self.dir.join("gravacoes.json"), &self.list) {
            log::warn!("não deu para salvar a lista de gravações: {error}");
        }
    }

    pub fn is_saving(&self) -> bool {
        self.jobs.iter().any(|job| matches!(job, Job::Saving(_)))
    }

    pub fn is_transcribing(&self, id: &str) -> bool {
        self.jobs
            .iter()
            .any(|job| matches!(job, Job::Transcribing(current, _) if current == id))
    }

    pub fn is_summarizing(&self, id: &str) -> bool {
        self.jobs
            .iter()
            .any(|job| matches!(job, Job::Summarizing(current, _) if current == id))
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty() || is_recording()
    }

    /// Começa a gravar a ligação em andamento.
    pub fn start(&mut self, chat: String, name: String) {
        if !is_recording() {
            start_recorder(chat, name);
        }
    }

    /// Para a gravação e grava os arquivos numa thread. Com `transcribe`, a
    /// transcrição começa assim que os arquivos ficam prontos.
    pub fn stop(&mut self, ctx: &egui::Context) {
        let Some(recorder) = stop_recorder() else {
            return;
        };
        let dir = self.dir.clone();
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("gravacao".into())
            .spawn(move || {
                let _ = sender.send(write_recording(&dir, recorder));
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.jobs.push(Job::Saving(receiver));
        }
    }

    /// Transcreve as duas vozes de uma ligação e as intercala pelo tempo.
    pub fn transcribe(&mut self, config: &Config, id: &str, ctx: &egui::Context) {
        if self.is_transcribing(id) {
            return;
        }
        let Some(recording) = self.list.iter().find(|recording| recording.id == id) else {
            return;
        };
        self.errors.remove(id);
        let Some(key) = config.key() else {
            self.errors.insert(
                id.to_owned(),
                "Coloque a chave da transcrição em Configurações › Transcrição e gravações.".into(),
            );
            return;
        };
        let (mine, theirs) = (recording.mine.clone(), recording.theirs.clone());
        let config = config.clone();
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("transcricao-ligacao".into())
            .spawn(move || {
                let result = (|| {
                    let mut lines = Vec::new();
                    for (path, is_mine) in [(mine, true), (theirs, false)] {
                        let audio = match prepared(&path) {
                            Ok(audio) => audio,
                            // Uma trilha muda (o contato não falou) não é erro.
                            Err(_) => continue,
                        };
                        for segment in request(&config, &key, audio)? {
                            lines.push(Line {
                                start: segment.start,
                                mine: is_mine,
                                text: segment.text,
                            });
                        }
                    }
                    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
                    Ok(lines)
                })();
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.jobs.push(Job::Transcribing(id.to_owned(), receiver));
        }
    }

    /// Pede ao Claude o resumo da ligação para o SDR.
    pub fn summarize(&mut self, claude_key: Option<String>, model: &str, id: &str, ctx: &egui::Context) {
        if self.is_summarizing(id) {
            return;
        }
        let Some(recording) = self.list.iter().find(|recording| recording.id == id) else {
            return;
        };
        let Some(text) = recording.transcript_text() else {
            return;
        };
        let Some(key) = claude_key else {
            self.errors.insert(
                id.to_owned(),
                "Para resumir, coloque a chave da API do Claude no painel do Claude › Config.".into(),
            );
            return;
        };
        self.errors.remove(id);
        let prompt = format!(
            "Esta é a transcrição de uma ligação de pré-vendas (SDR) com {name}. \
Escreva em português, curto e direto, com estas partes:\n\
**Resumo** (3 frases no máximo)\n**Dores e interesses do lead**\n**Objeções** (e como contornar cada uma)\n\
**Temperatura do lead** (frio, morno ou quente, e por quê)\n**Próximos passos** combinados\n\
**Sugestão de mensagem de follow-up** para mandar no WhatsApp.\n\
Não use travessões; use vírgulas.\n\nTranscrição:\n{text}",
            name = recording.name
        );
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1500,
            "messages": [{"role": "user", "content": prompt}],
        });
        let receiver = crate::claude::api::call(key, body, ctx.clone());
        self.jobs.push(Job::Summarizing(id.to_owned(), receiver));
    }

    pub fn delete(&mut self, id: &str) {
        if let Some(index) = self.list.iter().position(|recording| recording.id == id) {
            let recording = self.list.remove(index);
            for path in [&recording.path, &recording.mine, &recording.theirs] {
                let _ = std::fs::remove_file(path);
            }
            if self.selected.as_deref() == Some(id) {
                self.selected = None;
            }
            self.save();
        }
    }

    /// Recolhe o que terminou. Devolve a gravação recém-salva, se houver, para
    /// a transcrição automática.
    pub fn poll(&mut self) -> Option<String> {
        let mut saved_id = None;
        let mut changed = false;
        let mut keep = Vec::new();
        for job in std::mem::take(&mut self.jobs) {
            match job {
                Job::Saving(receiver) => match receiver.try_recv() {
                    Ok(Ok(recording)) => {
                        saved_id = Some(recording.id.clone());
                        self.notice = Some(format!(
                            "Ligação com {} gravada ({}).",
                            recording.name,
                            clock(recording.seconds as f32)
                        ));
                        self.list.insert(0, recording);
                        changed = true;
                    }
                    Ok(Err(error)) => {
                        self.notice = Some(format!("A gravação não pôde ser salva: {error}"));
                    }
                    Err(TryRecvError::Empty) => keep.push(Job::Saving(receiver)),
                    Err(TryRecvError::Disconnected) => {}
                },
                Job::Transcribing(id, receiver) => match receiver.try_recv() {
                    Ok(Ok(lines)) => {
                        if let Some(recording) = self.list.iter_mut().find(|r| r.id == id) {
                            recording.transcript = Some(lines);
                            changed = true;
                        }
                    }
                    Ok(Err(error)) => {
                        self.errors.insert(id, error);
                    }
                    Err(TryRecvError::Empty) => keep.push(Job::Transcribing(id, receiver)),
                    Err(TryRecvError::Disconnected) => {}
                },
                Job::Summarizing(id, receiver) => match receiver.try_recv() {
                    Ok(Ok(reply)) => {
                        let text = crate::claude::api::text_of(&reply);
                        if let Some(recording) = self.list.iter_mut().find(|r| r.id == id) {
                            recording.summary = Some(text);
                            changed = true;
                        }
                    }
                    Ok(Err(error)) => {
                        self.errors.insert(id, error);
                    }
                    Err(TryRecvError::Empty) => keep.push(Job::Summarizing(id, receiver)),
                    Err(TryRecvError::Disconnected) => {}
                },
            }
        }
        self.jobs = keep;
        if changed {
            self.save();
        }
        saved_id
    }
}

/// Um nome de arquivo sem caracteres que o Windows recusa.
fn file_safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "contato".into()
    } else {
        trimmed.chars().take(40).collect()
    }
}

/// 16 kHz mono `i16` em OGG/Opus.
fn encode_track(samples: &[i16]) -> Result<Vec<u8>, String> {
    let floats: Vec<f32> = samples.iter().map(|&s| f32::from(s) / 32768.0).collect();
    let upsampled = crate::call_audio::resample(&floats, CALL_RATE, crate::voice::RATE);
    crate::voice::encode(&upsampled)
}

fn write_recording(dir: &Path, recorder: Recorder) -> Result<Recording, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let Recorder {
        started_at,
        chat,
        name,
        mut me,
        mut them,
        ..
    } = recorder;
    let length = me.len().max(them.len());
    if length < CALL_RATE as usize {
        return Err("a gravação tem menos de um segundo".into());
    }
    me.resize(length, 0);
    them.resize(length, 0);
    let mix: Vec<i16> = me
        .iter()
        .zip(&them)
        .map(|(&a, &b)| (i32::from(a) + i32::from(b)).clamp(-32768, 32767) as i16)
        .collect();
    let stamp = jiff::Timestamp::from_second(started_at)
        .map(|time| time.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d_%H-%M-%S").to_string())
        .unwrap_or_else(|_| started_at.to_string());
    let base = format!("{stamp}_{}", file_safe(&name));
    let path = dir.join(format!("{base}.ogg"));
    let mine = dir.join(format!("{base}_voce.ogg"));
    let theirs = dir.join(format!("{base}_contato.ogg"));
    for (target, samples) in [(&path, &mix), (&mine, &me), (&theirs, &them)] {
        let bytes = encode_track(samples)?;
        std::fs::write(target, bytes).map_err(|error| error.to_string())?;
    }
    Ok(Recording {
        id: base,
        chat,
        name,
        started_at,
        seconds: (length / CALL_RATE as usize) as u32,
        path,
        mine,
        theirs,
        transcript: None,
        summary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verbose_reply_keeps_its_segments_in_order() {
        let reply = r#"{"text":"oi tudo bem","segments":[{"start":0.0,"text":" oi "},{"start":1.5,"text":"tudo bem"}]}"#;
        let segments = parse(reply).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "oi");
        assert_eq!(segments[1].start, 1.5);
    }

    #[test]
    fn a_plain_reply_is_one_segment() {
        let segments = parse(r#"{"text":"olá"}"#).unwrap();
        assert_eq!(segments, vec![Segment { start: 0.0, text: "olá".into() }]);
    }

    #[test]
    fn a_gap_in_the_call_becomes_silence_and_bursts_do_not() {
        let started = Instant::now() - Duration::from_secs(2);
        let mut track = Vec::new();
        place(&mut track, started, &[1; 960]);
        // Dois segundos depois do começo: o quadro vai para perto dos 32000.
        assert!(track.len() >= 31_000 && track.len() <= 32_100, "{}", track.len());
        let before = track.len();
        place(&mut track, started, &[1; 960]);
        assert_eq!(track.len(), before + 960, "uma rajada não abre buraco");
    }

    #[test]
    fn a_custom_service_takes_its_own_address_and_model() {
        let mut config = Config {
            provider: Provider::Custom,
            ..Config::default()
        };
        assert!(config.missing().is_some());
        assert_eq!(config.key().as_deref(), Some(""), "um servidor local sem chave");
        config.endpoint = "https://api.together.xyz/v1/".into();
        config.model = "openai/whisper-large-v3".into();
        assert!(config.missing().is_none());
        assert_eq!(config.endpoint(), "https://api.together.xyz/v1/audio/transcriptions");
        assert_eq!(config.model(), "openai/whisper-large-v3");
        let groq = Config::default();
        assert_eq!(groq.model(), "whisper-large-v3-turbo");
        assert!(groq.endpoint().starts_with("https://api.groq.com/"));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_safe("Ana / Loja: 01"), "Ana _ Loja_ 01");
        assert_eq!(file_safe("???"), "___");
        assert_eq!(file_safe(""), "contato");
    }

    #[test]
    fn the_clock_reads_minutes_and_seconds() {
        assert_eq!(clock(207.4), "03:27");
    }
}
