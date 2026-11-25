//! Publica uma versão nova do VerkzChat para a atualização automática.
//!
//!   publicar gerar-chave <assets\update-public-key.hex>
//!       Cria a chave de assinatura da VERKZ (uma vez só). A privada fica em
//!       %USERPROFILE%\.verkzchat-assinatura\chave-privada.hex; a pública é
//!       gravada no arquivo indicado, que vai embutido no VerkzChat.
//!
//!   publicar versao <versao> <instalador.exe> <chave-publica.hex> <pasta-saida> [AppImage|-]
//!       Monta <pasta-saida>\verkzchat\ pronta para subir em
//!       https://verkzsystem.tech/apps/verkzchat/ :
//!         latest.json
//!         v<versao>\verkzchat-v<versao>-x86_64-pc-windows-msvc-setup.exe
//!         v<versao>\verkzchat-<versao>-x86_64.AppImage   (se informado)
//!         v<versao>\checksums.txt
//!         v<versao>\checksums.txt.sig
//!         v<versao>\<pacote>.sig e <pacote>.sig.txt   (assinatura de cada pacote)
//!
//! A variável VERKZCHAT_CHAVE pode apontar para outro arquivo de chave privada.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

const BASE_URL: &str = "https://verkzsystem.tech/apps/verkzchat";
const PAGINA: &str = "https://verkzsystem.tech/apps";
const ALVO: &str = "x86_64-pc-windows-msvc";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let resultado = match args.first().map(String::as_str) {
        Some("gerar-chave") if args.len() == 2 => gerar_chave(Path::new(&args[1])),
        Some("versao") if args.len() == 5 || args.len() == 6 => publicar(
            &args[1],
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            args.get(5).filter(|a| a.as_str() != "-").map(Path::new),
        ),
        _ => Err(
            "Uso:\n  publicar gerar-chave <chave-publica.hex>\n  publicar versao <versao> <instalador.exe> <chave-publica.hex> <pasta-saida> [AppImage|-]"
                .into(),
        ),
    };
    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        Err(erro) => {
            eprintln!("[ERRO] {erro}");
            ExitCode::FAILURE
        }
    }
}

fn arquivo_chave() -> Result<PathBuf, String> {
    if let Some(caminho) = std::env::var_os("VERKZCHAT_CHAVE") {
        return Ok(PathBuf::from(caminho));
    }
    let casa = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or("Não achei a pasta do usuário")?;
    Ok(PathBuf::from(casa)
        .join(".verkzchat-assinatura")
        .join("chave-privada.hex"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(texto: &str) -> Result<Vec<u8>, String> {
    let texto = texto.trim();
    if texto.len() % 2 != 0 {
        return Err("Chave em formato inválido".into());
    }
    (0..texto.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&texto[i..i + 2], 16).map_err(|_| "Chave em formato inválido".to_owned()))
        .collect()
}

fn ler_chave() -> Result<Ed25519KeyPair, String> {
    let caminho = arquivo_chave()?;
    let texto = fs::read_to_string(&caminho).map_err(|_| {
        format!(
            "Não achei a chave privada em {}. Rode o compilar.bat para criá-la ou copie o backup para lá.",
            caminho.display()
        )
    })?;
    Ed25519KeyPair::from_pkcs8(&unhex(&texto)?)
        .map_err(|_| format!("A chave em {} está corrompida", caminho.display()))
}

fn gerar_chave(publica: &Path) -> Result<(), String> {
    let caminho = arquivo_chave()?;
    if caminho.exists() {
        let par = ler_chave()?;
        fs::write(publica, hex(par.public_key().as_ref()) + "\n").map_err(|e| e.to_string())?;
        println!("Chave já existia em {}; chave pública conferida.", caminho.display());
        return Ok(());
    }
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "Não consegui gerar a chave")?;
    let par = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "Chave gerada inválida")?;
    fs::create_dir_all(caminho.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&caminho, hex(pkcs8.as_ref()) + "\n").map_err(|e| e.to_string())?;
    fs::write(publica, hex(par.public_key().as_ref()) + "\n").map_err(|e| e.to_string())?;
    println!("Chave de assinatura criada em {}", caminho.display());
    println!("FAÇA BACKUP DESSE ARQUIVO. Sem ele, as próximas versões não serão aceitas pelos VerkzChat instalados.");
    Ok(())
}

