//! Atualização automática do VerkzChat pelo servidor da VERKZ.
//!
//! O app consulta `https://verkzsystem.tech/apps/verkzchat/latest.json` uma
//! vez por dia. Quando há versão nova, baixa o instalador do mesmo servidor,
//! confere o `checksums.txt` contra a assinatura Ed25519 da VERKZ
//! (`checksums.txt.sig`), confere o SHA-256 do instalador e só então o roda
//! em silêncio, reabrindo o app. Se a nova versão não abrir, volta sozinho
//! para a anterior.
//!
//! A chave pública fica em `assets/update-public-key.hex`; a privada, que
//! assina as versões, fica só no computador de quem publica (veja
//! `packaging/publicar`). Sem a assinatura certa nada é instalado, nem que
//! alguém invada o servidor.

pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, Request, ReqwestTransport, Response, Transport, UpdateConfig};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// Pasta onde ficam as versões publicadas do VerkzChat.
pub const FEED_URL: &str = "https://verkzsystem.tech/apps/verkzchat";

/// Página aberta pelo link "Baixar do site" quando a atualização automática
/// não pode ser feita nesta cópia.
pub const DOWNLOAD_PAGE: &str = "https://verkzsystem.tech/apps";

/// Identidade do VerkzChat para o atualizador.
pub const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["br.com.verkzcompany.VerkzChat"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    // Com o servidor próprio, o repositório não é lido, mas precisa ter o
    // formato dono/nome.
    ..UpdateConfig::new("verkz/verkzchat", "VerkzChat", "verkzchat", env!("CARGO_PKG_VERSION"))
};

/// Atualização automática ligada.
pub const SELF_UPDATE: bool = true;

/// A hora de agora, "14:05", para a área de atualizações dizer quando olhou.
pub fn now_label() -> String {
    jiff::Zoned::now().strftime("%H:%M").to_string()
}

/// O servidor de versões da VERKZ.
pub fn source() -> Source {
    Source::feed(FEED_URL).expect("FEED_URL é um endereço HTTPS válido")
}

/// Um atualizador no cliente HTTP do app, que respeita o proxy configurado.
pub fn updater() -> anyhow::Result<Updater> {
    let mut builder = reqwest::blocking::Client::builder();
    if let Some(proxy) = crate::proxy::reqwest_proxy() {
        builder = builder.proxy(proxy);
    }
    Ok(Updater::new(CONFIG, SiteTransport::new(ReqwestTransport::new(builder)?)).with_source(source()))
}

/// Maior pacote de atualização aceito (1 GiB).
const MAX_PACKAGE: u64 = 1 << 30;

/// O painel do site publica o `latest.json` no formato
/// `{ "version", "pub_date", "platforms": { "windows-x86_64": { "url", "signature" } } }`,
/// e o atualizador lê o formato de listagem de releases (`tag_name`, `assets`,
/// `checksums.txt` e `checksums.txt.sig`). Este transporte traduz um no outro:
///
/// - baixa o pacote novo uma vez, calcula o SHA-256 e guarda o arquivo (o
///   download pedido depois pelo atualizador sai desse arquivo, sem baixar de
///   novo);
/// - monta o `checksums.txt` (`<sha256>  <nome>`) e entrega como
///   `checksums.txt.sig` a assinatura que o painel guarda.
///
/// Quem confere a assinatura continua sendo o atualizador, com a chave
/// pública embutida: pacote adulterado ou assinatura de outra chave não passa.
/// Um `latest.json` que já vem no formato de listagem passa direto.
struct SiteTransport {
    inner: ReqwestTransport,
    state: Mutex<SiteState>,
}

#[derive(Default)]
struct SiteState {
    /// Arquivos que o transporte serve no lugar do servidor, por endereço.
    served: HashMap<String, Served>,
}

enum Served {
    Bytes(Vec<u8>),
    File(PathBuf),
}

impl SiteTransport {
    fn new(inner: ReqwestTransport) -> Self {
        Self { inner, state: Mutex::new(SiteState::default()) }
    }

    fn latest_url() -> String {
        format!("{FEED_URL}/latest.json")
    }

