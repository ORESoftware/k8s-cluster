#!/bin/bash
# List all repos for each qualifying org and check local clone status
# Orgs created after 2024-07-30

ORGS=(
  dancing-dragons
  benefactor-cc
  sonus-auris
  usa-acc
  3FA-app
  fiducia-plus
  fiducia-run
  fiducia-app
  akrion-sim
  kanonas-cloud
  kanonikal
  canonical-cloud
  fiducia-cloud
  declarative-migrations
  forestal-col
  athlet-o
  daedalus-fab
  scintilla-run
  quaestor-ledger
  sagitta-stack
  claritas-viz
  discrete-event-systems
  shared-auth
  messaging-intel
  drone-mngr
  laser-ptr-ctrl
  zed-pkg
  opto-sync
  fifa-math
  voxletra
  z-pkg
  zed-cli
  zedcli
  anticaptrad
  zed-pkg-test
  rust-ssr-demos
  subjunctiv
  cliptown
  den-pkg
  den-cli
  networking-components
  file-tunnel
)

BASE="$HOME/codes"

for org in "${ORGS[@]}"; do
  # Map org name to local directory (handle dots/hyphens - user examples show hyphens -> dots sometimes)
  local_dir="$BASE/$org"
  
  # Get all repos for this org
  repos=$(gh api "/orgs/$org/repos" --paginate --jq '.[].name' 2>/dev/null)
  
  if [ -z "$repos" ]; then
    echo "ORG:$org	REPOS:0	STATUS:empty"
    continue
  fi
  
  repo_count=$(echo "$repos" | wc -l | tr -d ' ')
  
  for repo in $repos; do
    clone_path="$local_dir/$repo"
    if [ -d "$clone_path/.git" ]; then
      echo "EXISTING	$org/$repo	$clone_path"
    else
      echo "MISSING	$org/$repo	$clone_path"
    fi
  done
done
