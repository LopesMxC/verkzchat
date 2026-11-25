//! Liga o VerkzChat ao Claude Desktop e ao Claude Code do próprio SDR, que
//! entra neles com a conta Claude dele. O VerkzChat só registra o conector
//! (`verkzchat mcp`) na configuração desses apps; o login é sempre no app
//! oficial da Anthropic.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};

/// Nome do conector nos apps do Claude.
pub const NAME: &str = "verkzchat";

pub fn executable() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("verkzchat"))
}

/// Onde o Claude Desktop guarda a configuração de conectores locais.
pub fn desktop_config_path() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Claude").join("claude_desktop_config.json"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/Application Support/Claude")
                .join("claude_desktop_config.json")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|dir| dir.join("Claude").join("claude_desktop_config.json"))
    }
}

/// A configuração com o conector do VerkzChat, mantendo todo o resto.
pub fn merged(existing: Option<&str>, exe: &Path) -> Result<Value, String> {
    let mut root = match existing.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => serde_json::from_str::<Value>(text)
            .map_err(|_| "O arquivo de configuração do Claude Desktop está com erro; não mexi nele.".to_owned())?,
        None => json!({}),
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| "O arquivo de configuração do Claude Desktop tem um formato inesperado; não mexi nele.".to_owned())?;
    let servers = object.entry("mcpServers").or_insert_with(|| Value::Object(Map::new()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| "\"mcpServers\" no Claude Desktop tem um formato inesperado; não mexi nele.".to_owned())?;
    servers.insert(
        NAME.into(),
        json!({ "command": exe.display().to_string(), "args": ["mcp"] }),
    );
    Ok(root)
}

/// Se o Claude Desktop já tem o conector apontando para este VerkzChat.
pub fn desktop_ready() -> bool {
    let Some(path) = desktop_config_path() else { return false };
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return false };
    value["mcpServers"][NAME]["command"].as_str() == Some(&executable().display().to_string())
}

/// Registra o conector no Claude Desktop. Guarda uma cópia do arquivo antigo.
pub fn connect_desktop() -> Result<PathBuf, String> {
    let path = desktop_config_path().ok_or("Não achei a pasta do Claude Desktop neste computador.")?;
    let existing = std::fs::read_to_string(&path).ok();
    let value = merged(existing.as_deref(), &executable())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("Não consegui criar {}: {error}", dir.display()))?;
    }
    if let Some(old) = &existing {
        let _ = std::fs::write(path.with_extension("json.antes-do-verkzchat"), old);
    }
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| format!("Não consegui gravar {}: {error}", path.display()))?;
    Ok(path)
}

/// Abre o Claude Desktop, onde o SDR entra com a conta dele.
pub fn open_desktop() -> Result<(), String> {
    let started = if cfg!(target_os = "windows") {
        let installed = std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("AnthropicClaude").join("claude.exe"))
            .filter(|path| path.is_file());
        match installed {
            Some(path) => Command::new(path).spawn().is_ok(),
            None => Command::new("cmd").args(["/C", "start", "", "claude://"]).spawn().is_ok(),
        }
    } else if cfg!(target_os = "macos") {
        Command::new("open").args(["-a", "Claude"]).spawn().is_ok()
    } else {
        Command::new("claude-desktop").spawn().is_ok()
    };
    if started {
        Ok(())
    } else {
        Err("Não achei o Claude Desktop. Instale em claude.ai/download e clique de novo.".into())
    }
}

/// O comando do Claude Code que registra o conector.
pub fn code_command() -> String {
    format!(
        "claude mcp add --transport stdio --scope user {NAME} -- \"{}\" mcp",
        executable().display()
    )
}

/// Registra o conector no Claude Code, se ele estiver instalado.
pub fn connect_code() -> Result<String, String> {
    let exe = executable().display().to_string();
    let run = |program: &str| {
        let mut command = if cfg!(target_os = "windows") {
            let mut command = Command::new("cmd");
            command.args(["/C", program]);
            command
        } else {
            Command::new(program)
        };
        // Tira um registro antigo (outro caminho) antes de adicionar.
        let _ = {
            let mut remove = if cfg!(target_os = "windows") {
                let mut remove = Command::new("cmd");
                remove.args(["/C", program]);
                remove
            } else {
                Command::new(program)
            };
            remove.args(["mcp", "remove", "--scope", "user", NAME]).output()
        };
        command
            .args(["mcp", "add", "--transport", "stdio", "--scope", "user", NAME, "--", &exe, "mcp"])
            .output()
    };
    match run("claude") {
        Ok(output) if output.status.success() => Ok("Conector registrado no Claude Code.".into()),
        Ok(output) => Err(format!(
            "O Claude Code recusou: {}",
            String::from_utf8_lossy(&output.stderr).trim().chars().take(300).collect::<String>()
        )),
        Err(_) => Err("Não achei o Claude Code (comando claude). Instale e clique de novo, ou rode o comando abaixo.".into()),
    }
}

