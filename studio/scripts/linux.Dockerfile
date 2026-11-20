# Builds Dawnroom's Linux bundles (.deb + AppImage) in a clean Ubuntu 22.04 (glibc 2.35, the oldest
# base we support). From the repo root:
#   docker build -f studio/scripts/linux.Dockerfile -t dawnroom-linux-build .
#   docker run --rm -v "$PWD:/src:ro" -v "$PWD/.shots/runA/linux-out:/out" dawnroom-linux-build
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential curl wget file ca-certificates git pkg-config libssl-dev xdg-utils \
      libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev patchelf \
 && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && apt-get install -y nodejs && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
ENV PATH=/root/.cargo/bin:$PATH
COPY studio/scripts/build-linux.sh /build-linux.sh
CMD ["bash", "/build-linux.sh"]
