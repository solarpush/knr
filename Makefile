# ============================================================================
# knr v0.4.0 Developer & Install Makefile
# ============================================================================

PREFIX      ?= $(HOME)/.local
BINDIR      ?= $(PREFIX)/bin
VSCODE_EXT  ?= $(HOME)/.vscode/extensions/knr-lang
CARGO       ?= cargo
KNR_BIN     := target/release/knr

.DEFAULT_GOAL := help

.PHONY: help build release install install-debug install-vscode install-all \
        test clippy check-all site dev-site dev-studio dev-tasks dev-raymarch serve-site \
        render-site render-studio render-tasks doctor bench mcp-info clean

help: ## Show available targets
	@echo "knr v0.4.0 Makefile targets:"
	@echo "  make install         Build release binary and atomically install into $(BINDIR)/knr"
	@echo "  make install-debug   Build dev binary and atomically install into $(BINDIR)/knr (faster)"
	@echo "  make install-vscode  Symlink editors/vscode into $(VSCODE_EXT)"
	@echo "  make install-all     Install both release CLI binary and VS Code extension"
	@echo "  make test            Run full workspace test suite (headless)"
	@echo "  make clippy          Run clippy (-D warnings) across default, no-default, and gpu"
	@echo "  make check-all       Run full test suite + all clippy checks"
	@echo "  make doctor          Run knr environment & toolchain health diagnostics"
	@echo "  make bench           Benchmark site/app.tsk & examples/neural_studio/src/app.tsk"
	@echo "  make mcp-info        Print ready-to-paste MCP server config for AI editors/agents"
	@echo "  make site            Rebuild site/ Web Canvas+WebGPU+WASM bundle & audit"
	@echo "  make dev-studio      Launch examples/neural_studio/src/app.tsk in Desktop GPU window"
	@echo "  make dev-site        Launch site/app.tsk in native Desktop GPU window (hot-reload)"
	@echo "  make dev-tasks       Launch examples/task_studio.tsk in Desktop GPU window"
	@echo "  make dev-raymarch    Launch examples/raymarch_3d.tsk in Desktop GPU window"
	@echo "  make serve-site      Serve site/ locally at http://localhost:8080"
	@echo "  make render-studio   Run Neural Studio flow.json E2E + multi-viewport audit"
	@echo "  make render-site     Render site/app.tsk to /tmp/knr_site.png with --assert-audit"
	@echo "  make render-tasks    Render examples/task_studio.tsk flow to /tmp/knr_tasks.png"
	@echo "  make clean           Remove target/ artifacts"

build: ## Build dev binary
	$(CARGO) build -p knr-cli --bin knr

release: ## Build optimized release binary (+ staticlib runtime for native AOT builds)
	$(CARGO) build --release -p knr-runtime --lib
	$(CARGO) build --release -p knr-cli --bin knr

install: release ## Atomically install release `knr` binary (safe even while `knr lsp` is running)
	@mkdir -p "$(BINDIR)"
	cp "$(KNR_BIN)" "$(BINDIR)/knr.new"
	chmod +x "$(BINDIR)/knr.new"
	mv -f "$(BINDIR)/knr.new" "$(BINDIR)/knr"
	@if [ -f target/release/libknr_runtime.a ]; then \
		cp target/release/libknr_runtime.a "$(BINDIR)/libknr_runtime.a.new" && \
		mv -f "$(BINDIR)/libknr_runtime.a.new" "$(BINDIR)/libknr_runtime.a"; \
	fi
	@echo "Installed $$("$(BINDIR)/knr" --version) -> $(BINDIR)/knr"

install-debug: build ## Fast atomic install of dev `knr` binary
	@mkdir -p "$(BINDIR)"
	cp target/debug/knr "$(BINDIR)/knr.new"
	chmod +x "$(BINDIR)/knr.new"
	mv -f "$(BINDIR)/knr.new" "$(BINDIR)/knr"
	@echo "Installed $$("$(BINDIR)/knr" --version) (dev) -> $(BINDIR)/knr"

