@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"
rem Compila a versão de Mac na nuvem do GitHub. Repositório PÚBLICO: o tempo de
rem compilação em Mac é grátis para repositórios públicos, e o código-fonte
rem fica visível para qualquer pessoa. Autor: Lopes.
set REPO=verkzchat-build

where gh >nul 2>nul || (echo Instale o GitHub CLI em https://cli.github.com e rode "gh auth login". & pause & exit /b 1)
where git >nul 2>nul || (echo Instale o Git em https://git-scm.com. & pause & exit /b 1)
gh auth status >nul 2>nul || (echo Rode "gh auth login" primeiro. & pause & exit /b 1)

if not exist .git (
  git init -b main >nul
)
git add -A
git -c user.name=Lopes -c user.email=lopes@users.noreply.github.com commit -m "Compilar para Mac" >nul 2>nul

git remote get-url origin >nul 2>nul
if errorlevel 1 (
  echo Criando o repositório público %REPO%...
  gh repo create %REPO% --public --source . --remote origin --push || (pause & exit /b 1)
) else (
  git push -u origin HEAD || (pause & exit /b 1)
)

echo Iniciando a compilação do Mac...
gh workflow run compilar.yml -f sistemas=mac || (pause & exit /b 1)
timeout /t 8 /nobreak >nul
for /f %%i in ('gh run list --workflow compilar.yml --limit 1 --json databaseId --jq ".[0].databaseId"') do set RUN=%%i
gh run watch %RUN% --exit-status || (echo A compilação falhou. Veja: gh run view %RUN% --log-failed & pause & exit /b 1)

if exist dist\mac rmdir /s /q dist\mac
gh run download %RUN% -n verkzchat-mac -D dist\mac || (pause & exit /b 1)
echo.
echo Pronto: dist\mac
explorer dist\mac
pause
