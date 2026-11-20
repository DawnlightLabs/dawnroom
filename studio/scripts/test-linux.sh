#!/usr/bin/env bash
# In a clean ubuntu:22.04 container, with the build output mounted at /out:
#   docker run --rm -v "<out>:/out:ro" ubuntu:22.04 bash /out/test-linux.sh
# 1) the .deb installs with apt alone (its Depends pull in what it needs), `dawnroom` works and finds
#    its registry; 2) Studio starts under a virtual display and starts the engine on 8420.
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
echo "== apt install ./Dawnroom_*.deb"
apt-get install -y -qq /out/Dawnroom_*_amd64.deb >/tmp/apt.log 2>&1 && echo "installed" || { tail -20 /tmp/apt.log; exit 1; }
dpkg -L dawnroom | grep -E "bin/|registry/runtimes/[^/]*$|\.desktop$" | head -6
echo "== dawnroom --version: $(dawnroom --version)"
echo "== dawnroom doctor (this container: no GPU)"
dawnroom doctor 2>&1 | grep -E "GPU|Runtime|OS|RAM" | head -8
# A stand-in vulkaninfo that reports one AMD discrete card (the summary format of a real Radeon), so
# the engine's own Linux detection and runtime choice run end to end. No real GPU is involved.
mkdir -p /tmp/fakebin && cat > /tmp/fakebin/vulkaninfo <<'VK'
#!/bin/sh
printf 'Devices:
========
GPU0:
	apiVersion = 1.3.287
	vendorID = 0x1002
	deviceID = 0x7480
	deviceType = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU
	deviceName = AMD Radeon RX 7600 (RADV NAVI33)
	driverInfo = Mesa 24.2.8
'
VK
chmod +x /tmp/fakebin/vulkaninfo
echo "== dawnroom doctor with the stand-in vulkaninfo (AMD RX 7600)"
PATH=/tmp/fakebin:$PATH dawnroom doctor 2>&1 | grep -E "GPU|Runtime" | head -6
echo "== dawnroom update on the .deb install (a system package): it says how it updates"
dawnroom --port 8499 update; echo "exit $?"
dawnroom --port 8431 serve --data-dir /tmp/data > /tmp/engine.log 2>&1 &
sleep 4
echo "== models listed by the installed engine: $(dawnroom --port 8431 list --all | grep -c ':q\|:fp\|:bf')"
dawnroom --port 8431 stop
echo "== Studio under Xvfb"
apt-get install -y -qq xvfb xauth >/dev/null 2>&1
xvfb-run -a dawnroom-studio > /tmp/studio.log 2>&1 &
for i in $(seq 1 20); do sleep 1; (exec 3<>/dev/tcp/127.0.0.1/8420) 2>/dev/null && break; done
if (exec 3<>/dev/tcp/127.0.0.1/8420) 2>/dev/null; then echo "Studio started the engine: 8420 is answering"; else echo "engine not answering"; tail -20 /tmp/studio.log; fi
ps -eo comm | grep -E "^dawnroom" | sort | uniq -c
