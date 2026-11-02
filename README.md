# `knr` — Représentation Intermédiaire SSA Linéaire & Multi-Cibles

`knr` est un langage et une représentation intermédiaire (IR) en forme SSA (*Static Single Assignment*) pensée pour la génération et la vérification formelle de programmes système, réseau et interface utilisateur, sans Garbage Collector ni annotations de durée de vie complexes.

📄 **Spécification complète** : voir [`SPEC.md`](./SPEC.md).

---

## Installation Rapide (`install.sh`) & Mise à jour (`knr update`)

### 1. Installer via `install.sh` (Linux & macOS — `x86_64` / `aarch64`)
```bash
# Installer la dernière version publiée sur GitHub Releases (dans ~/.local/bin)
curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | sh

# Installer une version précise (ex: v0.1.0) ou choisir le dossier d'installation
curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | KNR_VERSION=v0.1.0 KNR_INSTALL_DIR="$HOME/.local/bin" sh
```

### 2. Mettre à jour depuis la CLI (`knr update`)
```bash
# Vérifier si une nouvelle release est disponible sur GitHub sans l'installer
knr update --check

# Télécharger et remplacer automatiquement le binaire par la dernière release
knr update

# Cibler une version précise ou forcer la réinstallation
knr update --version v0.1.0
knr update --force
```

---

## Caractéristiques Principales

- **Mémoire Linéaire Stricte (Zero-GC)** : Séparation claire entre types scalaires (`Copy` : `i64`, `f64`, `bool`) et types linéaires (`Move` : `buf`, `str`, `vec[T]`, `map[K, V]`, `chan[T]`, `struct[Name]`), permettant les mutations sur place (*in-place mutation*) en toute sécurité.
- **Schémas Déclaratifs (`schema` / `struct`)** : Structures typées par index (`struct_new`, `struct_get`, `struct_set`) vérifiées statiquement sans coût d'introspection dynamique.
- **Système de Capacités & Effets** : Contrôle statique des effets (`Net`, `Ui`, `Hw`, `Async`) déclarés dans la signature de chaque fonction.
- **Bijection Texte ↔ Binaire (`.ssa` ↔ `.knrbin`)** : Format texte plat lisible et encodage binaire compact sur mots de 64 bits (8 octets par instruction, en-tête `"KNIR"`).
- **Diagnostics Structurés en JSON** : Le vérificateur statique (`knr-verify`) retourne l'état exact des registres vivants et une suggestion de correction (`fix_hint`) directement exploitable par le tooling ou un agent.
- **Oracle VM à Compteur de Cycles (Gas Metering)** : Exécution déterministe bornée (`knr-interp`) avec traçage pas à pas (`--trace`).
- **Compilation AOT Multi-Cibles** : Génération de code natif via **Cranelift** et WebAssembly via **`wasm-encoder`**.

---

## Architecture du Workspace

| Crate | Chemin | Rôle |
| :--- | :--- | :--- |
| **`knr-core`** | `crates/knr-core` | AST SSA, 29 opcodes, `schema`/`struct`, système de types, parseur `.ssa` et codec binaire `.knrbin` |
| **`knr-verify`** | `crates/knr-verify` | Vérificateur statique en 4 passes (SSA, propriété linéaire, types, effets) + diagnostic JSON |
| **`knr-interp`** | `crates/knr-interp` | Machine virtuelle déterministe de référence avec compteur de cycles (*Gas Metering*) |
| **`knr-runtime`** | `crates/knr-runtime` | Trait `HostPlatform` et ponts d'exécution (`Net`/Tokio, `Ui`/VDOM, `Hw`/SIMD & MMIO) |
| **`knr-codegen`** | `crates/knr-codegen` | Backends de compilation AOT vers code natif (Cranelift) et WebAssembly (`wasm-encoder`) |
| **`knr-cli`** | `crates/knr-cli` | Interface en ligne de commande `knr` (`check`, `run`, `build`, `asm`, `disasm`, `update`) |

---

## Commandes CLI (`knr`)

```bash
# 1. Vérifier statiquement un programme SSA (sortie JSON optionnelle)
knr check programme.ssa --json

# 2. Exécuter dans la VM de référence avec limite de gaz et trace
knr run programme.ssa --gas 1000000 --trace

# 3. Compiler en binaire natif ou module WebAssembly
knr build programme.ssa --target native -o programme
knr build programme.ssa --target wasm -o programme.wasm

# 4. Assembler en bytecode binaire (.knrbin) et désassembler (.ssa)
knr asm programme.ssa -o programme.knrbin
knr disasm programme.knrbin

# 5. Lancer le serveur SSR auto-hébergé du site en knr (port 8080)
knr run site/server.ssa --gas 100000000
```

---

## Feuille de Route

- [x] **Étape 1** : Initialisation du workspace Cargo & `knr-core` (AST, parseur `.ssa`, codec `.knrbin`, `schema`/`struct`)
- [x] **Étape 2** : Vérificateur statique `knr-verify` (4 passes + diagnostics JSON + tests unitaires)
- [x] **Étape 3** : Machine virtuelle `knr-interp` & CLI `knr-cli` (`check`, `run`, `asm`, `disasm`, `update`)
- [x] **Étape 4** : Ponts `knr-runtime` (`Net`, `Ui`, `Hw`, `Async`)
- [x] **Étape 5** : Compilation AOT `knr-codegen` (Cranelift & WASM) + fuzzing différentiel (`proptest`)
