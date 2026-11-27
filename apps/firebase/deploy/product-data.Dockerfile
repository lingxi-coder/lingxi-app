# Build context contains the Firebase source and a lingxi-data/ archive from LINGXI_DATA_BASELINE.
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS core
WORKDIR /src
COPY lingxi-data/ ./
RUN cargo build --locked --release --bin lingxi-data && strip target/release/lingxi-data

FROM node:22-bookworm-slim@sha256:e9f565d32859f0cbfb52125eafe8be39390f24dac302dc3cdf566e8ba3ab21ee
WORKDIR /srv
RUN apt-get update && apt-get install -y --no-install-recommends openssl && rm -rf /var/lib/apt/lists/*
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts --no-fund --no-audit && npm cache clean --force
COPY scripts/serve-lingxi-data.mjs scripts/start-cloud-data-core.mjs ./scripts/
COPY src/ ./src/
COPY --from=core /src/target/release/lingxi-data /usr/local/bin/lingxi-data
COPY deploy/start-product-data.sh /usr/local/bin/start-product-data
USER node
ENTRYPOINT ["/bin/sh", "/usr/local/bin/start-product-data"]
