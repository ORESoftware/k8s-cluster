# zed-monorepo: orchestrate the retained submodules under apps/.
.DEFAULT_GOAL := help
RUST_SVCS := zed-interfaces zed-api-server.rs zed-web-server.rs
TS_PKGS   := zed-clients/typescript zed-sync

.PHONY: help init pull status validate test build images site

help: ## List targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*##/\t/' | sort

init: ## Sync and initialize all retained submodules
	git submodule sync --recursive
	git submodule update --init --recursive

pull: ## Advance retained submodules using their configured remotes
	git submodule sync --recursive
	git submodule update --init --recursive --remote --merge

status: ## Show recursive pinned-submodule status
	git submodule status --recursive

validate: ## Enforce package, inventory, and gitlink invariants
	python3 scripts/check-portfolio-inventory.py

test: validate ## Run retained repos' tests
	cd apps/zed-interfaces && cargo test
	cd apps/zed-api-server.rs && cargo test --workspace
	cd apps/zed-web-server.rs && cargo test
	cd apps/zed-clients/rust && cargo test
	cd apps/zed-clients/typescript && npm ci && npm run build && npm test
	cd apps/zed-clients/python && python3 -m unittest
	cd apps/zed-clients/go && go test ./...
	cd apps/zed-sync && npm ci && npm run build && npm test

build: validate ## Build retained Rust services and TypeScript packages
	cd apps/zed-api-server.rs && cargo build --release --workspace
	cd apps/zed-web-server.rs && cargo build --release
	cd apps/zed-clients/typescript && npm ci && npm run build
	cd apps/zed-sync && npm ci && npm run build

images: validate ## Build the api/web container images (context = apps/)
	docker build -f apps/zed-api-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-api-server:dev apps
	docker build -f apps/zed-web-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-web-server:dev apps

site: validate ## Build the marketing site
	cd apps/zed-pkg.github.io && npm ci && npm run build
