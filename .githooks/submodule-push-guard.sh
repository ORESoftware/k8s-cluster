#!/usr/bin/env bash
# Block a superproject push when any submodule gitlink it would publish points at
# a commit that is NOT reachable on that submodule's own remote.
#
# Why this exists
# ---------------
# The sync bot (and humans) can commit a moved submodule pointer in this repo and
# push the superproject WITHOUT first pushing the submodule commit to the
# submodule's remote. The parent then references a submodule commit that exists
# nowhere fetchable, and every clone's `git pull` / `git submodule update` dies
# with:
#
#     fatal: remote error: upload-pack: not our ref <sha>
#
# Unlike the post-* reminder hooks (which only nag AFTER a pointer moved locally),
# this runs at PUSH time — the last point before the broken pointer becomes
# everyone else's problem — and refuses the push instead of just warning.
#
# Called by the `pre-push` hook, which forwards git's stdin and args:
#   $1 = remote name        $2 = remote url
#   stdin: <local ref> SP <local sha> SP <remote ref> SP <remote sha> per ref
#
# Emergency opt-out (use sparingly, it reintroduces the failure mode):
#   DD_SKIP_SUBMODULE_PUSH_GUARD=1 git push ...

set -u

case "${DD_SKIP_SUBMODULE_PUSH_GUARD:-}" in
    1 | true | yes | on) exit 0 ;;
esac

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0
[ -f "$repo_root/.gitmodules" ] || exit 0

# Git's well-known empty-tree object — used as the diff base when pushing a brand
# new branch (remote sha is all-zeros), so we vet every gitlink in the snapshot.
EMPTY_TREE='4b825dc642cb6eb9a060e54bf8d69288fbee4904'

is_zero() { case "$1" in *[!0]*) return 1 ;; *) return 0 ;; esac; }

# Resolve a submodule path to (gitdir, url). Works for an initialized submodule
# and for one that is configured + has a modules/ object store but no worktree.
resolve_gitdir() {
    local path="$1" gd
    # Git exports the superproject's GIT_DIR / GIT_WORK_TREE while running
    # hooks.  They must not leak into this nested invocation or Git resolves
    # the parent repository again and treats every gitlink as if it belonged to
    # the superproject remote.
    # Do not invoke `git -C` in an uninitialized submodule directory: Git walks
    # upward and reports the superproject's gitdir, which would validate a
    # gitlink against the *cluster* remote. An initialized submodule always has
    # its own .git file/directory at the configured path.
    if [ -e "$repo_root/$path/.git" ]; then
        gd="$(env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR -u GIT_INDEX_FILE \
            git -C "$repo_root/$path" rev-parse --absolute-git-dir 2>/dev/null)" \
            && { printf '%s\n' "$gd"; return 0; }
    fi
    gd="$(git -C "$repo_root" rev-parse --git-path "modules/$path" 2>/dev/null)"
    [ -n "$gd" ] && [ -d "$gd" ] && { printf '%s\n' "$gd"; return 0; }
    return 1
}

resolve_url() {
    local path="$1" gd="$2" url
    url="$(git --git-dir="$gd" config --get remote.origin.url 2>/dev/null)"
    [ -n "$url" ] && { printf '%s\n' "$url"; return 0; }
    git config -f "$repo_root/.gitmodules" --get "submodule.$path.url" 2>/dev/null
}

# Is commit $2 reachable from a branch/tag on the submodule remote $3 (gitdir $1)?
# Returns 0 = reachable (safe to push), 1 = NOT reachable (block), 2 = unverifiable.
gitlink_on_remote() {
    local gd="$1" sha="$2" url="$3"
    [ -n "$url" ] || return 2
    local ns="refs/_pushguard/$$"
    # Pull the remote's branch + tag tips (objects + refs) into a private
    # namespace. Cheap-ish; only runs for submodules whose pointer changed.
    git --git-dir="$gd" fetch -q --no-tags "$url" \
        "+refs/heads/*:$ns/h/*" "+refs/tags/*:$ns/t/*" 2>/dev/null
    local rc=1
    if git --git-dir="$gd" cat-file -e "${sha}^{commit}" 2>/dev/null; then
        local ref
        while IFS= read -r ref; do
            if git --git-dir="$gd" merge-base --is-ancestor "$sha" "$ref" 2>/dev/null; then
                rc=0
                break
            fi
        done < <(git --git-dir="$gd" for-each-ref --format='%(refname)' "$ns")
    fi
    # Clean up the private refs no matter what.
    git --git-dir="$gd" for-each-ref --format='delete %(refname)' "$ns" 2>/dev/null \
        | git --git-dir="$gd" update-ref --stdin 2>/dev/null
    return "$rc"
}

# Collect "path<TAB>sha" for every gitlink that this push would introduce/move,
# de-duplicated across all refs being pushed.
changed_gitlinks="$(
    while read -r _localref localsha _remoteref remotesha; do
        [ -n "${localsha:-}" ] || continue
        is_zero "$localsha" && continue   # branch deletion — nothing to vet
        if is_zero "${remotesha:-0}"; then base="$EMPTY_TREE"; else base="$remotesha"; fi
        git diff --raw --no-renames "$base" "$localsha" 2>/dev/null \
            | while IFS="$(printf '\t')" read -r meta path; do
                # meta = ":<srcmode> <dstmode> <srcsha> <dstsha> <status>"
                set -- $meta
                [ "${2:-}" = "160000" ] || continue   # dst must be a gitlink
                is_zero "${4:-0}" && continue          # gitlink removed
                printf '%s\t%s\n' "$path" "$4"
            done
    done | sort -u
)"

[ -n "$changed_gitlinks" ] || exit 0

problems=""
while IFS="$(printf '\t')" read -r path sha; do
    [ -n "$path" ] || continue
    gd="$(resolve_gitdir "$path")" || {
        problems+="  ✗ $path @ ${sha:0:12}  — submodule not initialized; cannot verify (run: git submodule update --init '$path')"$'\n'
        continue
    }
    url="$(resolve_url "$path" "$gd")"
    gitlink_on_remote "$gd" "$sha" "$url"
    case $? in
        0) : ;;  # reachable on remote — fine
        2) problems+="  ⚠ $path @ ${sha:0:12}  — no remote url resolved; cannot verify"$'\n' ;;
        *) problems+="  ✗ $path @ ${sha:0:12}  — commit is NOT on its remote ($url). Push the submodule first:"$'\n'"        ( cd '$path' && git push origin HEAD )"$'\n' ;;
    esac
done <<< "$changed_gitlinks"

[ -n "$problems" ] || exit 0

if [ -t 2 ]; then b='\033[1m'; rd='\033[31m'; r='\033[0m'; else b=''; rd=''; r=''; fi
{
    printf '%b' "${rd}${b}"
    printf 'pre-push blocked: submodule pointer(s) not on their remote\n'
    printf '%b' "${r}"
    printf '%s' "$problems"
    printf '\n'
    printf 'Pushing this superproject would record submodule commits that nobody can\n'
    printf 'fetch, breaking `git pull` / `git submodule update` for every clone with:\n'
    printf '    fatal: remote error: upload-pack: not our ref <sha>\n\n'
    printf 'Fix: push the submodule(s) above, then re-push this repo.\n'
    printf 'Override (reintroduces the failure mode): DD_SKIP_SUBMODULE_PUSH_GUARD=1 git push ...\n'
} >&2

exit 1
