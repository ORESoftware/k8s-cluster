FROM ghcr.io/gleam-lang/gleam:v1.16.0-node-alpine
WORKDIR /app

COPY package.json ./
RUN npm install --omit=dev

COPY . .

CMD ["gleam", "run", "--target", "javascript"]
