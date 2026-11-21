# `knr` — Langage Linéaire Zero-GC, IR SSA Vérifiée & Multi-Cibles (Natif, Desktop, Web/WASM)

`knr` est un langage de programmation complet (`.knr`) compilant vers une représentation intermédiaire SSA linéaire vérifiée statiquement (`.ssa` / `.knrbin`), conçu pour l'exécution système, réseau et graphique **sans Garbage Collector ni annotations de durée de vie (`'a`)**, et pensé dès l'origine pour le tooling et les agents IA.

📄 **Spécification** : voir [`SPEC.md`](./SPEC.md) · 🤖 **Skill IA** : voir [`.agents/skills/knr-ssa/SKILL.md`](./.agents/skills/knr-ssa/SKILL.md).

---

## Caractéristiques Clés

- **Source ergonomique `.knr` -> IR SSA canonique `.ssa` / `.knrbin`** : Boucles `while` (`break`/`continue`), variables mutables par paramètres de blocs SSA, `enum` + `match` exhaustif, `if`/`else` expressions, `&&`/`||`, `%`, chaînes (`+`, `==`, `to_str`, `str_len`), et imports multi-fichiers `.knr`.
- **Sécurité Mémoire Sans GC ni Lifetimes** : Propriété linéaire unique + auto-drop déterministe en fin de scope + **emprunts de seconde classe (`&T`)** qui permettent de lire une structure sans la déplacer et interdisent statiquement toute fuite (`BorrowEscape`) en moins de 100 µs.
- **Système d'Effets Explicite** : Contrôle statique et transitif des capacités (`Net`, `Ui`, `Hw`, `Async`).
- **Parité Totale des Cibles (VM == Natif Cranelift == WebAssembly)** :
  - **Natif AOT (`--target native`)** : Compilé via Cranelift et lié contre le vrai runtime Rust `libknr_runtime.a` (serveurs HTTP Tokio natifs, structs, strings, channels).
  - **WebAssembly autonome (`--target wasm`)** : Allocateur linéaire, structs, strings et vecteurs embarqués dans le `.wasm` (**zéro import JS requis** pour les apps pures & UI).
- **UI Déclarative Typée MVU (Desktop Natif + Web/WASM)** :
  - Écrivez une seule app `init` / `update` / `view` avec des nœuds `UiNode` typés (zéro HTML/CSS brut).
  - **Desktop natif (`knr run app.knr --gui`)** : Fenêtre multi-plateforme (`winit` + `softbuffer` + `tiny-skia` + `ab_glyph`, Wayland / X11 / macOS / Windows) avec moteur Flexbox et gestion souris/clavier + fallback headless déterministe.
  - **Web/WASM (`knr build app.knr --target web -o dist/`)** : Exporte `index.html` (SSR initial + pont générique ~90 lignes) + `app.wasm` autonome.
- **Outillage AI-First** : `knr explore`, `knr check --json`, `knr test --json`, `knr dev`, `knr asm`/`disasm`.

---

## Démarrage Rapide

```bash
# 1. Vérifier statiquement un projet multi-fichiers .knr (diagnostics JSON optionnels)
knr check examples/counter_app.knr --json

# 2. Exécuter les tests unitaires déterministes (sans navigateur ni réseau)
knr test examples/counter_app.knr

# 3. Lancer l'app UI MVU en fenêtre native Desktop (Wayland / X11 / macOS / Windows)
knr run examples/counter_app.knr --gui

# 4. Exporter l'app UI MVU en site Web + WebAssembly autonome
knr build examples/counter_app.knr --target web -o dist/

# 5. Compiler un vrai serveur HTTP multi-fichiers .knr en binaire natif ELF autonome
knr build examples/http_server.knr --target native -o ./http_server
./http_server

# 6. Explorer l'architecture et le graphe d'appels d'un projet
knr explore examples/counter_app.knr --format ai
```

---

## Architecture du Workspace

| Crate | Chemin | Rôle |
| :--- | :--- | :--- |
| **`knr-core`** | `crates/knr-core` | Syntaxe `.knr`, AST SSA (paramètres de blocs, `&T`, `enum`/`match`), abaissement, `explore`, codec binaire `.knrbin` |
| **`knr-verify`** | `crates/knr-verify` | Vérificateur statique en 4 passes (SSA, propriété linéaire + emprunts `&T`, types, effets) + diagnostics JSON |
| **`knr-interp`** | `crates/knr-interp` | Machine virtuelle déterministe de référence avec gas metering et traces pas à pas |
| **`knr-runtime`** | `crates/knr-runtime` | Runtime Rust (`rlib` + `staticlib` C-ABI) : `Net` (Tokio), `Hw`, `UiNode` Flexbox + `tiny-skia` + `ab_glyph` + `winit`/`softbuffer` |
| **`knr-codegen`** | `crates/knr-codegen` | Backends AOT & JIT Cranelift natif + WebAssembly autonome (`wasm-encoder`) |
| **`knr-cli`** | `crates/knr-cli` | CLI unifiée `knr` (`check`, `test`, `run`, `build`, `explore`, `dev`, `asm`, `disasm`, `init`, `new`, `skill`, `update`) |
