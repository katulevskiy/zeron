# The cloud box image (docs/cloud.md): a Zeron engine plus the coding agents,
# started by a box's Durable Object as `docker.io/zeronsh/zeron-cloud:<version>`.
#
# Build context: this folder with the Linux x86_64 `zeron` binary next to this
# file (.github/workflows/cloud-image.yml takes it from the release tarball).
#
#   docker build --platform linux/amd64 -t zeron-cloud cloud/image
#
# ZERON_LIBC=gnu (the default) installs the shared libraries the desktop
# build links (gpui, the webview); they are loaded but unused headless. An
# engine-only static musl build (`--no-default-features`) passes
# ZERON_LIBC=musl and skips them.

FROM debian:trixie-slim

ARG ZERON_LIBC=gnu
ARG NODE_VERSION=24.21.0
ARG CLAUDE_CODE_VERSION=latest
ARG CODEX_VERSION=latest
ARG OPENCODE_VERSION=latest

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git openssh-client ripgrep python3 tini xz-utils \
      procps less unzip \
 && if [ "$ZERON_LIBC" = gnu ]; then \
      apt-get install -y --no-install-recommends \
        libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 \
        libwayland-egl1 libx11-6 libx11-xcb1 libxcb1 libxrandr2 libxi6 \
        libfontconfig1 libfreetype6 libasound2t64 libvulkan1 \
        libgtk-3-0t64 libwebkit2gtk-4.1-0 libjson-glib-1.0-0; \
    fi \
 && rm -rf /var/lib/apt/lists/*

# Node 24 LTS from the official tarball, checked against its SHASUMS256.
RUN curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
 && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" \
      | grep " node-v${NODE_VERSION}-linux-x64.tar.xz\$" | sha256sum -c - \
 && tar -xJf "node-v${NODE_VERSION}-linux-x64.tar.xz" -C /usr/local --strip-components=1 --no-same-owner \
 && rm "node-v${NODE_VERSION}-linux-x64.tar.xz" \
 && node --version && npm --version

RUN useradd --create-home --uid 1000 --user-group --shell /bin/bash zeron

# The `zeron` user, by number so the runtime needn't resolve the name.
USER 1000:1000
WORKDIR /home/zeron
# IS_SANDBOX: Claude Code accepts --dangerously-skip-permissions in a
# container only when told it is one.
ENV HOME=/home/zeron \
    NPM_CONFIG_PREFIX=/home/zeron/.npm-global \
    PATH=/home/zeron/.npm-global/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    IS_SANDBOX=1

# The agent CLIs, under the user's own npm prefix so Zeron can update them.
RUN npm install -g \
      "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
      "@openai/codex@${CODEX_VERSION}" \
      "opencode-ai@${OPENCODE_VERSION}" \
 && npm cache clean --force \
 && claude --version && codex --version && opencode --version

COPY --chmod=755 zeron /usr/local/bin/zeron
# Fail the build, not the first boot, when the binary needs a library the
# image lacks.
RUN if ldd /usr/local/bin/zeron 2>&1 | grep -q 'not found'; then \
      ldd /usr/local/bin/zeron | grep 'not found'; exit 1; \
    fi \
 && zeron --version

# The control server (ZERON_CLOUD_CONTROL_PORT, set by the Durable Object).
EXPOSE 8787

ENTRYPOINT ["/usr/bin/tini", "--", "zeron", "cloud-boot"]
