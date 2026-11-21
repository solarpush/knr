# `knr` — Langage Linéaire Zero-GC, IR SSA Vérifiée, GPU UI & Multi-Cibles (v0.1.0)

`knr` est un langage de programmation complet (`.knr`) compilant vers une représentation intermédiaire SSA linéaire vérifiée statiquement (`.ssa` / `.knrbin`), conçu pour l'exécution système, réseau et graphique GPU **sans Garbage Collector ni annotations de durée de vie (`'a`)**, et pensé dès l'origine pour le tooling et les agents IA.

📄 **Spécification** : voir [`SPEC.md`](./SPEC.md) · 🤖 **Skill IA** : voir [`.agents/skills/knr-ssa/SKILL.md`](./.agents/skills/knr-ssa/SKILL.md).

---

## Installation & Mise à jour

Binaires précompilés disponibles pour **Linux (`x86_64`, `aarch64`)** et **macOS Apple Silicon (`aarch64`)** (inclut `knr` + `libknr_runtime.a` pour compiler en natif sans installer Rust) :

```bash
# Installer la dernière version publiée sur GitHub Releases (dans ~/.local/bin)
curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | sh

# Installer une version précise (ex: v0.1.0) ou choisir le dossier d'installation
curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | KNR_VERSION=v0.1.0 KNR_INSTALL_DIR="$HOME/.local/bin" sh

