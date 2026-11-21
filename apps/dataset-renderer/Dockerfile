FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update && apt-get install -y --no-install-recommends chromium fonts-noto-cjk nodejs   && rm -rf /var/lib/apt/lists/*
COPY worker.mjs /opt/lingxi/worker.mjs
RUN chmod 0444 /opt/lingxi/worker.mjs
ENV HOME=/tmp/viewer-home
USER 65532:65532
ENTRYPOINT ["/usr/bin/node", "/opt/lingxi/worker.mjs"]
