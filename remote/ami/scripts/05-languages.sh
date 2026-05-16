#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# 05-languages.sh - Node/nvm, Python 2/3, Go, Rust, Java, Elixir, Gleam,
#                   Dart/Flutter, TypeScript, pnpm, and AWS SDK packages
# Runs as: ec2-user, with sudo available for system installs
# Env:
#   NODE_VERSION   - Node.js major or exact version
#   GO_VERSION     - Go version without leading v
#   RUST_PROFILE   - rustup profile: minimal | default | complete
#   JAVA_VERSION   - Corretto major version
#   GLEAM_VERSION  - "latest" or a version without leading v
#   ELIXIR_VERSION - "latest" or a version without leading v
# ---------------------------------------------------------------------------
set -euo pipefail

echo "=========================================================="
echo "  05 - Language Runtimes and Developer SDKs"
echo "=========================================================="

export HOME="/home/ec2-user"
export USER="ec2-user"

NODE_VERSION="${NODE_VERSION:-22}"
GO_VERSION="${GO_VERSION:-1.23.0}"
RUST_PROFILE="${RUST_PROFILE:-default}"
JAVA_VERSION="${JAVA_VERSION:-21}"
GLEAM_VERSION="${GLEAM_VERSION:-1.6.1}"
ELIXIR_VERSION="${ELIXIR_VERSION:-1.17.3}"

echo "Installing Python 3, Java ${JAVA_VERSION}, and runtime prerequisites..."
sudo dnf install -y \
  python3 \
  python3-pip \
  python3-devel \
  "java-${JAVA_VERSION}-amazon-corretto-devel" \
  unzip \
  zip \
  glibc-langpack-en \
  openssl-devel \
  readline-devel \
  sqlite-devel \
  xz-devel \
  zlib-devel \
  bzip2-devel \
  libffi-devel

python3 -m pip install --user --upgrade pip
python3 -m pip install --user --upgrade boto3 botocore virtualenv

echo "Installing Python 2.7.18 from source..."
if ! command -v python2 >/dev/null 2>&1; then
  mkdir -p /tmp/python2-src
  curl -fsSL https://www.python.org/ftp/python/2.7.18/Python-2.7.18.tgz -o /tmp/python2.tgz
  tar -xzf /tmp/python2.tgz --strip-components=1 -C /tmp/python2-src
  cd /tmp/python2-src
  ./configure --prefix=/usr/local --enable-optimizations
  make -j"$(nproc)"
  sudo make altinstall
  sudo ln -sf /usr/local/bin/python2.7 /usr/local/bin/python2
  cd "$HOME"
fi

echo "Installing nvm and Node.js ${NODE_VERSION}..."
export NVM_DIR="$HOME/.nvm"
mkdir -p "$NVM_DIR"
curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh -o /tmp/install-nvm.sh
bash /tmp/install-nvm.sh
source "$NVM_DIR/nvm.sh"
nvm install "${NODE_VERSION}"
nvm alias default "${NODE_VERSION}"
nvm use default
corepack enable
npm install -g \
  pnpm \
  typescript \
  ts-node \
  aws-cdk \
  aws-sdk \
  @aws-sdk/client-ec2 \
  @aws-sdk/client-eks \
  @aws-sdk/client-ecr \
  @aws-sdk/client-s3

NODE_BIN_DIR="$(dirname "$(command -v node)")"
for node_tool in node npm npx pnpm tsc ts-node cdk; do
  if [[ -x "${NODE_BIN_DIR}/${node_tool}" ]]; then
    sudo ln -sf "${NODE_BIN_DIR}/${node_tool}" "/usr/local/bin/${node_tool}"
  fi
done

echo "Installing Go ${GO_VERSION}..."
curl -fsSL "https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz" -o /tmp/go.tgz
sudo mkdir -p "/usr/local/go-${GO_VERSION}"
sudo tar -xzf /tmp/go.tgz --strip-components=1 -C "/usr/local/go-${GO_VERSION}"
sudo ln -sfn "/usr/local/go-${GO_VERSION}" /usr/local/go

echo "Installing Rust via rustup..."
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh
bash /tmp/rustup-init.sh -y --profile "${RUST_PROFILE}" --default-toolchain stable
source "$HOME/.cargo/env"

