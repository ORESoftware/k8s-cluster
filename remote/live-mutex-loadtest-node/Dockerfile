FROM docker.io/library/node:22-bookworm-slim

WORKDIR /app

COPY package*.json ./
RUN npm ci --omit=dev --ignore-scripts

COPY src ./src

CMD ["node", "src/main.js"]
