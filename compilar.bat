@echo off
rem ==========================================================================
rem  Compila o VerkzChat para Windows, Linux e Mac.
rem
rem    compilar.bat            mostra o menu
rem    compilar.bat windows    so Windows
rem    compilar.bat linux      so Linux (pelo WSL)
rem    compilar.bat mac        so Mac (pelo GitHub Actions)
rem    compilar.bat todos      os tres
rem
rem  Resultado na pasta dist\ (windows, linux com .AppImage/.deb/.rpm, mac).
rem ==========================================================================
setlocal EnableExtensions EnableDelayedExpansion
cd /d "%~dp0"
title Compilar VerkzChat

set "VERSAO="
for /f "usebackq tokens=3" %%v in (`findstr /b /c:"version = " Cargo.toml`) do if not defined VERSAO set "VERSAO=%%~v"
if not defined VERSAO set "VERSAO=0.0.0"

set "ALVO=%~1"
if "%ALVO%"=="" goto menu
goto escolher

:menu
echo.
echo  ===============================================
echo   VerkzChat %VERSAO% - compilar
echo  ===============================================
echo   1. Windows
echo   2. Linux  (pelo WSL, o Linux do Windows)
echo   3. Mac    (na nuvem do GitHub)
echo   4. Todos
echo.
choice /c 1234 /n /m "Escolha [1-4]: "
if errorlevel 4 (set "ALVO=todos") else if errorlevel 3 (set "ALVO=mac") else if errorlevel 2 (set "ALVO=linux") else set "ALVO=windows"

:escolher
set "FALHOU="
if /i "%ALVO%"=="windows" call :windows
if /i "%ALVO%"=="linux" call :linux
if /i "%ALVO%"=="mac" call :mac
if /i "%ALVO%"=="todos" (
    call :windows
    call :linux
    call :mac
)
rem A pasta de atualizacao reune o instalador do Windows e o AppImage do Linux
rem (a atualizacao automatica do Linux so funciona no AppImage), por isso so
rem e montada depois que os dois alvos terminaram.
if defined INSTALADOR_OK call :publicar
echo.
if defined FALHOU (
    echo  Terminou com problemas em: %FALHOU%
    echo  Leia as mensagens acima.
) else (
    echo  Tudo pronto. Os arquivos estao na pasta dist\
)
if exist dist start "" explorer "%CD%\dist"
echo.
pause
exit /b 0

rem --------------------------------------------------------------------------
:windows
echo.
echo  == Windows ==
call :preparar_windows
if errorlevel 1 (set "FALHOU=!FALHOU! Windows" & exit /b 1)
call :chave_atualizacao
if errorlevel 1 (set "FALHOU=!FALHOU! Windows" & exit /b 1)
rem O rustc usa de 2 a 3 GB por crate em release. Com pouca RAM livre, varios ao
rem mesmo tempo estouram a memoria ("memory allocation ... failed") e o erro
rem se espalha como "can't find crate" em outros crates. Limita pelo que sobra.
rem LTO ligado (app menor e mais otimizado, compilacao mais lenta). Para acelerar, defina CARGO_PROFILE_RELEASE_LTO=off antes.
rem O nmake do OpenSSL trava com as variaveis de jobserver que o Cargo repassa.
set "MAKEFLAGS="
set "CARGO_MAKEFLAGS="
if not defined CARGO_TARGET_DIR set "CARGO_TARGET_DIR=C:\verkz-target"
if not defined CARGO_PROFILE_RELEASE_LTO set "CARGO_PROFILE_RELEASE_LTO=thin"
if not defined CARGO_PROFILE_RELEASE_CODEGEN_UNITS set "CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16"
if not defined CARGO_BUILD_JOBS (
    for /f %%j in ('powershell -NoProfile -Command "$r=[math]::Floor((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory/1MB/2.5); [math]::Max(2,[math]::Min($r,[Environment]::ProcessorCount))"') do set "CARGO_BUILD_JOBS=%%j"
)
echo  Compilando com !CARGO_BUILD_JOBS! tarefas em paralelo ^(a primeira vez demora de 15 a 40 minutos^)...
cargo build --release --locked
if errorlevel 1 (
    echo  [ERRO] A compilacao para Windows falhou.
    set "FALHOU=!FALHOU! Windows"
    exit /b 1
)
if not exist dist\windows mkdir dist\windows
copy /y "%CARGO_TARGET_DIR%\release\verkzchat.exe" "dist\windows\VerkzChat.exe" >nul
echo  Pronto: dist\windows\VerkzChat.exe
call :achar_inno
if defined ISCC (
    echo  Gerando o instalador...
    "%ISCC%" /Q /DVersion=%VERSAO% "/DBinary=%CARGO_TARGET_DIR%\release\verkzchat.exe" "/DOutputDir=%CD%\dist\windows" packaging\windows\verkzchat.iss
    if errorlevel 1 (
        echo  [AVISO] O instalador falhou; o VerkzChat.exe continua pronto.
    ) else (
        echo  Pronto: dist\windows\VerkzChat-%VERSAO%-instalador.exe
        set "INSTALADOR_OK=1"
    )
) else (
    echo  [AVISO] Inno Setup nao encontrado: gerei so o VerkzChat.exe, sem instalador.
)
exit /b 0