    /// GET que segue redirecionamentos dentro do próprio site.
    fn fetch(&self, request: &Request<'_>) -> anyhow::Result<Response> {
        let base = reqwest::Url::parse(FEED_URL)?;
        let mut current = reqwest::Url::parse(request.url)?;
        for _ in 0..=5 {
            anyhow::ensure!(current.origin() == base.origin(), "Endereço fora do site de atualização");
            let response = self.inner.get(&Request { url: current.as_str(), ..*request })?;
            match response.status {
                300..=399 => {
                    let location = response
                        .location
                        .ok_or_else(|| anyhow::anyhow!("Redirecionamento sem destino"))?;
                    current = current.join(&location)?;
                }
                _ => return Ok(response),
            }
        }
        anyhow::bail!("Redirecionamentos demais")
    }

    /// Baixa `url` para um arquivo, devolvendo caminho, tamanho e SHA-256.
    fn download(&self, request: &Request<'_>, url: &str) -> anyhow::Result<(PathBuf, u64, String)> {
        use sha2::{Digest, Sha256};
        let response = self.fetch(&Request { url, ..*request })?;
        anyhow::ensure!(
            (200..300).contains(&response.status),
            "O servidor respondeu {} ao baixar a atualização",
            response.status
        );
        let dir = std::env::temp_dir().join("verkzchat-update-cache");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("pacote");
        let mut file = std::fs::File::create(&path)?;
        let mut body = response.body.take(MAX_PACKAGE + 1);
        let (mut hasher, mut size) = (Sha256::new(), 0u64);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = body.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])?;
            size += read as u64;
        }
        anyhow::ensure!(size > 0 && size <= MAX_PACKAGE, "Tamanho de atualização inválido");
        let sha: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        Ok((path, size, sha))
    }

    /// Traduz o `latest.json` do painel para o formato do atualizador.
    fn translate(&self, request: &Request<'_>, site: &SiteRelease) -> anyhow::Result<Vec<u8>> {
        let tag = format!("v{}", site.version);
        let mut listing = serde_json::json!({
            "tag_name": tag,
            "html_url": DOWNLOAD_PAGE,
            "draft": false,
            "prerelease": false,
            "published_at": site.pub_date,
            "assets": [],
        });
        let newer = parse_version(&site.version) > parse_version(env!("CARGO_PKG_VERSION"));
        if let (true, Some(entry)) = (newer, site.entry()) {
            let signature = decode_signature(&entry.signature)?;
            // O atualizador só aceita o nome exato do contrato (o mesmo que a
            // ferramenta de publicação assina), seja qual for o nome do arquivo
            // no site.
            let name = if cfg!(target_os = "windows") {
                format!("{SLUG}-v{}-x86_64-pc-windows-msvc-setup.exe", site.version)
            } else {
                format!("{SLUG}-{}-{}.AppImage", site.version, std::env::consts::ARCH)
            };
            let package_key = entry.url.clone();
            let known = self.state.lock().unwrap().served.contains_key(&package_key);
            let (size, sha) = if known {
                // Já baixado nesta versão: o SHA-256 vem do checksums guardado.
                let state = self.state.lock().unwrap();
                let Some(Served::Bytes(text)) =
                    state.served.get(&format!("{FEED_URL}/{tag}/checksums.txt"))
                else {
                    anyhow::bail!("Cache de atualização incompleto");
                };
                let text = String::from_utf8_lossy(text).into_owned();
                let sha = text.split_whitespace().next().unwrap_or_default().to_owned();
                let Some(Served::File(path)) = state.served.get(&package_key) else {
                    anyhow::bail!("Cache de atualização incompleto");
                };
                (std::fs::metadata(path)?.len(), sha)
            } else {
                let (path, size, sha) = self.download(request, &entry.url)?;
                self.state.lock().unwrap().served.insert(package_key.clone(), Served::File(path));
                (size, sha)
            };
            let checksums = format!("{sha}  {name}\n");
            check_signature(&checksums, &signature, &sha)?;
            let base = format!("{FEED_URL}/{tag}");
            {
                let mut state = self.state.lock().unwrap();
                state.served.insert(format!("{base}/checksums.txt"), Served::Bytes(checksums.clone().into_bytes()));
                state.served.insert(format!("{base}/checksums.txt.sig"), Served::Bytes(signature));
            }
            listing["assets"] = serde_json::json!([
                { "name": name, "browser_download_url": entry.url, "size": size },
                { "name": "checksums.txt", "browser_download_url": format!("{base}/checksums.txt"), "size": checksums.len() },
                { "name": "checksums.txt.sig", "browser_download_url": format!("{base}/checksums.txt.sig"), "size": 64 },
            ]);
        }
        Ok(serde_json::to_vec(&listing)?)
    }
}

