# zed-monorepo: orchestrate the submodules under apps/.
.DEFAULT_GOAL := help
RUST_SVCS := zed-interfaces zed-cli zed-api-server.rs zed-web-server.rs
TS_PKGS   := zed-clients/typescript zed-sync

.PHONY: help init pull status test build images site

help: ## List targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*##/\t/' | sort

init: ## Init/update all submodules
	git submodule update --init --recursive

pull: ## Advance every submodule to its remote main
	git submodule foreach 'git checkout main && git pull --ff-only origin main'

status: ## Short status across submodules
	git submodule foreach 'git status -s'

test: ## Run each repo's tests
	cd apps/zed-interfaces && cargo test
	cd apps/zed-cli && cargo test
	cd apps/zed-api-server.rs && cargo test --workspace
	cd apps/zed-web-server.rs && cargo test
	cd apps/zed-clients/rust && cargo test
	cd apps/zed-clients/typescript && npm install && npm run build && npm test
	cd apps/zed-clients/python && python3 -m unittest
	cd apps/zed-clients/go && go test ./...
	cd apps/zed-sync && npm install && npm run build && npm test

build: ## Build the Rust services and TS packages
	cd apps/zed-cli && cargo build --release
	cd apps/zed-api-server.rs && cargo build --release --workspace
	cd apps/zed-web-server.rs && cargo build --release
	cd apps/zed-clients/typescript && npm install && npm run build
	cd apps/zed-sync && npm install && npm run build

images: ## Build the api/web container images (context = apps/)
	docker build -f apps/zed-api-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-api-server:dev apps
	docker build -f apps/zed-web-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-web-server:dev apps

site: ## Build the marketing site
	cd apps/zed-pkg.github.io && npm install && npm run build