echo "Installing Gleam..."
if [[ "${GLEAM_VERSION}" == "latest" ]]; then
  GLEAM_TAG=$(curl -fsSL https://api.github.com/repos/gleam-lang/gleam/releases/latest | jq -r .tag_name)
else
  GLEAM_TAG="v${GLEAM_VERSION}"
fi
GLEAM_VERSION_NORMALIZED="${GLEAM_TAG#v}"
curl -fsSL "https://github.com/gleam-lang/gleam/releases/download/${GLEAM_TAG}/gleam-v${GLEAM_VERSION_NORMALIZED}-x86_64-unknown-linux-musl.tar.gz" -o /tmp/gleam.tgz
tar -xzf /tmp/gleam.tgz -C /tmp
sudo install -m 0755 /tmp/gleam /usr/local/bin/gleam

echo "Installing Erlang/OTP and Elixir..."
sudo dnf install -y erlang || sudo dnf install -y \
  erlang-erts \
  erlang-kernel \
  erlang-stdlib \
  erlang-compiler \
  erlang-crypto \
  erlang-parsetools \
  erlang-public_key \
  erlang-ssl \
  erlang-tools \
  erlang-syntax_tools \
  erlang-inets \
  erlang-mnesia

if [[ "${ELIXIR_VERSION}" == "latest" ]]; then
  ELIXIR_TAG=$(curl -fsSL https://api.github.com/repos/elixir-lang/elixir/releases/latest | jq -r .tag_name)
else
  ELIXIR_TAG="v${ELIXIR_VERSION}"
fi
OTP_MAJOR=$(erl -noshell -eval 'io:format("~s", [erlang:system_info(otp_release)]), halt().')
sudo mkdir -p /usr/local/lib/elixir
curl -fsSL "https://github.com/elixir-lang/elixir/releases/download/${ELIXIR_TAG}/elixir-otp-${OTP_MAJOR}.zip" -o /tmp/elixir.zip \
  || curl -fsSL "https://github.com/elixir-lang/elixir/releases/download/${ELIXIR_TAG}/elixir-otp-27.zip" -o /tmp/elixir.zip \
  || curl -fsSL "https://github.com/elixir-lang/elixir/releases/download/${ELIXIR_TAG}/elixir-otp-26.zip" -o /tmp/elixir.zip \
  || curl -fsSL "https://github.com/elixir-lang/elixir/releases/download/${ELIXIR_TAG}/elixir-otp-25.zip" -o /tmp/elixir.zip
sudo unzip -oq /tmp/elixir.zip -d /usr/local/lib/elixir
for elixir_tool in /usr/local/lib/elixir/bin/*; do
  sudo ln -sf "${elixir_tool}" "/usr/local/bin/$(basename "${elixir_tool}")"
done

echo "Installing Flutter stable channel and Dart..."
sudo mkdir -p /opt
sudo git clone https://github.com/flutter/flutter.git -b stable --depth 1 /opt/flutter
sudo chown -R ec2-user:ec2-user /opt/flutter
export PATH="/opt/flutter/bin:/opt/flutter/bin/cache/dart-sdk/bin:/usr/local/go/bin:$HOME/go/bin:$HOME/.local/bin:$PATH"
flutter config --no-analytics
flutter precache --linux
sudo ln -sf /opt/flutter/bin/flutter /usr/local/bin/flutter
if [[ -x /opt/flutter/bin/cache/dart-sdk/bin/dart ]]; then
  sudo ln -sf /opt/flutter/bin/cache/dart-sdk/bin/dart /usr/local/bin/dart
fi

echo "Writing language PATH profile..."
sudo tee /etc/profile.d/dd-languages.sh >/dev/null <<'PROFILE'
export PATH="$PATH:/usr/local/go/bin:$HOME/go/bin:$HOME/.local/bin"
export PATH="$PATH:/opt/flutter/bin:/opt/flutter/bin/cache/dart-sdk/bin"
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
[ -s "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if command -v npm >/dev/null 2>&1; then
  export NODE_PATH="$(npm root -g 2>/dev/null):${NODE_PATH:-}"
fi
PROFILE

echo ""
echo "--- Language Runtime Versions ---"
git --version
python2 --version 2>&1
python3 --version
java -version 2>&1 | head -1
node --version
npm --version
pnpm --version
tsc --version
cdk --version
NODE_PATH="$(npm root -g)" node -e "console.log('aws-sdk:', require('aws-sdk/package.json').version)"
python3 -c "import boto3; print('boto3:', boto3.__version__)"
/usr/local/go/bin/go version
rustc --version
cargo --version
gleam --version
erl -noshell -eval 'io:format("erlang: ~s~n", [erlang:system_info(otp_release)]), halt().'
elixir --version
flutter --version
dart --version

echo "05 - Language runtime installation complete"