:preparar_windows
where winget >nul 2>&1
if errorlevel 1 (
    echo  [ERRO] O winget nao esta disponivel. Instale o "Instalador de Aplicativo" pela Microsoft Store e rode de novo.
    exit /b 1
)
rem Rust
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>&1
if errorlevel 1 (
    echo  Instalando o Rust...
    winget install -e --id Rustlang.Rustup --accept-source-agreements --accept-package-agreements
    set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
)
where cargo >nul 2>&1
if errorlevel 1 (
    echo  [ERRO] O Rust nao ficou disponivel. Feche esta janela e rode o compilar.bat de novo.
    exit /b 1
)
rem Compilador C++ da Microsoft
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "TEMVC="
if exist "%VSWHERE%" for /f "usebackq delims=" %%i in (`call "%VSWHERE%" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "TEMVC=%%i"
if not defined TEMVC (
    echo  Instalando o Visual Studio Build Tools com C++ ^(uns 5 GB, pode pedir permissao^)...
    winget install -e --id Microsoft.VisualStudio.2022.BuildTools --accept-source-agreements --accept-package-agreements --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
)
rem CMake, Perl e NASM (para a criptografia embutida do banco de mensagens)
where cmake >nul 2>&1 || if exist "%ProgramFiles%\CMake\bin\cmake.exe" set "PATH=%ProgramFiles%\CMake\bin;%PATH%"
where cmake >nul 2>&1
if errorlevel 1 (
    echo  Instalando o CMake...
    winget install -e --id Kitware.CMake --accept-source-agreements --accept-package-agreements
    set "PATH=%ProgramFiles%\CMake\bin;%PATH%"
)
rem O Perl do Git atrapalha a compilacao do OpenSSL: o Strawberry vai na frente.
if exist "C:\Strawberry\perl\bin\perl.exe" set "PATH=C:\Strawberry\perl\bin;%PATH%"
where perl >nul 2>&1
if errorlevel 1 (
    echo  Instalando o Perl...
    winget install -e --id StrawberryPerl.StrawberryPerl --accept-source-agreements --accept-package-agreements
    set "PATH=C:\Strawberry\perl\bin;%PATH%"
)
where nasm >nul 2>&1 || if exist "%ProgramFiles%\NASM\nasm.exe" set "PATH=%ProgramFiles%\NASM;%PATH%"
where nasm >nul 2>&1
if errorlevel 1 (
    echo  Instalando o NASM...
    winget install -e --id NASM.NASM --accept-source-agreements --accept-package-agreements
    set "PATH=%ProgramFiles%\NASM;%LOCALAPPDATA%\bin\NASM;%PATH%"
)
call :achar_inno
if not defined ISCC (
    echo  Instalando o Inno Setup ^(para gerar o instalador^)...
    winget install -e --id JRSoftware.InnoSetup --accept-source-agreements --accept-package-agreements
)
exit /b 0

:achar_inno
set "ISCC="
if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe"
if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
exit /b 0

rem --------------------------------------------------------------------------
rem  Atualizacao automatica (verkzsystem.tech/apps/verkzchat)
rem
rem  A chave privada fica em %USERPROFILE%\.verkzchat-assinatura\ e assina cada
rem  versao. A publica vai embutida no VerkzChat. Faca backup da privada: sem
rem  ela, os VerkzChat instalados recusam as proximas versoes.
:ferramenta_publicar
echo  Preparando a ferramenta de publicacao...
cargo build --release --quiet --manifest-path packaging\publicar\Cargo.toml
if errorlevel 1 (
    echo  [ERRO] Nao consegui compilar a ferramenta de publicacao.
    exit /b 1
)
set "PUBLICAR=%CD%\packaging\publicar\target\release\verkzchat-publicar.exe"
exit /b 0

:chave_atualizacao
call :ferramenta_publicar
if errorlevel 1 exit /b 1
"%PUBLICAR%" gerar-chave assets\update-public-key.hex
if errorlevel 1 (
    echo  [ERRO] Nao consegui preparar a chave de assinatura das atualizacoes.
    exit /b 1
)
exit /b 0

:publicar
if exist dist\windows\publicar rmdir /s /q dist\windows\publicar
set "APPIMAGE_ARQ=-"
if exist "dist\linux\verkzchat-%VERSAO%-x86_64.AppImage" set "APPIMAGE_ARQ=%CD%\dist\linux\verkzchat-%VERSAO%-x86_64.AppImage"
"%PUBLICAR%" versao %VERSAO% "%CD%\dist\windows\VerkzChat-%VERSAO%-instalador.exe" assets\update-public-key.hex "%CD%\dist\windows\publicar" "%APPIMAGE_ARQ%"
if errorlevel 1 (
    echo  [AVISO] Nao consegui montar a pasta de atualizacao.
    exit /b 0
)
echo  Assinaturas de cada pacote ^(.sig e .sig.txt^): dist\windows\publicar\verkzchat\v%VERSAO%\
echo  Para liberar a atualizacao: suba dist\windows\publicar\verkzchat\ para
echo  https://verkzsystem.tech/apps/verkzchat/ ^(a pasta v%VERSAO% primeiro, o latest.json por ultimo^).
exit /b 0

rem --------------------------------------------------------------------------
:linux
echo.
echo  == Linux ==
set "DISTRO=Ubuntu-22.04"
set "WSL_UTF8=1"
wsl.exe -l -q | findstr /i /x "!DISTRO!" >nul 2>&1
if errorlevel 1 (
    echo  O WSL ^(Linux dentro do Windows^) nao esta instalado.
    choice /c SN /m "  Instalar o Ubuntu 22.04 no WSL agora? Pode pedir administrador e reiniciar o PC"
    if errorlevel 2 (set "FALHOU=!FALHOU! Linux" & exit /b 1)
    powershell -NoProfile -Command "Start-Process wsl.exe -ArgumentList '--install -d Ubuntu-22.04' -Verb RunAs -Wait"
    echo  Reinicie o computador, abra o Ubuntu uma vez para criar o usuario e rode o compilar.bat de novo.
    set "FALHOU=!FALHOU! Linux"
    exit /b 1
)
set "WSLDIR="
for /f "usebackq delims=" %%p in (`wsl.exe -d !DISTRO! -e wslpath -a "%CD%"`) do set "WSLDIR=%%p"
if not defined WSLDIR (
    echo  [ERRO] Nao consegui achar esta pasta dentro do WSL.
    set "FALHOU=!FALHOU! Linux"
    exit /b 1
)
echo  Compilando dentro do WSL ^(pode pedir a senha do Linux para instalar dependencias^)...
wsl.exe -d !DISTRO! -e bash -lc "cd '%WSLDIR%' && tr -d '\r' < packaging/compilar-linux.sh > /tmp/verkzchat-linux.sh && VERKZCHAT_RAIZ='%WSLDIR%' bash /tmp/verkzchat-linux.sh"
if errorlevel 1 (
    echo  [ERRO] A compilacao para Linux falhou.
    set "FALHOU=!FALHOU! Linux"
    exit /b 1
)
exit /b 0

rem --------------------------------------------------------------------------
:mac
echo.
echo  == Mac ==
echo  O app de Mac so compila num Mac. Este passo manda o GitHub compilar
echo  numa maquina macOS deles e baixa o resultado.
if exist "%ProgramFiles%\GitHub CLI\gh.exe" set "PATH=%ProgramFiles%\GitHub CLI;%PATH%"
if exist "%ProgramFiles%\Git\cmd\git.exe" set "PATH=%ProgramFiles%\Git\cmd;%PATH%"
where git >nul 2>&1
if errorlevel 1 (
    echo  Instalando o Git...
    winget install -e --id Git.Git --accept-source-agreements --accept-package-agreements
    set "PATH=%ProgramFiles%\Git\cmd;%PATH%"
)
where gh >nul 2>&1
if errorlevel 1 (
    echo  Instalando o GitHub CLI...
    winget install -e --id GitHub.cli --accept-source-agreements --accept-package-agreements
    set "PATH=%ProgramFiles%\GitHub CLI;%PATH%"
)
where gh >nul 2>&1
if errorlevel 1 (
    echo  [ERRO] O GitHub CLI nao ficou disponivel. Feche esta janela e rode de novo.
    set "FALHOU=!FALHOU! Mac"
    exit /b 1
)
gh auth status >nul 2>&1
if errorlevel 1 (
    echo  Entre na sua conta do GitHub:
    gh auth login --web --git-protocol https
    if errorlevel 1 (set "FALHOU=!FALHOU! Mac" & exit /b 1)
)
rem O codigo precisa estar num repositorio do GitHub.
git rev-parse --is-inside-work-tree >nul 2>&1
if errorlevel 1 (
    git init -q -b main
    git add -A
    git -c user.name="VerkzChat" -c user.email="build@verkzcompany.com.br" commit -q -m "VerkzChat"
)
git remote get-url origin >nul 2>&1
if errorlevel 1 (
    echo  Este projeto ainda nao esta no GitHub.
    choice /c SN /m "  Criar um repositorio PRIVADO verkzchat na sua conta e enviar o codigo"
    if errorlevel 2 (set "FALHOU=!FALHOU! Mac" & exit /b 1)
    gh repo create verkzchat --private --source . --remote origin --push
    if errorlevel 1 (
        echo  [ERRO] Nao consegui criar o repositorio.
        set "FALHOU=!FALHOU! Mac"
        exit /b 1
    )
) else (
    git status --porcelain | findstr . >nul
    if not errorlevel 1 (
        echo  Ha alteracoes que ainda nao estao no GitHub; o GitHub compila o que esta la.
        choice /c SN /m "  Enviar as alteracoes agora"
        if not errorlevel 2 (
            git add -A
            git -c user.name="VerkzChat" -c user.email="build@verkzcompany.com.br" commit -q -m "Atualiza o VerkzChat"
        )
    )
    git push -q
)
set "RAMO="
for /f "usebackq delims=" %%b in (`git rev-parse --abbrev-ref HEAD`) do set "RAMO=%%b"
echo  Pedindo a compilacao ao GitHub...
gh workflow run compilar.yml --ref %RAMO% -f sistemas=mac
if errorlevel 1 (
    echo  [ERRO] O GitHub recusou. Confira se o arquivo .github\workflows\compilar.yml esta no repositorio.
    set "FALHOU=!FALHOU! Mac"
    exit /b 1
)
timeout /t 10 /nobreak >nul
set "EXECUCAO="
for /f "usebackq delims=" %%i in (`gh run list --workflow compilar.yml --limit 1 --json databaseId --jq ".[0].databaseId"`) do set "EXECUCAO=%%i"
if not defined EXECUCAO (
    echo  [ERRO] Nao achei a compilacao no GitHub.
    set "FALHOU=!FALHOU! Mac"
    exit /b 1
)
echo.
echo  Compilando no GitHub ^(execucao %EXECUCAO%^).
echo  A primeira vez leva uns 30 a 40 minutos; as seguintes, bem menos.
echo  Pode deixar esta janela aberta e fazer outra coisa.
echo.
set /a MINUTOS=0
:mac_esperar
set "ESTADO="
for /f "usebackq delims=" %%s in (`gh run view %EXECUCAO% --json status --jq ".status"`) do set "ESTADO=%%s"
if /i "%ESTADO%"=="completed" goto mac_terminou
echo  ... ainda compilando ^(%MINUTOS% min^)
timeout /t 60 /nobreak >nul
set /a MINUTOS+=1
goto mac_esperar

:mac_terminou
set "RESULTADO="
for /f "usebackq delims=" %%r in (`gh run view %EXECUCAO% --json conclusion --jq ".conclusion"`) do set "RESULTADO=%%r"
if not exist dist\mac mkdir dist\mac
if /i not "%RESULTADO%"=="success" (
    gh run view %EXECUCAO% --log-failed > dist\mac\erro-compilacao.txt 2>&1
    echo.
    echo  [ERRO] A compilacao no GitHub falhou ^(%RESULTADO%^). Final do registro:
    echo  ------------------------------------------------------------------
    powershell -NoProfile -Command "Get-Content -Tail 40 'dist\mac\erro-compilacao.txt'"
    echo  ------------------------------------------------------------------
    echo  Registro completo salvo em dist\mac\erro-compilacao.txt
    set "FALHOU=!FALHOU! Mac"
    exit /b 1
)
gh run download %EXECUCAO% -n verkzchat-mac -D dist\mac
if errorlevel 1 (set "FALHOU=!FALHOU! Mac" & exit /b 1)
echo  Pronto: dist\mac\ ^(VerkzChat-%VERSAO%-mac.zip^)
exit /b 0