fn versao_valida(versao: &str) -> bool {
    let nucleo = versao.split(['-', '+']).next().unwrap_or("");
    let partes: Vec<_> = nucleo.split('.').collect();
    partes.len() == 3 && partes.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn publicar(
    versao: &str,
    instalador: &Path,
    publica: &Path,
    saida: &Path,
    appimage: Option<&Path>,
) -> Result<(), String> {
    let versao = versao.trim().trim_start_matches('v');
    if !versao_valida(versao) {
        return Err(format!("Versão inválida: {versao} (use 1.2.3)"));
    }
    let par = ler_chave()?;
    let embutida = fs::read_to_string(publica).map_err(|e| format!("{}: {e}", publica.display()))?;
    if embutida.trim() != hex(par.public_key().as_ref()) {
        return Err(format!(
            "A chave pública em {} não é a desta chave privada. O VerkzChat compilado não aceitaria esta versão.",
            publica.display()
        ));
    }
    let bytes = fs::read(instalador).map_err(|e| format!("{}: {e}", instalador.display()))?;

    let pasta_app = saida.join("verkzchat");
    let tag = format!("v{versao}");
    let pasta = pasta_app.join(&tag);
    fs::create_dir_all(&pasta).map_err(|e| e.to_string())?;

    // Cada arquivo publicado: nome, conteúdo. O AppImage tem o nome que o
    // atualizador procura: <slug>-<versão>-<arquitetura>.AppImage (sem o "v").
    let nome = format!("verkzchat-v{versao}-{ALVO}-setup.exe");
    let mut arquivos: Vec<(String, Vec<u8>)> = vec![(nome, bytes)];
    if let Some(caminho) = appimage {
        let dados = fs::read(caminho).map_err(|e| format!("{}: {e}", caminho.display()))?;
        arquivos.push((format!("verkzchat-{versao}-x86_64.AppImage"), dados));
    }
    let mut checksums = String::new();
    for (nome, dados) in &arquivos {
        fs::write(pasta.join(nome), dados).map_err(|e| e.to_string())?;
        let linha = format!("{}  {nome}\n", hex(digest(&SHA256, dados).as_ref()));
        // A assinatura do pacote sozinho (o que o painel do site pede junto
        // com o arquivo): Ed25519 sobre a linha "<sha256>  <nome>" do
        // checksums.txt. `.sig` é o arquivo (64 bytes); `.sig.txt` é o mesmo
        // conteúdo em hexadecimal, para colar no campo de texto.
        let individual = par.sign(linha.as_bytes());
        fs::write(pasta.join(format!("{nome}.sig")), individual.as_ref())
            .map_err(|e| e.to_string())?;
        fs::write(pasta.join(format!("{nome}.sig.txt")), hex(individual.as_ref()) + "\n")
            .map_err(|e| e.to_string())?;
        checksums += &linha;
    }
    fs::write(pasta.join("checksums.txt"), &checksums).map_err(|e| e.to_string())?;
    let assinatura = par.sign(checksums.as_bytes());
    fs::write(pasta.join("checksums.txt.sig"), assinatura.as_ref()).map_err(|e| e.to_string())?;

    let asset = |nome: &str, tamanho: usize| {
        format!(
            "    {{ \"name\": \"{nome}\", \"browser_download_url\": \"{BASE_URL}/{tag}/{nome}\", \"size\": {tamanho} }}"
        )
    };
    let mut assets: Vec<String> = arquivos.iter().map(|(n, d)| asset(n, d.len())).collect();
    assets.push(asset("checksums.txt", checksums.len()));
    assets.push(asset("checksums.txt.sig", 64));
    let latest = format!(
        "{{\n  \"tag_name\": \"{tag}\",\n  \"html_url\": \"{PAGINA}\",\n  \"draft\": false,\n  \"prerelease\": false,\n  \"assets\": [\n{}\n  ]\n}}\n",
        assets.join(",\n"),
    );
    fs::write(pasta_app.join("latest.json"), latest).map_err(|e| e.to_string())?;

    println!("Pronto: {}", pasta_app.display());
    println!("Suba a pasta \"verkzchat\" inteira para {BASE_URL}/");
    println!("(primeiro a pasta {tag}, por último o latest.json)");
    Ok(())
}
