#!/usr/bin/env bash
# The headers a full native build links on a Linux CI runner: sound, gamepads,
# keyboard layouts and the file dialogs. ci.yml's clippy job and play.yml run
# this, so the list lives in one place.
#
# apt on a hosted runner sometimes can't resolve the package mirrors
# ("Temporary failure resolving 'azure.archive.ubuntu.com'"), and left alone it
# keeps trying each of a hundred packages until the job's own timeout: two
# clippy jobs sat here for 40 minutes on 2026-11-11. So every apt call has a
# time limit and the whole thing is tried three times before it fails.
set -uo pipefail

pkgs="pkg-config libasound2-dev libudev-dev libxkbcommon-dev libgtk-3-dev"
opts="-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 -o DPkg::Lock::Timeout=60"

for attempt in 1 2 3; do
    # $pkgs and $opts are word lists on purpose.
    # shellcheck disable=SC2086
    if sudo timeout 90 apt-get $opts update \
        && sudo timeout 180 apt-get $opts install -y $pkgs; then
        echo "linux-build-deps: installed (attempt $attempt)"
        exit 0
    fi
    echo "::warning::linux-build-deps: apt attempt $attempt failed or timed out"
    # An install cut off part way leaves dpkg needing this before the next try.
    sudo dpkg --configure -a >/dev/null 2>&1 || true
    sleep $((attempt * 15))
done

echo "::error::linux-build-deps: apt failed three times; the runner's network or the Ubuntu mirrors are likely at fault. Re-run the job." >&2
exit 1
