FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update && apt-get install -y --no-install-recommends chromium fonts-noto-cjk libnss3-tools nodejs python3 && rm -rf /var/lib/apt/lists/*
COPY browser.mjs /opt/lingxi/browser.mjs
COPY observe.py /opt/lingxi/observe.py
COPY fixture.html /opt/lingxi/fixture.html
ENV HOME=/tmp/viewer-home
USER 65532:65532
ENTRYPOINT ["/usr/bin/node", "/opt/lingxi/browser.mjs"]
