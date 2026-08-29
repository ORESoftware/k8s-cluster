#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

region="${1:?AWS region required}"
target_repository="${2:?target repository required}"
source_repository="${3:?source repository required}"
evidence_file="${4:?evidence output path required}"

[[ "$region" =~ ^[a-z0-9-]+$ ]]
[[ "$target_repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]
[[ "$source_repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]

for command in aws base64 curl git jq openssl sha256sum ssh ssh-keygen tar; do
  command -v "$command" >/dev/null || {
    printf 'deploy-key-bootstrap missing-command=%s\n' "$command" >&2
    exit 70
  }
done

work="$(mktemp -d /tmp/k8s-libs-deploy-key.XXXXXX)"
cleanup() {
  unset GH_TOKEN GIT_SSH_COMMAND
  python3 - "$work" <<'PY'
import shutil
import sys
from pathlib import Path
path = Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
}
trap cleanup EXIT

if ! command -v gh >/dev/null 2>&1; then
  gh_version='2.94.0'
  case "$(uname -m)" in
    x86_64|amd64) gh_arch=amd64 ;;
    aarch64|arm64) gh_arch=arm64 ;;
    *) printf 'unsupported architecture: %s\n' "$(uname -m)" >&2; exit 70 ;;
  esac
  archive="gh_${gh_version}_linux_${gh_arch}.tar.gz"
  release="https://github.com/cli/cli/releases/download/v${gh_version}"
  curl --fail --silent --show-error --location --retry 4 --retry-all-errors \
    --output "$work/$archive" "$release/$archive"
  curl --fail --silent --show-error --location --retry 4 --retry-all-errors \
    --output "$work/gh-checksums.txt" "$release/gh_${gh_version}_checksums.txt"
  grep -F "  $archive" "$work/gh-checksums.txt" > "$work/gh-checksum.txt"
  (cd "$work" && sha256sum --check --strict gh-checksum.txt)
  tar --extract --gzip --file "$work/$archive" --directory "$work"
  export PATH="$work/gh_${gh_version}_linux_${gh_arch}/bin:$PATH"
fi

publisher_secret="$work/publisher-secret.json"
aws secretsmanager get-secret-value \
  --region "$region" \
  --secret-id dd/remote-dev/agent-secrets \
  --query SecretString \
  --output text > "$publisher_secret"
GH_TOKEN="$(jq -er '.GH_PAT | select(type == "string" and length > 0)' "$publisher_secret")"
export GH_TOKEN
rm -f "$publisher_secret"

authenticated_login="$(gh api user --jq '.login')"
[[ "$authenticated_login" == ORESoftware ]]

key_prefix='k8s-cluster repo-checks read-only'
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
key_title="$key_prefix $timestamp"
private_key="$work/id_ed25519"
public_key="$private_key.pub"
ssh-keygen -q -t ed25519 -a 100 -N '' -C "$key_title" -f "$private_key"
chmod 600 "$private_key"
chmod 644 "$public_key"
key_fingerprint="$(ssh-keygen -lf "$public_key" -E sha256 | awk '{print $2}')"
[[ "$key_fingerprint" == SHA256:* ]]

existing_keys="$work/existing-deploy-keys.json"
gh api --paginate --slurp "repos/$source_repository/keys?per_page=100" \
  | jq 'add' > "$existing_keys"
while IFS= read -r key_id; do
  [[ "$key_id" =~ ^[0-9]+$ ]]
  gh api --method DELETE "repos/$source_repository/keys/$key_id"
done < <(
  jq -r --arg prefix "$key_prefix" \
    '.[] | select((.title // "") | startswith($prefix)) | .id' \
    "$existing_keys"
)

created_key="$work/created-deploy-key.json"
gh api --method POST "repos/$source_repository/keys" \
  --raw-field title="$key_title" \
  --raw-field key="$(cat "$public_key")" \
  -F read_only=true > "$created_key"
jq -e --arg title "$key_title" '
  (.id | type == "number") and
  .title == $title and
  .read_only == true and
  (.verified == true or .verified == false)
' "$created_key" >/dev/null
deploy_key_id="$(jq -r '.id' "$created_key")"

known_hosts="$work/known_hosts"
ssh -o BatchMode=yes \
  -o ConnectTimeout=20 \
  -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=accept-new \
  -o UserKnownHostsFile="$known_hosts" \
  -i "$private_key" \
  -T git@github.com > "$work/ssh-auth.stdout" 2> "$work/ssh-auth.stderr" || ssh_status=$?
ssh_status="${ssh_status:-0}"
# GitHub's successful SSH authentication deliberately exits 1 after printing the
# no-shell message; reject every other status and require that message.
[[ "$ssh_status" -eq 1 ]]
grep -Fq 'successfully authenticated' "$work/ssh-auth.stderr"

GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=20 -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$known_hosts -i $private_key"
export GIT_SSH_COMMAND
remote_main="$(git ls-remote "git@github.com:$source_repository.git" refs/heads/main | awk '{print $1}')"
[[ "$remote_main" =~ ^[0-9a-f]{40}$ ]]
unset GIT_SSH_COMMAND

gh secret set K8S_LIBS_DEPLOY_KEY --repo "$target_repository" < "$private_key"
secret_names="$work/repository-secret-names.txt"
gh secret list --repo "$target_repository" --json name --jq '.[].name' \
  | LC_ALL=C sort > "$secret_names"
grep -Fxq K8S_LIBS_DEPLOY_KEY "$secret_names"

mkdir -p "$(dirname "$evidence_file")"
jq -n \
  --arg target_repository "$target_repository" \
  --arg source_repository "$source_repository" \
  --arg authenticated_login "$authenticated_login" \
  --arg key_title "$key_title" \
  --arg key_fingerprint "$key_fingerprint" \
  --arg remote_main "$remote_main" \
  --argjson deploy_key_id "$deploy_key_id" \
  '{
    schema_version: 1,
    target_repository: $target_repository,
    source_repository: $source_repository,
    authenticated_login: $authenticated_login,
    deploy_key_id: $deploy_key_id,
    deploy_key_title: $key_title,
    deploy_key_fingerprint: $key_fingerprint,
    read_only: true,
    source_main_sha: $remote_main,
    ssh_authentication_verified: true,
    repository_read_verified: true,
    actions_secret_name: "K8S_LIBS_DEPLOY_KEY",
    credential_value_recorded: false
  }' > "$evidence_file"
chmod 600 "$evidence_file"

printf 'rotated read-only deploy key id=%s source=%s target=%s\n' \
  "$deploy_key_id" "$source_repository" "$target_repository"
