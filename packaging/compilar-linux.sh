#!/usr/bin/env bash
# Compila o VerkzChat para Linux e gera os pacotes: .AppImage, .deb e .rpm.
# O compilar.bat chama este script dentro do WSL (Ubuntu 22.04); também roda
# direto num Ubuntu/Debian.
#
#   bash packaging/compilar-linux.sh
#
# Resultado em dist/linux/:
#   verkzchat-<versão>-x86_64.AppImage   (o único que se atualiza sozinho)
#   verkzchat_<versão>_amd64.deb
#   verkzchat-<versão>-1.x86_64.rpm
set -euo pipefail

cd "${VERKZCHAT_RAIZ:-$(dirname "$0")/..}"
root="$(pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"

echo "== VerkzChat $version para Linux (AppImage, deb e rpm) =="

# Dependências do sistema (pede a senha do Linux na primeira vez).
need=(ca-certificates build-essential pkg-config cmake perl clang libclang-dev curl file rpm
      binutils dpkg-dev libssl-dev libgtk-3-dev libxdo-dev libdbus-1-dev libudev-dev
      libasound2-dev libpipewire-0.3-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev)
missing=()
for package in "${need[@]}"; do
    dpkg -s "$package" >/dev/null 2>&1 || missing+=("$package")
done
if [ ${#missing[@]} -gt 0 ]; then
    echo "Instalando dependências: ${missing[*]}"
    sudo apt-get update
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "${missing[@]}"
fi

# Rust (a versão certa vem do rust-toolchain.toml).
if ! command -v cargo >/dev/null 2>&1; then
    [ -f "$HOME/.cargo/env" ] || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

# A compilação fica dentro do Linux: na pasta do Windows (/mnt/c) é bem mais lenta.
export CARGO_TARGET_DIR="$HOME/.cache/verkzchat-target"
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-thin}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}"
# O rustc usa de 2 a 3 GB por crate em release: limita o paralelismo pela RAM livre.
if [ -z "${CARGO_BUILD_JOBS:-}" ]; then
    avail_gb=$(awk '/MemAvailable/ {printf "%d", $2/1048576/2.5}' /proc/meminfo)
    cpus=$(nproc)
    jobs=$(( avail_gb < cpus ? avail_gb : cpus ))
    export CARGO_BUILD_JOBS=$(( jobs < 1 ? 1 : jobs ))
fi
echo "Compilando com $CARGO_BUILD_JOBS tarefas em paralelo"
cargo build --release --locked
binary="$CARGO_TARGET_DIR/release/verkzchat"
strip "$binary" 2>/dev/null || true

out="$root/dist/linux"
rm -rf "$out"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# O arquivo .desktop do repositório chama `verkzchat`, o certo para pacote.
desktop="$root/packaging/applications/verkzchat.desktop"
icon="$root/packaging/icons/verkzchat.svg"

# Dependências de execução do .deb, tiradas das bibliotecas que o programa
# realmente usa (assim não ficam listas à mão que envelhecem).
deps="$({ ldd "$binary" 2>/dev/null || true; } | awk '/=> \// {print $3}' | while read -r lib; do
            dpkg -S "$(readlink -f "$lib")" 2>/dev/null | head -n 1 | cut -d: -f1 || true
        done | sort -u | tr '\n' ',' | sed 's/,$//; s/,/, /g' || true)"
[ -n "$deps" ] || deps="libc6"
echo "Dependências do .deb: $deps"

# ---------------------------------------------------------------- .deb ----
deb="$work/deb"
mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/share/applications" \
         "$deb/usr/share/icons/hicolor/scalable/apps" "$deb/usr/share/doc/verkzchat"
install -m 0755 "$binary" "$deb/usr/bin/verkzchat"
install -m 0644 "$desktop" "$deb/usr/share/applications/verkzchat.desktop"
install -m 0644 "$icon" "$deb/usr/share/icons/hicolor/scalable/apps/verkzchat.svg"
install -m 0644 LICENSE THIRD-PARTY-NOTICES.md "$deb/usr/share/doc/verkzchat/"
size="$(du -sk "$deb/usr" | cut -f1)"
cat > "$deb/DEBIAN/control" <<CONTROL
Package: verkzchat
Version: $version
Section: net
Priority: optional
Architecture: amd64
Maintainer: Lopes <contato@verkzsystem.tech>
Installed-Size: $size
Depends: $deps
Homepage: https://verkzsystem.tech/apps
Description: VerkzChat, o WhatsApp da VERKZ com o Claude como assistente
 Cliente de WhatsApp nativo para desktop. Esta cópia é instalada pelo gerenciador
 de pacotes: as atualizações vêm por ele ou por uma nova versão deste pacote.