// ─── Login do Claude Code ────────────────────────────────────────────────────
// Mesmo método do Lopes.Film: acha (ou instala) o Claude Code, confere com
// `claude auth status` e, se faltar, abre o login do navegador com
// `claude auth login`. O retorno volta para o próprio Claude Code, sem link
// claude:// que o Windows precise entregar ao app certo.

/// Esconde a janela preta do console no Windows.
fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// O Claude Code no PATH, na pasta do instalador oficial ou na cópia que o
/// Claude Desktop baixou, a mais nova primeiro.
pub fn claude_cli() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    if let Some(home) = &home {
        candidates.push(home.join(".local").join("bin").join(exe));
    }
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let mut versions: Vec<PathBuf> = std::fs::read_dir(roaming.join("Claude").join("claude-code"))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        versions.sort();
        candidates.extend(versions.into_iter().rev().map(|dir| dir.join(exe)));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(exe));
            if cfg!(windows) {
                candidates.push(dir.join("claude.cmd"));
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// O Claude Code no Windows roda comandos no Git Bash; diz onde ele está
/// quando o Git não está no PATH.
fn git_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    if let Some(path) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|name| std::env::var_os(name))
        .map(|root| PathBuf::from(root).join("Git").join("bin").join("bash.exe"))
        .find(|path| path.is_file())
}

fn claude_command(cli: &Path, args: &[&str]) -> Command {
    let mut command = if cli.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cmd")) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(cli);
        command
    } else {
        Command::new(cli)
    };
    command.args(args);
    if let Some(bash) = git_bash() {
        command.env("CLAUDE_CODE_GIT_BASH_PATH", bash);
    }
    quiet(&mut command);
    command
}

/// Se o Claude Code já tem uma conta logada.
pub fn login_status(cli: &Path) -> bool {
    claude_command(cli, &["auth", "status"]).output().is_ok_and(|output| output.status.success())
}

/// Instala o Claude Code com o instalador oficial, sem abrir terminal.
pub fn install_claude_code() -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://claude.ai/install.ps1 | iex"]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", "curl -fsSL https://claude.ai/install.sh | bash"]);
        command
    };
    match quiet(&mut command).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "A instalação do Claude Code falhou: {}",
            String::from_utf8_lossy(&output.stderr).trim().chars().take(300).collect::<String>()
        )),
        Err(error) => Err(format!("Não consegui iniciar a instalação do Claude Code: {error}")),
    }
}

/// Entra na conta Claude pelo Claude Code, esperando o navegador voltar.
/// Com `switch`, sai da conta atual antes, para escolher outra.
pub fn sign_in(switch: bool) -> Result<String, String> {
    let cli = match claude_cli() {
        Some(cli) => cli,
        None => {
            install_claude_code()?;
            claude_cli().ok_or("Instalei o Claude Code, mas não achei o programa. Abra o Claude Desktop uma vez e tente de novo.")?
        }
    };
    if switch {
        let _ = claude_command(&cli, &["auth", "logout"]).output();
    } else if login_status(&cli) {
        return Ok("O Claude já está logado neste computador.".into());
    }
    let status = claude_command(&cli, &["auth", "login"])
        .status()
        .map_err(|error| format!("Não consegui abrir o login do Claude: {error}"))?;
    if status.success() && login_status(&cli) {
        Ok("Login feito. Volte ao Claude.".into())
    } else {
        Err("O login não foi concluído. Clique de novo e termine no navegador.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_connector_joins_what_is_already_there() {
        let old = r#"{"theme":"dark","mcpServers":{"drive":{"command":"x"}}}"#;
        let value = merged(Some(old), Path::new("/apps/verkzchat")).unwrap();
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["mcpServers"]["drive"]["command"], "x");
        assert_eq!(value["mcpServers"]["verkzchat"]["command"], "/apps/verkzchat");
        assert_eq!(value["mcpServers"]["verkzchat"]["args"][0], "mcp");
    }

    #[test]
    fn an_empty_or_missing_file_gets_a_new_config() {
        for existing in [None, Some(""), Some("  \n")] {
            let value = merged(existing, Path::new("v")).unwrap();
            assert_eq!(value["mcpServers"]["verkzchat"]["command"], "v");
        }
    }

    #[test]
    fn a_broken_file_is_left_alone() {
        assert!(merged(Some("{ quebrado"), Path::new("v")).is_err());
        assert!(merged(Some("[1, 2]"), Path::new("v")).is_err());
        assert!(merged(Some(r#"{"mcpServers": 3}"#), Path::new("v")).is_err());
    }
}
