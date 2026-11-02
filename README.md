# `knr` — Représentation Intermédiaire SSA Linéaire & Multi-Cibles

`knr` est un langage et une représentation intermédiaire (IR) en forme SSA (*Static Single Assignment*) pensée pour la génération et la vérification formelle de programmes système, réseau et interface utilisateur, sans Garbage Collector ni annotations de durée de vie complexes.

📄 **Spécification complète** : voir [`SPEC.md`](./SPEC.md).

---

## Caractéristiques Principales

- **Mémoire Linéaire Stricte (Zero-GC)** : Séparation claire entre types scalaires (`Copy` : `i64`, `f64`, `bool`) et types linéaires (`Move` : `buf`, `str`, `vec[T]`, `map[K, V]`, `chan[T]`), permettant les mutations sur place (*in-place mutation*) en toute sécurité.
- **Système de Capacités & Effets** : Contrôle statique des effets (`Net`, `Ui`, `Hw`, `Async`) déclarés dans la signature de chaque fonction.
- **Bijection Texte ↔ Binaire (`.ssa` ↔ `.knrbin`)** : Format texte plat lisible et encodage binaire compact sur mots de 64 bits (8 octets par instruction, en-tête `"KNIR"`).
- **Diagnostics Structurés en JSON** : Le vérificateur statique (`knr-verify`) retourne l'état exact des registres vivants et une suggestion de correction (`fix_hint`) directement exploitable par le tooling ou un agent.
- **Oracle VM à Compteur de Cycles (Gas Metering)** : Exécution déterministe bornée (`knr-interp`) avec traçage pas à pas (`--trace`).
- **Compilation AOT Multi-Cibles** : Génération de code natif via **Cranelift** et WebAssembly via **`wasm-encoder`**.

---

## Architecture du Workspace

| Crate | Chemin | Rôle |
| :--- | :--- | :--- |
| **`knr-core`** | `crates/knr-core` | AST SSA, 26 opcodes, système de types, parseur `.ssa` et codec binaire `.knrbin` |
| **`knr-verify`** | `crates/knr-verify` | Vérificateur statique en 4 passes (SSA, propriété linéaire, types, effets) + diagnostic JSON |
| **`knr-interp`** | `crates/knr-interp` | Machine virtuelle déterministe de référence avec compteur de cycles (*Gas Metering*) |
| **`knr-runtime`** | `crates/knr-runtime` | Trait `HostPlatform` et ponts d'exécution (`Net`/Tokio, `Ui`/VDOM, `Hw`/SIMD & MMIO) |
| **`knr-codegen`** | `crates/knr-codegen` | Backends de compilation AOT vers code natif (Cranelift) et WebAssembly (`wasm-encoder`) |
| **`knr-cli`** | `crates/knr-cli` | Interface en ligne de commande `knr` (`check`, `run`, `build`, `asm`, `disasm`) |

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
```

---

## Feuille de Route

- [ ] **Étape 1** : Initialisation du workspace Cargo & `knr-core` (AST, parseur `.ssa`, codec `.knrbin`)
- [ ] **Étape 2** : Vérificateur statique `knr-verify` (4 passes + diagnostics JSON + 20 tests unitaires)
- [ ] **Étape 3** : Machine virtuelle `knr-interp` & CLI `knr-cli` (`check`, `run`, `asm`, `disasm`)
- [ ] **Étape 4** : Ponts `knr-runtime` (`Net`, `Ui`, `Hw`)
- [ ] **Étape 5** : Compilation AOT `knr-codegen` (Cranelift & WASM) + fuzzing différentiel (`proptest`)
