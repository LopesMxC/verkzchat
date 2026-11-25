# VerkzChat

VerkzChat é o WhatsApp de desktop da VERKZ com o Claude como assistente de SDR.
É um fork do [ZapFast](https://github.com/crmne/zapfast) (licença MIT, mantida em `LICENSE`).

## O que o Claude faz

O painel do Claude fica à direita da conversa. Abra pelo botão com a estrela na barra lateral ou com **Ctrl+Shift+K**.

| Aba | O que tem |
|---|---|
| **Análise** | Temperatura do lead (0 a 100: frio, morno, quente) com o motivo, resumo, etapa da negociação, sinais de compra, objeções com o trecho do lead e como contornar, próxima mensagem sugerida e follow-up sugerido. A análise vem do Claude (pedido no Claude Desktop / Claude Code, ou automática com a chave de API). |
| **Chat** | O registro de tudo o que o Claude fez no WhatsApp (pedidos feitos no Claude Desktop / Claude Code). Com a chave de API opcional, também dá para conversar com ele direto aqui. |
| **Follow-ups** | Tarefas agendadas. Na hora marcada o Claude abre a conversa, lê, ajusta o rascunho ao contexto e digita. Pode pular se o lead já respondeu. Inclui regras automáticas por temperatura. |
| **Relatório** | Conversas ativas, leads quentes, follow-ups enviados, taxa de resposta, temperatura da carteira, objeções mais frequentes e leads para priorizar hoje. O botão "Gerar relatório completo" pede ao Claude um relatório em Markdown e salva na pasta de downloads. |
| **Config** | Chave da API, modelo (Opus 5.5 por padrão) e o perfil de cada SDR: nome, tom de voz, o que vende, como escrever, critérios de temperatura, como contorna objeções, permissões e velocidade do cursor. Cada SDR pode ter o seu perfil. |

A temperatura também aparece como um selo colorido no avatar de cada conversa na lista.

## O cursor do Claude

Tudo o que o Claude faz no WhatsApp aparece na tela como um cursor coral com um rótulo ("Claude · abrindo Marina", "Claude · digitando 3/5"):

- abrir conversas clicando na lista;
- ler a conversa (o cursor "rola" a área de mensagens);
- ir ao campo e digitar a mensagem letra por letra;
- ir ao botão de enviar e enviar.

Enquanto ele age, a conversa ganha uma borda coral e uma faixa "Claude está no controle", com **Pausar** e **Parar**.

**Envio:** por padrão o Claude nunca envia sozinho. Ele digita e mostra "O Claude quer enviar esta mensagem" com **Aprovar envio** e **Cancelar**. Só envia sem perguntar se o perfil tiver "Enviar sem minha aprovação" ligado.

**Retomar o controle:** aperte **Esc** ou clique em qualquer lugar fora do painel do Claude. Ele para na hora.

## Conectar o Claude (sem chave de API)

O Claude usa a conta Claude de vocês, pelo **Claude Desktop** ou pelo **Claude Code**. O VerkzChat funciona como um conector (MCP): o Claude logado na conta chama as ferramentas do VerkzChat, e cada ação aparece na tela com o cursor.

1. Instale o Claude Desktop (claude.ai/download).
2. No VerkzChat, abra o painel do Claude → aba **Config** → **Conectar ao Claude Desktop**. O VerkzChat se registra no Claude Desktop (guardando uma cópia da configuração antiga em `claude_desktop_config.json.antes-do-verkzchat`) e abre o app. Se ele já estava aberto, feche de vez (inclusive na bandeja) e abra de novo.
3. No Claude Desktop, entre com a **sua conta Claude** (cada SDR com a dele).
4. Peça, por exemplo: "analise o lead aberto no VerkzChat". Na primeira vez o Claude Desktop pede permissão para usar as ferramentas do VerkzChat.

Para usar pelo **Claude Code**: botão **Conectar ao Claude Code** (precisa do comando `claude` instalado; o login é com `/login` no terminal).

O VerkzChat precisa estar aberto. O conector é o próprio programa rodando como `verkzchat mcp`; ele fala só com o VerkzChat da mesma máquina, por um canal local protegido por senha.

**Ferramentas que o Claude recebe:** `get_sdr_profile`, `list_chats`, `open_chat`, `read_chat`, `type_message`, `send_message`, `save_lead_analysis`, `get_lead_analysis`, `schedule_follow_up`, `list_follow_ups`, `list_due_follow_ups`, `complete_follow_up`.

**Follow-ups no horário:** quem executa é o Claude Desktop / Claude Code. Crie lá uma tarefa agendada (por exemplo, de hora em hora em horário comercial) com o pedido da aba Follow-ups: "Execute os follow-ups vencidos do VerkzChat…".

**Opcional, chave de API:** em Config, uma chave de console.anthropic.com faz o Claude trabalhar também sozinho dentro do app (análise automática a cada mensagem e follow-ups no horário mesmo sem o Claude Desktop aberto). É cobrada à parte, por uso.

Os dados do assistente (perfis, análises, follow-ups e a chave opcional) ficam em `claude.json`, na pasta de configuração do VerkzChat.

## Compilar

No Windows, dê dois cliques em **`compilar.bat`** e escolha:

| Opção | Como gera | Resultado |
|---|---|---|
| Windows | No próprio PC. Instala sozinho o que faltar (Rust, Build Tools do Visual Studio, CMake, Perl, NASM, Inno Setup) pelo winget. | `dist\windows\VerkzChat.exe` e `VerkzChat-<versão>-instalador.exe` |
| Linux | Pelo WSL (Ubuntu dentro do Windows). Se não houver WSL, oferece instalar (pede administrador e reiniciar). | `dist\linux\VerkzChat-<versão>-linux-x86_64.tar.gz` |
| Mac | Na nuvem do GitHub, numa máquina macOS (o app de Mac só compila num Mac). Pede login no GitHub e cria um repositório **privado** `verkzchat` se ainda não houver. | `dist\mac\VerkzChat-<versão>-mac.zip` |

A primeira compilação demora (de 15 a 40 minutos por sistema); as seguintes são bem mais rápidas.

O app de Mac sai sem a assinatura paga da Apple: na primeira vez, clique com o botão direito no VerkzChat → Abrir. As compilações de Mac no GitHub gastam minutos do plano do GitHub (em repositório privado, o minuto de macOS conta 10 vezes).

Num Linux direto (Ubuntu/Debian): `bash packaging/compilar-linux.sh`.

Manualmente, no Ubuntu/Debian:

```sh
sudo apt install build-essential cmake perl clang libclang-dev libasound2-dev libpipewire-0.3-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev
cargo build --release
./target/release/verkzchat
```

Para ver o painel com dados de exemplo, sem conectar o WhatsApp:

```sh
cargo run --features demo -- --demo-page chat,claude          # Análise
cargo run --features demo -- --demo-page chat,claude-control  # cursor digitando
cargo run --features demo -- --demo-page chat,claude-approve  # aprovação de envio
cargo run --features demo -- --demo-page chat,claude-followups
cargo run --features demo -- --demo-page chat,claude-report
cargo run --features demo -- --demo-page chat,claude-config
```

## Diferenças em relação ao ZapFast

- Nome, ícones e pasta de dados trocados para VerkzChat. É uma instalação separada: precisa vincular o celular de novo.
- Ícone com o símbolo oficial da VERKZ (`packaging/icons/verkzchat.svg`, gerado de `assets/verkz-symbol.svg`).
- **Sem atualização automática:** as opções e avisos de atualização saíram da interface e a verificação está travada (`src/updates.rs`, `SELF_UPDATE`). As releases do ZapFast substituiriam o VerkzChat pela versão sem o Claude.
- Identificador do app: `br.com.verkzcompany.VerkzChat`; editor nos instaladores: VERKZ.
- Arquivos de distribuição do ZapFast (Homebrew, Arch, RPM, Flatpak, Nix, winget, GitHub Actions, site) removidos.
- Código novo: `src/claude/` (análise, agente, cursor, follow-ups, relatório, armazenamento) e `src/ui/claude.rs` (painel e cursor).

## Riscos

O VerkzChat se conecta como um aparelho vinculado, sem a API oficial do WhatsApp. Automatizar mensagens aumenta o risco de o WhatsApp restringir o número: evite volume alto, envios em massa e mensagens repetidas para muitos contatos.
