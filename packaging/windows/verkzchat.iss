; Instalador do VerkzChat para Windows (Inno Setup 6.3 ou mais novo).
; O compilar.bat chama assim:
;
;   iscc /DVersion=0.21.3 /DBinary=...\verkzchat.exe /DOutputDir=dist\windows packaging\windows\verkzchat.iss
;
; Instala na pasta de programas do usuário, sem precisar de administrador.

#ifndef Version
  #error Version must be defined on the ISCC command line
#endif
#ifndef Binary
  #error Binary must be defined on the ISCC command line
#endif
#ifndef OutputDir
  #error OutputDir must be defined on the ISCC command line
#endif

#define AppName "VerkzChat"
#define AppExeName "verkzchat.exe"

[Setup]
; Identidade própria do VerkzChat (não é a do ZapFast). Nunca mude: é por ela
; que o Windows reconhece uma atualização.
AppId={{24D69B44-D318-4123-83C3-90AC183A416A}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher=VERKZ
AppCopyright=© 2026 VERKZ
AppPublisherURL=https://verkzcompany.com.br
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=VerkzChat-{#Version}-instalador
SetupIconFile=verkzchat.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\{#AppExeName}

[Languages]
Name: "pt"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "desktopicon"; Description: "Criar atalho na área de trabalho"; GroupDescription: "Atalhos:"

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "{#AppExeName}"; Flags: ignoreversion
; Marca de "instalado pelo instalador": é por ela que a atualização automática
; sabe que pode rodar o instalador novo em silêncio por cima desta cópia.
Source: "verkzchat-installer.txt"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; Arquivos que versões antigas instalavam e esta não instala mais.
Type: files; Name: "{app}\README.md"
Type: files; Name: "{app}\LICENSE"
Type: files; Name: "{app}\THIRD-PARTY-NOTICES.md"

[UninstallDelete]
; Pastas temporárias da atualização automática.
Type: filesandordirs; Name: "{app}\.verkzchat-update-*"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "br.com.verkzcompany.verkzchat"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon; AppUserModelID: "br.com.verkzcompany.verkzchat"

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Abrir o {#AppName}"; Flags: nowait postinstall skipifsilent