install-vscode: ## Symlink first-party VS Code `.tsk` extension
	@mkdir -p "$$(dirname "$(VSCODE_EXT)")"
	ln -sfn "$(CURDIR)/editors/vscode" "$(VSCODE_EXT)"
	@echo "Linked $(CURDIR)/editors/vscode -> $(VSCODE_EXT)"
	@echo "Run 'Developer: Reload Window' in VS Code to activate."

install-all: install install-vscode ## Install CLI binary + VS Code extension

test: ## Run full workspace test suite
	KNR_HEADLESS=1 $(CARGO) test --workspace -- --test-threads=4

clippy: ## Run strict clippy across all feature sets
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) clippy -p knr-runtime --no-default-features -- -D warnings
	$(CARGO) clippy -p knr-runtime --features gpu --all-targets -- -D warnings

check-all: test clippy ## Run all tests and clippy checks

doctor: release ## Run toolchain environment and project health checks
	$(KNR_BIN) doctor

bench: release ## Run VM + Cranelift JIT + artifact size benchmarks
	$(KNR_BIN) bench site/app.tsk
	$(KNR_BIN) bench examples/neural_studio/src/app.tsk

mcp-info: ## Print ready-to-paste JSON config to register `knr mcp` in AI agents
	@echo 'Register the built-in knr MCP server (8 native tools) in Claude Code, Cursor, Windsurf, or Antigravity:'
	@echo ''
	@echo '{'
	@echo '  "mcpServers": {'
	@echo '    "knr": {'
	@echo '      "command": "knr",'
	@echo '      "args": ["mcp"]'
	@echo '    }'
	@echo '  }'
	@echo '}'

site: release ## Rebuild site/ web bundle and verify check + test + visual audit
	$(KNR_BIN) check site/app.tsk --json
	$(KNR_BIN) test site/app.tsk
	$(KNR_BIN) build site/app.tsk --target web -o site/
	$(KNR_BIN) render site/app.tsk --width 1280 --height 1450 --scale 1.5 --assert-audit

dev-studio: ## Launch examples/neural_studio/src/app.tsk with live GPU hot-reload
	$(BINDIR)/knr dev examples/neural_studio/src/app.tsk --gui

dev-site: ## Launch site/app.tsk with live GPU hot-reload
	$(BINDIR)/knr dev site/app.tsk --gui

dev-tasks: ## Launch examples/task_studio.tsk with live GPU hot-reload
	$(BINDIR)/knr dev examples/task_studio.tsk --gui

dev-raymarch: ## Launch examples/raymarch_3d.tsk with live GPU hot-reload
	$(BINDIR)/knr dev examples/raymarch_3d.tsk --gui

serve-site: ## Serve compiled site/ web bundle on http://localhost:8080
	@echo "Serving site/ at http://localhost:8080"
	python3 -m http.server 8080 -d site

render-studio: release ## Run Neural Studio E2E flow.json + mobile/desktop visual audit
	$(KNR_BIN) render examples/neural_studio/src/app.tsk \
		--flow examples/neural_studio/flow.json -o /tmp/knr_neural_studio.png --assert-audit
	$(KNR_BIN) render examples/neural_studio/src/app.tsk \
		--viewports mobile,desktop -o /tmp/knr_neural_studio_vp.png --assert-audit

render-site: release ## Render site/app.tsk snapshot to /tmp/knr_site.png
	$(KNR_BIN) render site/app.tsk -o /tmp/knr_site.png --width 1280 --height 1450 --scale 1.5 --assert-audit

render-tasks: release ## Render examples/task_studio.tsk interactive flow to /tmp/knr_tasks.png
	$(KNR_BIN) render examples/task_studio.tsk \
		--input task_draft_input="Ship Agentic Flutter v0.4.0" \
		--click btn_add_task --click btn_complete_next --click btn_filter_open \
		-o /tmp/knr_tasks.png --width 1280 --height 860 --scale 1.5 --assert-audit

clean: ## Clean Cargo build directory
	$(CARGO) clean