CONTROL
dpkg-deb --root-owner-group --build "$deb" "$out/verkzchat_${version}_amd64.deb" >/dev/null
echo "Pronto: dist/linux/verkzchat_${version}_amd64.deb"

# ---------------------------------------------------------------- .rpm ----
rpmtop="$work/rpm"
mkdir -p "$rpmtop"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
cp "$binary" "$rpmtop/SOURCES/verkzchat"
cp "$desktop" "$rpmtop/SOURCES/verkzchat.desktop"
cp "$icon" "$rpmtop/SOURCES/verkzchat.svg"
cp LICENSE THIRD-PARTY-NOTICES.md "$rpmtop/SOURCES/"
cat > "$rpmtop/SPECS/verkzchat.spec" <<SPEC
%global debug_package %{nil}
%global __os_install_post %{nil}
%define _build_id_links none
Name:           verkzchat
Version:        $version
Release:        1
Summary:        VerkzChat, o WhatsApp da VERKZ com o Claude como assistente
License:        MIT
URL:            https://verkzsystem.tech/apps
BuildArch:      x86_64

%description
Cliente de WhatsApp nativo para desktop. Esta cópia é instalada pelo gerenciador
de pacotes: as atualizações vêm por ele ou por uma nova versão deste pacote.

%install
install -D -m 0755 %{_sourcedir}/verkzchat %{buildroot}/usr/bin/verkzchat
install -D -m 0644 %{_sourcedir}/verkzchat.desktop %{buildroot}/usr/share/applications/verkzchat.desktop
install -D -m 0644 %{_sourcedir}/verkzchat.svg %{buildroot}/usr/share/icons/hicolor/scalable/apps/verkzchat.svg
install -D -m 0644 %{_sourcedir}/LICENSE %{buildroot}/usr/share/licenses/verkzchat/LICENSE
install -D -m 0644 %{_sourcedir}/THIRD-PARTY-NOTICES.md %{buildroot}/usr/share/licenses/verkzchat/THIRD-PARTY-NOTICES.md

%files
/usr/bin/verkzchat
/usr/share/applications/verkzchat.desktop
/usr/share/icons/hicolor/scalable/apps/verkzchat.svg
/usr/share/licenses/verkzchat/
SPEC
if rpmbuild --quiet --define "_topdir $rpmtop" -bb "$rpmtop/SPECS/verkzchat.spec" \
   && cp "$rpmtop"/RPMS/x86_64/*.rpm "$out/verkzchat-${version}-1.x86_64.rpm"; then
    echo "Pronto: dist/linux/verkzchat-${version}-1.x86_64.rpm"
else
    echo "[AVISO] Não consegui gerar o .rpm; o .deb e o .AppImage seguem."
fi

# ------------------------------------------------------------ AppImage ----
# O nome é o que o atualizador do app procura: <slug>-<versão>-<arquitetura>.AppImage
tools="$HOME/.cache/verkzchat-tools"
mkdir -p "$tools"
appimagetool="$tools/appimagetool-x86_64.AppImage"
if [ ! -x "$appimagetool" ]; then
    curl -fL -o "$appimagetool" \
        https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$appimagetool"
fi
appdir="$work/VerkzChat.AppDir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/icons/hicolor/scalable/apps"
install -m 0755 "$binary" "$appdir/usr/bin/verkzchat"
install -m 0644 "$icon" "$appdir/verkzchat.svg"
install -m 0644 "$icon" "$appdir/usr/share/icons/hicolor/scalable/apps/verkzchat.svg"
install -m 0644 "$desktop" "$appdir/verkzchat.desktop"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/verkzchat" "$@"
APPRUN
chmod +x "$appdir/AppRun"
# --appimage-extract-and-run: roda sem FUSE, que o WSL normalmente não tem.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$appimagetool" --no-appstream \
    "$appdir" "$out/verkzchat-${version}-x86_64.AppImage"
chmod +x "$out/verkzchat-${version}-x86_64.AppImage"
echo "Pronto: dist/linux/verkzchat-${version}-x86_64.AppImage"
echo "Obs.: só o AppImage se atualiza sozinho; .deb e .rpm são atualizados pelo gerenciador de pacotes."
