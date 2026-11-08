#!/bin/sh
# dev/alsa-local.sh — provision ALSA (libasound) WITHOUT root, for the
# `audio-cpal` feature on hosts that lack the dev headers
# (e.g. the autonomous-work sandbox: no sudo, no libasound2-dev).
#
# What it does:
#   1. Builds alsa-lib 1.2.14 from source into $HOME/.local
#   2. Writes ~/.asoundrc mapping `default` to ALSA's software `null`
#      PCM, so the full output path is exercisable with no hardware
#   3. Prints the env vars cargo needs (PKG_CONFIG_PATH/LD_LIBRARY_PATH)
#
# Usage:
#   sh dev/alsa-local.sh
#   eval "$(sh dev/alsa-local.sh)"      # also export the env vars
#
# Then:
#   cargo test -p prototype-audio --features audio-cpal
#   cargo run -p prototype-bin --features audio-cpal -- \
#       play-audio <file.p3d> <clip-name>
#
# Evidence: block 8 (2026-11-03) — this exact procedure produced a
# passing end-to-end CPAL stream (48 000 Hz × 2 ch, 77 386 callbacks,
# 0 stream errors) inside the sandbox.  See PROGRESS.md.
set -e

VERSION=1.2.14
PREFIX="$HOME/.local"
TARBALL="/tmp/alsa-lib-$VERSION.tar.bz2"

if [ ! -f "$PREFIX/lib/pkgconfig/alsa.pc" ]; then
    if [ ! -d "/tmp/alsa-lib-$VERSION" ]; then
        curl -sSfL --max-time 120 -o "$TARBALL" \
            "https://www.alsa-project.org/files/pub/lib/alsa-lib-$VERSION.tar.bz2"
        tar xjf "$TARBALL" -C /tmp
    fi
    cd "/tmp/alsa-lib-$VERSION"
    ./configure --prefix="$PREFIX" --disable-python --disable-topology >/dev/null
    make -j2 >/dev/null
    make install >/dev/null
fi

printf 'pcm.!default { type null }\n' > "$HOME/.asoundrc"

echo "ALSA ready: $PREFIX (null default device in ~/.asoundrc)"
echo "export PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig"
echo "export LD_LIBRARY_PATH=$PREFIX/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
