APP := gdrive-optimize
CLI := cargo run -p $(APP) --

BLUE := \033[38;2;100;200;255m
AMBER := \033[38;2;255;180;100m
GREEN := \033[38;2;120;255;120m
RESET := \033[0m

.PHONY: help build build-release package-release install completions test test-unit test-integration test-functional test-acceptance test-doc test-all test-coverage lint fmt fmt-check clippy run sync report gdrive-sync fixtures-validate fixtures-update docs docs-serve clean clean-all setup check-deps

help:
	@printf "$(BLUE)%s$(RESET)\n" "gdrive-optimize developer targets"
	@printf "$(AMBER)%s$(RESET)\n" "Build"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "build" "cargo build --workspace"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "build-release" "cargo build --workspace --release"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "package-release" "archive the release binary into dist/"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "install" "cargo install --path crates/gdrive-optimize"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "completions" "generate bash/zsh/fish completions"
	@printf "$(AMBER)%s$(RESET)\n" "Test"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "test" "unit + integration + functional"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "test-acceptance" "cargo test -p gdrive-optimize --test acceptance_mock_end_to_end"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "test-doc" "cargo test --workspace --doc"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "test-coverage" "real workspace coverage with enforced threshold"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "test-all" "lint + test + acceptance + docs"
	@printf "$(AMBER)%s$(RESET)\n" "Run"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "run" "$(CLI) --help"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "sync" "$(CLI) sync"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "gdrive-sync" "$(CLI) db remote sync"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "report" "$(CLI) report all"
	@printf "$(AMBER)%s$(RESET)\n" "Docs and maintenance"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "docs" "validate docs files exist"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "check-deps" "verify cargo tooling"
	@printf "  $(GREEN)%-18s$(RESET) %s\n" "clean" "remove local build artifacts"

build:
	cargo build --workspace

build-release:
	cargo build --workspace --release

package-release: build-release
	@mkdir -p dist
	@version=$$(cargo pkgid -p $(APP) | sed 's/.*#//'); \
	platform=$$(uname -s | tr '[:upper:]' '[:lower:]'); \
	arch=$$(uname -m); \
	archive="dist/$(APP)-v$${version}-$${platform}-$${arch}.tar.gz"; \
	tar -czf "$$archive" -C target/release $(APP); \
	printf "%s\n" "$$archive"

install:
	cargo install --path crates/gdrive-optimize

completions:
	@mkdir -p dist/completions
	cargo run -p $(APP) -- completions bash > dist/completions/$(APP).bash
	cargo run -p $(APP) -- completions zsh > dist/completions/_$(APP)
	cargo run -p $(APP) -- completions fish > dist/completions/$(APP).fish

test: test-unit test-integration test-functional

test-unit:
	cargo test --workspace --lib

test-integration:
	cargo test -p gdrive-db --test db_integration
	cargo test -p gdrive-core --test sync_integration
	cargo test -p gdrive-db --test path_cache_integration

test-functional:
	cargo test -p gdrive-optimize --test cli_sync_functional
	cargo test -p gdrive-optimize --test cli_report_functional
	cargo test -p gdrive-optimize --test cli_find_functional
	cargo test -p gdrive-optimize --test cli_polish_functional
	cargo test -p gdrive-optimize --test cli_unshare_functional
	cargo test -p gdrive-optimize --test cli_trash_functional
	cargo test -p gdrive-optimize --test cli_db_remote_functional

test-acceptance:
	cargo test -p gdrive-optimize --test acceptance_mock_end_to_end

test-doc:
	cargo test --workspace --doc

test-all: lint test test-acceptance test-doc

test-coverage:
	cargo llvm-cov --version >/dev/null 2>&1 || cargo install cargo-llvm-cov --locked
	cargo llvm-cov --workspace --summary-only --fail-under-lines 85

lint: fmt-check clippy

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

run:
	$(CLI) --help

sync:
	$(CLI) sync

gdrive-sync:
	$(CLI) db remote sync

report:
	$(CLI) report all

fixtures-validate:
	cargo test -p gdrive-optimize --test fixtures_validate -- --nocapture

fixtures-update:
	@echo "Explicit snapshot/fixture refresh helper implemented in repo scripts"

docs:
	@test -f docs/architecture/overview.md
	@test -f docs/architecture/sync-engine.md
	@test -f docs/architecture/path-model.md
	@test -f docs/design/cli-ux.md
	@test -f docs/testing/strategy.md
	@test -f docs/operator/getting-started.md
	@test -f docs/operator/google-cloud-setup.md
	@test -f docs/operator/runbooks/monthly-cleanup.md
	@test -f docs/operator/runbooks/sharing-audit.md
	@test -f docs/operator/runbooks/revoked-token-recovery.md
	@test -f docs/operator/runbooks/invalid-page-token-recovery.md
	@test -f docs/operator/runbooks/scope-upgrade-prompts.md
	@test -f docs/testing/live-smoke.md

docs-serve:
	@echo "Static docs serving is not configured in Phase 0."

clean:
	rm -rf target coverage lcov.info

clean-all: clean
	rm -rf reports

setup: check-deps
	@echo "Phase 0 setup complete."

check-deps:
	cargo --version
	rustup --version