# Vérifier les mises à jour et mettre à jour depuis la CLI
knr update --check
knr update
```

---

## Caractéristiques Clés (v0.1.0)

- **Source ergonomique `.knr` → IR SSA canonique `.ssa` / `.knrbin`** : Boucles `while` (`break`/`continue`), variables mutables par paramètres de blocs SSA, `schema` / `enum` + `match` exhaustif, `if`/`else` expressions, spread de structs (`Style { bg: #111827, ..style_card() }`), littéraux couleur `#hex`, et imports multi-fichiers `.knr`.
- **Sécurité Mémoire Sans GC ni Lifetimes** : Propriété linéaire unique + auto-drop déterministe en fin de scope + **emprunts de seconde classe (`&T`)** qui permettent de lire une structure sans la déplacer et interdisent statiquement toute fuite (`BorrowEscape`) en moins de 100 µs.
- **Système d'Effets Explicite** : Contrôle statique et transitif des capacités (`Net`, `Ui`, `Hw`, `Async`).
- **Bibliothèques Standard Intégrées (`std/*`)** : Intégrées directement dans `knr-core` (aucun chemin relatif requis) :
  - `import "std/ui";` — Primitives UI déclaratives non contraintes (`Widget`, `Style` 40 champs, `Color`, `Vec2`, Flex/Grid, dégradés, ombres, `backdrop_blur`).
  - `import "std/anim";` — Courbes d'easing, interpolation et amortissement purs pour animations 60/120 FPS.
  - `import "std/math";` — Fonctions mathématiques 2D/3D et primitives SDF à parité CPU/GPU (`sin`, `cos`, `sqrt`, `atan2`, `smoothstep`, `smin`, `clamp`, `lerp`/`mix`).
  - `import "std/sys";` — Primitives système natives : système de fichiers (lecture/écriture), JSON et client HTTP.
- **UI GPU Non Contrainte (`wgpu 23`) & Compilateur de Shaders `.knr` (`shader fn`)** :
  - **Moteur GPU natif (`wgpu 23`)** : Rendu Vulkan / Metal / DX12 avec quads SDF analytiques, `backdrop_blur` Dual Kawase et atlas de glyphes GPU, avec **fallback CPU automatique** (`softbuffer` + `tiny-skia` + `ab_glyph`) et mode headless déterministe (`KNR_HEADLESS=1`).
  - **Shaders GPU écrits en pur `.knr`** : Déclarez `shader fn my_fx(uv: Vec2, time: f64, ...) -> Color`, vérifiés GPU-purs par `knr-verify` puis compilés et validés par **`naga`** vers **WGSL** (Desktop `wgpu` + WebGPU) et **GLSL ES 3.00** (WebGL2 universel dans le navigateur).
- **Compilation Multi-Cibles (Parité VM == Natif Cranelift == WebAssembly == Web)** :
  - **Binaires natifs autonomes (`--target native` et `--target native --gui`)** : Compilés AOT via Cranelift et liés statiquement contre `libknr_runtime.a` (serveurs HTTP Tokio, outils CLI/système et applications Desktop autonomes).
  - **WebAssembly autonome (`--target wasm`)** : Allocateur linéaire, structs, chaînes et vecteurs embarqués dans le `.wasm` (**zéro import JS requis**).
  - **Applications Web complètes (`--target web`)** : Exporte `index.html` (SSR initial + pont WebGPU/WebGL2) + `app.wasm` autonome (zéro HTML/CSS/GLSL écrit à la main).
- **Hot-Reload Live, LSP & Extensions Éditeurs** :
  - **Hot-reload Desktop & Shaders (`knr dev app.knr --gui`)** : Recompilation et mise à jour instantanées de l'UI MVU et des shaders WGSL à chaque sauvegarde.
  - **Serveur LSP intégré (`knr lsp`)** : Diagnostics temps réel, complétion (`std/*`, champs `Style`/`Widget`, mots-clés), hover, go-to-definition et symboles sur stdio (JSON-RPC 2.0).
  - **Extensions éditeurs** : Support prêt à l'emploi pour VS Code ([`editors/vscode`](./editors/vscode)), Neovim ([`editors/neovim`](./editors/neovim)) et Zed ([`editors/zed`](./editors/zed)).
- **Outillage AI-First** : `knr explore` (`--format ai|json|mermaid`), `knr check --json`, `knr test --json`, `knr skill`, `knr asm`/`disasm`.

---

## Démarrage Rapide

```bash
# 1. Vérifier statiquement un projet multi-fichiers .knr (diagnostics JSON optionnels)
knr check examples/counter_app.knr --json

# 2. Exécuter les tests unitaires déterministes (sans navigateur ni réseau)
knr test examples/counter_app.knr

# 3. Lancer une app GPU UI MVU + shaders en fenêtre native Desktop (wgpu + fallback CPU)
knr run examples/raymarch_3d.knr --gui

# 4. Développer en live avec hot-reload UI & shaders WGSL
knr dev examples/shader_studio.knr --gui

# 5. Compiler en binaire natif autonome (CLI / serveur HTTP ou application Desktop GUI)
knr build examples/http_server.knr --target native -o ./http_server
knr build examples/raymarch_3d.knr --target native --gui -o ./raymarch_3d

# 6. Exporter en WebAssembly zéro-import ou application Web complète (SSR + WASM + WebGPU/WebGL2)
knr build examples/counter_app.knr --target wasm -o counter.wasm
knr build site/site.knr --target web -o dist_site/

# 7. Explorer l'architecture et le graphe d'appels d'un projet (format AI / JSON / Mermaid)
knr explore examples/raymarch_3d.knr --format ai
```

---

## Exemples Inclus

| Exemple | Description | Commandes |
| :--- | :--- | :--- |
| [`examples/counter_app.knr`](./examples/counter_app.knr) | App MVU interactive (`UiNode`, emprunts `&State`, tests déterministes) | `knr run examples/counter_app.knr --gui` · `knr test examples/counter_app.knr` |
| [`examples/http_server.knr`](./examples/http_server.knr) | Serveur HTTP SSR multi-fichiers compilé en binaire natif autonome (Cranelift + Tokio) | `knr build examples/http_server.knr --target native -o ./http_server` |
| [`examples/shader_studio.knr`](./examples/shader_studio.knr) | Studio de shaders GPU 2D en pur `.knr` (`aurora_waves`, `plasma_orbit` → WGSL + WebGL2) | `knr dev examples/shader_studio.knr --gui` · `knr build examples/shader_studio.knr --target web -o dist_shader/` |
| [`examples/raymarch_3d.knr`](./examples/raymarch_3d.knr) | Démo flagship de Raymarching SDF 3D temps réel (`std/ui` + `std/math` + `shader fn`) | `knr run examples/raymarch_3d.knr --gui` · `knr build examples/raymarch_3d.knr --target native --gui -o ./raymarch_3d` |
| [`examples/sys_demo.knr`](./examples/sys_demo.knr) | Démonstration `std/sys` : système de fichiers, parsing/sérialisation JSON et client HTTP | `knr run examples/sys_demo.knr` · `knr build examples/sys_demo.knr --target native -o ./sys_demo` |
| [`site/site.knr`](./site/site.knr) | Site vitrine officiel de `knr` avec shader GPU `knr_hero_nebula` (zéro HTML/CSS/GLSL) | `knr run site/site.knr --gui` · `knr build site/site.knr --target web -o dist_site/` |

---

## Support Éditeurs & Serveur LSP (`knr lsp`)

`knr` embarque un serveur Language Server Protocol (`knr lsp`) sans dépendance externe, utilisable directement avec les extensions fournies :

- **VS Code** : [`editors/vscode`](./editors/vscode) — Coloration syntaxique TextMate `.knr`/`.ssa`, snippets et client LSP automatique.
- **Neovim** : [`editors/neovim`](./editors/neovim) — Configuration `nvim-lspconfig` / `vim.lsp` et détection filetype `.knr`.
- **Zed** : [`editors/zed`](./editors/zed) — Extension langage `.knr` connectée à `knr lsp`.

---

## Architecture du Workspace

| Crate | Chemin | Rôle |
| :--- | :--- | :--- |
| **`knr-core`** | `crates/knr-core` | Syntaxe `.knr`, bibliothèques standard intégrées (`std/ui`, `std/anim`, `std/math`, `std/sys`), AST SSA (`&T`, `enum`/`match`, `shader fn`), abaissement, `explore`, codec binaire `.knrbin` |
| **`knr-verify`** | `crates/knr-verify` | Vérificateur statique en 4 passes (SSA, propriété linéaire + emprunts `&T`, types, effets, pureté GPU `shader fn`) + diagnostics JSON |
| **`knr-interp`** | `crates/knr-interp` | Machine virtuelle déterministe de référence avec gas metering et traces pas à pas |
| **`knr-runtime`** | `crates/knr-runtime` | Runtime Rust (`rlib` + `staticlib` C-ABI sans `wgpu` par défaut) : `Net` (Tokio), `Hw`, `Sys` (FS/JSON/HTTP), moteur GPU `wgpu 23` (feature `gpu`) + fallback CPU `winit`/`softbuffer`/`tiny-skia`/`ab_glyph` + headless (`KNR_HEADLESS=1`) |
| **`knr-codegen`** | `crates/knr-codegen` | Backends AOT & JIT Cranelift natif, WebAssembly autonome (`wasm-encoder`), et compilateur de shaders `.knr` → WGSL / GLSL ES 3.00 validés par `naga` |
| **`knr-cli`** | `crates/knr-cli` | CLI unifiée `knr` (`check`, `test`, `run`, `build`, `dev`, `lsp`, `explore`, `asm`, `disasm`, `init`, `new`, `skill`, `update`) |
