FROM node:22-bookworm-slim@sha256:e9f565d32859f0cbfb52125eafe8be39390f24dac302dc3cdf566e8ba3ab21ee
WORKDIR /srv
COPY deploy/dataset-gateway/package.json deploy/dataset-gateway/package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts --no-fund --no-audit && npm cache clean --force
COPY scripts ./scripts
COPY src ./src
USER node
ENTRYPOINT ["node","scripts/serve-dataset-staging.mjs"]
