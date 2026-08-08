FROM node:22-bookworm-slim

ENV NODE_ENV=production
WORKDIR /work

COPY remote/deployments/benefactor-orchestrator/package.json ./package.json
RUN npm install --omit=dev --ignore-scripts --no-audit --no-fund \
  && npm cache clean --force

COPY remote/deployments/benefactor-orchestrator/ ./

ENV NODE_OPTIONS=--import=/work/scraper-contact-preload.mjs
USER node
ENTRYPOINT ["node"]
CMD ["contact-batch.mjs"]