impl Transport for SiteTransport {
    fn get(&self, request: &Request<'_>) -> anyhow::Result<Response> {
        if let Some(served) = self.state.lock().unwrap().served.get(request.url) {
            return Ok(match served {
                Served::Bytes(bytes) => Response::ok(Cursor::new(bytes.clone())),
                Served::File(path) => Response::ok(std::fs::File::open(path)?),
            });
        }
        if request.url != Self::latest_url() {
            return self.inner.get(request);
        }
        let response = self.inner.get(request)?;
        if !(200..300).contains(&response.status) {
            return Ok(response);
        }
        let mut body = Vec::new();
        response.body.take(1024 * 1024).read_to_end(&mut body)?;
        // Formato de listagem de releases: passa sem tradução.
        let value: serde_json::Value = serde_json::from_slice(&body)?;
        if value.get("tag_name").is_some() {
            return Ok(Response::ok(Cursor::new(body)));
        }
        let site: SiteRelease = serde_json::from_value(value)
            .map_err(|error| anyhow::anyhow!("latest.json do site em formato desconhecido: {error}"))?;
        Ok(Response::ok(Cursor::new(self.translate(request, &site)?)))
    }
}

/// O `latest.json` que o painel do site publica.
#[derive(serde::Deserialize)]
struct SiteRelease {
    version: String,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    platforms: HashMap<String, SitePackage>,
}

#[derive(serde::Deserialize)]
struct SitePackage {
    url: String,
    signature: String,
}

impl SiteRelease {
    /// O pacote para este sistema: `windows-x86_64`, `linux-x86_64`.
    fn entry(&self) -> Option<&SitePackage> {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            return None;
        };
        let key = format!("{os}-{}", std::env::consts::ARCH);
        self.platforms
            .iter()
            .find(|(name, _)| name.as_str() == key)
            .or_else(|| self.platforms.iter().find(|(name, _)| name.starts_with(&key)))
            .map(|(_, package)| package)
    }
}

fn parse_version(version: &str) -> (u64, u64, u64) {
    let core = version.trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
    let mut parts = core.split('.').map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

const SLUG: &str = "verkzchat";

/// Confere a assinatura do painel com a chave embutida, para dizer o motivo
/// quando não bate (a mensagem do atualizador é genérica).
fn check_signature(checksums: &str, signature: &[u8], sha: &str) -> anyhow::Result<()> {
    use ring::signature::{ED25519, UnparsedPublicKey};
    let Some(key) = CONFIG.publisher_key else { return Ok(()) };
    let key = key.trim();
    let key: Vec<u8> = (0..key.len().min(64))
        .step_by(2)
        .map(|at| u8::from_str_radix(&key[at..at + 2], 16))
        .collect::<Result<_, _>>()?;
    if UnparsedPublicKey::new(&ED25519, &key).verify(checksums.as_bytes(), signature).is_err() {
        anyhow::bail!(
            "A assinatura do site não confere com o arquivo hospedado (SHA-256 {sha}). \
             Reassine: o instalador no site precisa ser o mesmo arquivo que gerou o .sig.txt colado no painel."
        );
    }
    Ok(())
}

/// A assinatura Ed25519 (64 bytes) como o painel a guarda: hexadecimal ou base64.
fn decode_signature(text: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine as _;
    let text = text.trim();
    let bytes = if text.len() == 128 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..128)
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16))
            .collect::<Result<Vec<u8>, _>>()?
    } else {
        base64::engine::general_purpose::STANDARD.decode(text)?
    };
    anyhow::ensure!(bytes.len() == 64, "A assinatura da atualização é inválida");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(CONFIG.slug, "verkzchat");
    }

    #[test]
    fn the_site_listing_is_understood() {
        let json = r#"{"version":"0.25.0","notes":"","pub_date":"2026-11-24T11:27:07Z","platforms":{"windows-x86_64":{"signature":"ab","url":"https://verkzsystem.tech/apps/verkzchat/download/x.exe"}}}"#;
        let site: SiteRelease = serde_json::from_str(json).unwrap();
        assert_eq!(site.version, "0.25.0");
        assert!(parse_version("0.25.0") > parse_version("0.24.0"));
        assert!(parse_version("0.24.0") < parse_version("0.100.0"));
        let hex = "ab".repeat(64);
        assert_eq!(decode_signature(&hex).unwrap().len(), 64);
        assert!(decode_signature("ab").is_err());
    }

    #[test]
    fn updates_come_from_the_verkz_server() {
        assert!(SELF_UPDATE);
        assert!(!source().is_github());
        assert!(updater().is_ok());
    }
}
