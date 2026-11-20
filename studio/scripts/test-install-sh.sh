#!/usr/bin/env bash
# Tests site/public/install.sh in a clean ubuntu:22.04 against a fake release served in the container:
#   docker run --rm -v "<linux build out>:/out:ro" -v "<repo>:/src:ro" ubuntu:22.04 bash /src/studio/scripts/test-install-sh.sh
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null && apt-get install -y -qq curl python3 >/dev/null 2>&1
python3 /src/studio/scripts/fake-release.py /tmp/rel 0.1.0 8767 /out/Dawnroom_0.1.0_amd64.AppImage /out/dawnroom-0.1.0-linux-x64.tar.gz >/dev/null 2>&1 &
sleep 2
useradd -m tester   # a normal user: no sudo, its own ~/.local
run() { su - tester -c "export DAWNROOM_RELEASES=$1 PATH=\$HOME/.local/bin:\$PATH; sh /src/site/public/install.sh $2"; }
echo "== fresh install";      run http://127.0.0.1:8767/good/release.json ""
su - tester -c 'ls ~/.local/lib/dawnroom ~/.local/share/applications; readlink ~/.local/bin/dawnroom; ~/.local/bin/dawnroom --version'
echo "== re-run (update)";    run http://127.0.0.1:8767/good/release.json ""
echo "== checksum mismatch";  su - tester -c 'touch -d 2000-01-01 ~/.local/lib/dawnroom/Dawnroom.AppImage'
run http://127.0.0.1:8767/bad/release.json ""; echo "exit $?"
su - tester -c 'echo "AppImage untouched: $(date -r ~/.local/lib/dawnroom/Dawnroom.AppImage +%Y)"; ls /tmp | grep -c "^tmp" || true'
echo "== --cli as another user"; useradd -m cli
su - cli -c "DAWNROOM_RELEASES=http://127.0.0.1:8767/good/release.json sh /src/site/public/install.sh --cli"
su - cli -c 'ls ~/.local/lib/dawnroom; ~/.local/bin/dawnroom --version'
echo "== engine from the --cli install reads its own registry"
su - cli -c '~/.local/bin/dawnroom --port 8433 serve --data-dir /tmp/clidata >/dev/null 2>&1 & sleep 4; ~/.local/bin/dawnroom --port 8433 list --all | grep -c "not installed"; ~/.local/bin/dawnroom --port 8433 stop'

# `dawnroom update` against a newer fake release: the real 0.1.1 engine tarball from the build's
# /out/next, and (for full installs) the 0.1.0 AppImage published under the 0.1.1 name.
echo "== dawnroom update"
mkdir -p /tmp/next && cp /out/next/dawnroom-0.1.1-linux-x64.tar.gz /tmp/next/ && cp /out/Dawnroom_0.1.0_amd64.AppImage /tmp/next/Dawnroom_0.1.1_amd64.AppImage
python3 /src/studio/scripts/fake-release.py /tmp/rel2 0.1.1 8768 /tmp/next/dawnroom-0.1.1-linux-x64.tar.gz /tmp/next/Dawnroom_0.1.1_amd64.AppImage >/dev/null 2>&1 &
sleep 2
U=http://127.0.0.1:8768
echo "-- the --cli user, tampered release first"
su - cli -c "DAWNROOM_RELEASES=$U/bad/release.json ~/.local/bin/dawnroom --port 8434 update"; echo "exit $?"
su - cli -c '~/.local/bin/dawnroom --version'
echo "-- the --cli user, good release"
su - cli -c "DAWNROOM_RELEASES=$U/good/release.json ~/.local/bin/dawnroom --port 8434 update"; echo "exit $?"
su - cli -c '~/.local/bin/dawnroom --version; ls -a ~/.local/lib/dawnroom'
echo "-- again"
su - cli -c "DAWNROOM_RELEASES=$U/good/release.json ~/.local/bin/dawnroom --port 8434 update"
echo "-- the full install (tester), AppImage beside the engine"
su - tester -c 'sha256sum ~/.local/lib/dawnroom/Dawnroom.AppImage | cut -c1-16'
su - tester -c "DAWNROOM_RELEASES=$U/good/release.json ~/.local/bin/dawnroom --port 8435 update"; echo "exit $?"
su - tester -c '~/.local/bin/dawnroom --version; ls -la ~/.local/lib/dawnroom/Dawnroom.AppImage | cut -c1-10'
