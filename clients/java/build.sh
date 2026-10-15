#!/usr/bin/env bash
# Compile the Java 17 client with plain javac (no Maven required).
#
#   ./build.sh                                              # compile to ./out
#   java -cp out com.oresoftware.networkmutex.ProtocolTest  # offline tests
#   java -cp out com.oresoftware.networkmutex.Smoke         # live smoke
#
# If `javac` is not on PATH, set JAVA_HOME or JAVAC, e.g.
#   JAVAC=/opt/homebrew/opt/openjdk@17/bin/javac ./build.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"

JAVAC="${JAVAC:-javac}"
if ! command -v "$JAVAC" >/dev/null 2>&1; then
  if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/javac" ]; then
    JAVAC="$JAVA_HOME/bin/javac"
  elif [ -x /opt/homebrew/opt/openjdk@17/bin/javac ]; then
    JAVAC=/opt/homebrew/opt/openjdk@17/bin/javac
  else
    echo "FATAL: javac not found (need JDK 17+). Set JAVAC or JAVA_HOME." >&2
    exit 1
  fi
fi

mkdir -p out
# shellcheck disable=SC2046
"$JAVAC" --release 17 -d out $(find src -name '*.java')
echo "compiled -> $HERE/out"
