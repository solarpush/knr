# Spécification Technique — `knr` (SSA Intermediate Representation)

> Document de référence pour l'architecture, le système de types linéaires, le jeu d'instructions SSA, le vérificateur statique, la machine virtuelle et les backends de compilation du langage `knr`.

---

## 1. Architecture du Monorepo Rust (Workspace Cargo)

Le projet est organisé sous forme d'un workspace Cargo isolant chaque responsabilité dans une crate dédiée :

```text
.
├── Cargo.toml                 # Configuration du Workspace
├── README.md                  # Présentation générale et guide de démarrage
├── SPEC.md                    # Ce document de référence
├── crates/
│   ├── knr-core/              # AST SSA, Opcodes, Types, Format Binaire/Texte
│   ├── knr-verify/            # Vérificateur de types, mémoire linéaire et effets
│   ├── knr-interp/            # VM déterministe à compteur de cycles (Oracle)
│   ├── knr-runtime/           # Ordonnanceur async et ponts (Net, UI, Hardware)
│   ├── knr-codegen/           # Backends Cranelift (Natif) et WebAssembly
│   └── knr-cli/               # Binaire CLI `knr` (check, run, build, asm, disasm)
└── tests/
    └── fixtures/              # Programmes .ssa de test (valides et invalides)
```

### Configuration `Cargo.toml` racine

```toml
[workspace]
resolver = "2"
members = [
    "crates/knr-core",
    "crates/knr-verify",
    "crates/knr-interp",
    "crates/knr-runtime",
    "crates/knr-codegen",
    "crates/knr-cli",
]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
cranelift = "0.112"
cranelift-module = "0.112"
cranelift-object = "0.112"
wasm-encoder = "0.217"
tokio = { version = "1.40", features = ["full"] }
proptest = "1.5"
```

---

## 2. Spécification du Noyau SSA et des Types (`crates/knr-core`)

### 2.1 Système de Types et Règles de Mémoire Linéaire

Le langage `knr` se passe totalement de Garbage Collector et d'annotations de durée de vie complexes en séparant les types en deux catégories :

#### Types Scalaires (`Copy` — copiés implicitement)
- `i64` : Entier signé 64 bits.
- `f64` : Flottant 64 bits IEEE-754.
- `bool` : Booléen (`true` / `false`).

#### Types Linéaires (`Move` — possédés par un seul registre à la fois)
- `buf` : Tranche d'octets contiguë alignée en mémoire (réseau / hardware zero-copy).
- `str` : Chaîne UTF-8 immuable.
- `vec[T]` : Tableau dynamique typé.
- `map[K, V]` : Dictionnaire clé/valeur ordonné (utilisé pour l'état et l'arbre UI/DOM).
- `chan[T]` : Canal de communication asynchrone typé.

#### Règle d'or de la mémoire linéaire
Chaque instruction est classée selon qu'elle **emprunte** un registre (**Borrow** : lecture seule, le registre reste vivant après la ligne) ou qu'elle le **consomme** (**Consume** : transfert de propriété, le registre d'entrée devient invalide après la ligne).

Tout registre linéaire encore vivant à la fin d'une fonction sans avoir été retourné ou envoyé dans un canal doit être détruit par `free` (ou libéré automatiquement par insertion du compilateur).

---

### 2.2 Table Complète des 26 Opcodes

| Catégorie | Opcode | Entrées | Sortie | Sémantique Mémoire |
| :--- | :--- | :--- | :--- | :--- |
| **Constantes** | `const_i64` | Littéral entier | `i64` | Crée un scalaire |
| **Constantes** | `const_f64` | Littéral flottant | `f64` | Crée un scalaire |
| **Constantes** | `const_bool` | Littéral booléen | `bool` | Crée un scalaire |
| **Constantes** | `const_str` | Littéral texte | `str` | Alloue un type linéaire |
| **Arithmétique** | `add` / `sub` | `a: i64/f64`, `b: i64/f64` | `i64/f64` | Lecture `Copy` |
| **Arithmétique** | `mul` / `div` | `a: i64/f64`, `b: i64/f64` | `i64/f64` | Lecture `Copy` |
| **Comparaison** | `cmp_eq` / `cmp_lt` | `a: T`, `b: T` | `bool` | Emprunte (`Borrow`) `a` et `b` |
| **Buffers (HW)** | `buf_alloc` | `size: i64` | `buf` | Alloue un bloc d'octets aligné |
| **Buffers (HW)** | `buf_read_u8` | `b: buf`, `idx: i64` | `i64` | Emprunte (`Borrow`) `b` |
| **Buffers (HW)** | `buf_write_u8` | `b: buf`, `idx: i64`, `v: i64` | `buf` | Consomme `b`, retourne `b` muté |
| **Buffers (HW)** | `buf_slice` | `b: buf`, `start: i64`, `len: i64` | `buf` | Emprunte ou découpe sans copie |
| **Tableaux** | `vec_new` | Type `T` | `vec[T]` | Alloue un tableau vide |
| **Tableaux** | `vec_push` | `v: vec[T]`, `item: T` | `vec[T]` | Consomme `v` et `item`, retourne `v` |
| **Tableaux** | `vec_get` | `v: vec[T]`, `idx: i64` | `T` | Emprunte `v` |
| **Tableaux** | `vec_len` | `v: vec[T]` | `i64` | Emprunte `v` |
| **Maps (UI/JS)** | `map_new` | Types `K`, `V` | `map[K, V]` | Alloue un dictionnaire vide |
| **Maps (UI/JS)** | `map_set` | `m: map[K, V]`, `k: K`, `v: V` | `map[K, V]` | Consomme `m`, `k`, `v`, retourne `m` |
| **Maps (UI/JS)** | `map_get` | `m: map[K, V]`, `k: K` | `V` | Emprunte `m` et `k` |
| **Maps (UI/JS)** | `map_del` | `m: map[K, V]`, `k: K` | `map[K, V]` | Consomme `m`, emprunte `k` |
| **Concurrence** | `chan_new` | Type `T`, `cap: i64` | `chan[T]` | Effet `Async` : crée un canal |
| **Concurrence** | `chan_send` | `c: chan[T]`, `val: T` | `bool` | Emprunte `c`, consomme `val` |
| **Concurrence** | `chan_recv` | `c: chan[T]` | `T` | Emprunte `c`, produit `val` linéaire |
| **Concurrence** | `spawn` | `fn_id`, `arg: chan[T]` | `bool` | Effet `Async` : lance une tâche |
| **Système/Flux** | `syscall` | `effect_id`, `payload: T` | `R` | Appelle un pont hôte (`Net`, `UI`, `HW`) |
| **Système/Flux** | `free` | `reg: T` | `void` | Consomme et désalloue explicitement |
| **Système/Flux** | `br` / `br_if` / `ret` | Labels / Registre | `void` | Saut inconditionnel, conditionnel, retour |

---

### 2.3 Structures Rust Fondamentales (`crates/knr-core/src/lib.rs`)

```rust
use serde::{Deserialize, Serialize};

pub type RegId = u32;
pub type BlockId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    I64,
    F64,
    Bool,
    Str,
    Buf,
    Vec(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Chan(Box<Type>),
    Void,
}

impl Type {
    pub fn is_linear(&self) -> bool {
        !matches!(self, Type::I64 | Type::F64 | Type::Bool | Type::Void)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    Net,
    Ui,
    Hw,
    Async,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    ConstI64(i64),
    ConstF64(f64),
    ConstBool(bool),
    ConstStr(String),
    Add(RegId, RegId),
    Sub(RegId, RegId),
    Mul(RegId, RegId),
    Div(RegId, RegId),
    CmpEq(RegId, RegId),
    CmpLt(RegId, RegId),
    BufAlloc(RegId),
    BufReadU8(RegId, RegId),
    BufWriteU8(RegId, RegId, RegId),
    BufSlice(RegId, RegId, RegId),
    VecNew(Type),
    VecPush(RegId, RegId),
    VecGet(RegId, RegId),
    VecLen(RegId),
    MapNew(Type, Type),
    MapSet(RegId, RegId, RegId),
    MapGet(RegId, RegId),
    MapDel(RegId, RegId),
    ChanNew(Type, RegId),
    ChanSend(RegId, RegId),
    ChanRecv(RegId),
    Spawn(String, RegId),
    Syscall(Effect, String, RegId),
    Free(RegId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instruction {
    pub dst: Option<RegId>,
    pub ty: Type,
    pub op: Op,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Terminator {
    Ret(Option<RegId>),
    Br(BlockId),
    BrIf { cond: RegId, then_blk: BlockId, else_blk: BlockId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub params: Vec<(RegId, Type)>,
    pub ret_ty: Type,
    pub effects: Vec<Effect>,
    pub blocks: Vec<BasicBlock>,
}
```

---

### 2.4 Encodage Binaire Compact (Bijection Texte ↔ Binaire)

Chaque fichier texte `.ssa` possède une traduction binaire stricte (`.knrbin`) garantissant une bijection 1:1 :

1. **En-tête magique** : 4 octets (`0x4B 0x4E 0x49 0x52` = `"KNIR"`), suivi de la version (`u16`).
2. **Section Pool de Constantes** : table indexée des chaînes UTF-8 et flottants.
3. **Section Instructions** : chaque instruction est encodée sur un mot fixe de 64 bits (8 octets) :
   - **8 bits** : Code de l'Opcode (`0x01` à `0x1A`).
   - **8 bits** : Identifiant du type de sortie.
   - **16 bits** : Registre de destination (`dst`).
   - **16 bits** : Argument 1 (`src1` ou index de constante).
   - **16 bits** : Argument 2 (`src2`).

---

## 3. Le Vérificateur Statique (`crates/knr-verify`)

Le vérificateur s'exécute avant toute interprétation ou compilation. Il est conçu pour s'exécuter en moins de **100 microsecondes par fonction** et implémente 4 passes :

### Passe 1 — Validation SSA Stricte
- Les numéros de registres `%0`, `%1`, `%n` doivent être strictement croissants.
- Tout registre utilisé comme opérande dans une instruction doit dominer cette instruction (être défini plus haut dans le bloc ou dans un bloc parent).

### Passe 2 — Contrôle de Propriété Linéaire (State Machine par registre)
- Le vérificateur maintient une table `HashMap<RegId, RegState>` où `RegState` vaut `Alive(Type)` ou `Moved { consumed_at_line: usize }`.
- Lorsqu'un opcode consomme un registre (`MapSet`, `VecPush`, `BufWriteU8`, `ChanSend`, `Free`, `Ret`), son état passe à `Moved`.
- Si une instruction tente d'emprunter ou de consommer un registre déjà à l'état `Moved`, le vérificateur lève immédiatement une erreur `UseAfterMove`.
- À chaque `Terminator::Ret`, si un registre où `ty.is_linear() == true` est encore à l'état `Alive` (et n'est pas la valeur retournée), le vérificateur lève une erreur `LinearLeak` (ou insère automatiquement l'instruction `Free(%reg)` juste avant le `Ret` en mode auto-drop).

### Passe 3 — Vérification des Types
- Vérifie la correspondance exacte des types pour chaque opcode (ex. : dans `MapSet(m, k, v)`, si `m` est de type `Map[Str, I64]`, alors `k` doit être `Str` et `v` doit être `I64`).

### Passe 4 — Contrôle des Capacités et Effets
- Si une fonction contient `Syscall(Effect::Net, ..)`, l'en-tête de la fonction doit obligatoirement déclarer `Effect::Net` dans `fn.effects`.
- Si une fonction appelle une sous-fonction, elle doit posséder le sur-ensemble des effets de la sous-fonction.

### Format du Diagnostic JSON (Pour le Tooling et les Agents)

En cas d'échec, `knr-verify` ne renvoie pas du texte libre mais sérialise cette structure JSON exacte :

```json
{
  "status": "error",
  "error_kind": "UseAfterMove",
  "function": "handle_request",
  "line": 14,
  "instruction": "%6 = map_get %2 %3",
  "violated_register": "%2",
  "consumed_at_line": 11,
  "alive_registers_at_line_14": {
    "%0": "Chan<Buf>",
    "%3": "Str",
    "%4": "Map<Str, Buf>",
    "%5": "I64"
  },
  "fix_hint": "Replace %2 with %4 (the updated Map returned by map_set at line 11)."
}
```

---

## 4. La Machine Virtuelle de Référence (`crates/knr-interp`)

La VM est l'**Oracle Sémantique** du projet : elle définit le comportement exact de chaque instruction et sert de référence pour tester le compilateur natif.

### Représentation des valeurs (`enum Value`)
- `I64(i64)`
- `F64(f64)`
- `Bool(bool)`
- `Str(String)`
- `Buf(Vec<u8>)`
- `Vec(Vec<Value>)`
- `Map(BTreeMap<String, Value>)`
- `Chan(usize)`

### Compteur de Cycles (Gas Metering)
- La structure `VmContext` prend un paramètre `max_steps: u64` (par défaut `1_000_000`).
- Chaque instruction exécutée décrémente le compteur de `1`. S'il atteint `0`, la VM s'arrête proprement avec `VmError::GasExhausted`.
- Cela garantit qu'aucun programme généré ou testé en fuzzing ne peut bloquer l'environnement dans une boucle infinie.

### Mode Trace (`--trace`)
- Enregistre à chaque ligne un cliché `(line, opcode, dst_reg, new_value)` exportable en JSON pour inspecter l'exécution pas à pas.

---

## 5. Le Runtime Multi-Plateforme (`crates/knr-runtime`)

Pour que le même programme SSA puisse manipuler le DOM, ouvrir des sockets TCP et écrire dans des buffers matériels sans changer une seule ligne de code, le runtime expose un trait unique `HostPlatform` :

```rust
pub trait HostPlatform: Send + Sync {
    /// Exécute un appel système isolé par capacité (Net, UI, HW)
    fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError>;
}
```

Trois adaptateurs concrets sont implémentés derrière ce trait :

### 5.1 Le Pont Réseau & Concurrence (`Effect::Net` & `Effect::Async`)
- **Implémentation** : Basée sur le runtime asynchrone `tokio` (en natif) ou les WebSockets/`fetch` (en WASM).
- **Opérations standardisées** :
  - `net.listen(port_i64) -> chan[map]` : Écoute sur un port et pousse chaque requête entrante sous forme de `Map` contenant `"method"`, `"path"`, et `"body"` (de type `Buf`) dans le canal.
  - `net.respond(req_id, status_i64, body_buf) -> bool` : Envoie la réponse sur la socket correspondante sans copie mémoire inutile.

### 5.2 Le Pont Rendu UI / HTML (`Effect::Ui`)
- **Convention d'Arbre Virtuel Unifié** : Une interface graphique est représentée par une `Map` standard avec 4 clés réservées :
  - `"tag"` (`Str` : ex. `"div"`, `"button"`, `"canvas"`)
  - `"props"` (`Map[Str, Str]` : attributs CSS/HTML et identifiants d'événements)
  - `"text"` (`Str` : contenu textuel)
  - `"children"` (`Vec[Map]` : nœuds enfants)
- **Opérations standardisées** :
  - `ui.mount(vdom_map) -> chan[map]` : Envoie l'arbre de `Map` au moteur d'affichage et retourne un canal d'événements utilisateur (clics, saisies clavier).
- **Cibles** :
  - **Web (WASM)** : Un fichier `bridge.js` de ~150 lignes lit la `Map` sérialisée en mémoire linéaire WASM, applique un diff sur le DOM du navigateur et pousse les événements DOM dans le canal d'entrée.
  - **Desktop/Serveur (Natif)** : Sérialise l'arbre directement en chaîne HTML (SSR), ou pousse les changements via un WebSocket local vers une fenêtre WebView.

### 5.3 Le Pont Matériel / Système (`Effect::Hw`)
- **Implémentation** : Les valeurs de type `Buf` sont allouées avec un alignement mémoire strict sur 64 octets (compatible SIMD AVX-512 / ARM NEON et DMA).
- **Opérations standardisées** :
  - `hw.mmio_map(addr_i64, len_i64) -> buf` : Mappe une plage mémoire matérielle ou un fichier (`mmap`) directement dans un registre `buf`.
  - `hw.simd_xor(buf_a, buf_b) -> buf` : Opérations vectorielles sur tranches d'octets.

---

## 6. Le Générateur de Code Binaire (`crates/knr-codegen`)

Une fois le programme validé par `knr-verify` et testé sur `knr-interp`, `knr-codegen` le compile en binaire selon deux cibles :

### 6.1 Cible 1 : Exécutable Natif via Cranelift (`--target native`)
- **Traduction des Types** :
  - `I64`, `Bool` $\rightarrow$ `cranelift::codegen::ir::types::I64`.
  - `F64` $\rightarrow$ `types::F64`.
  - Tous les types linéaires (`Buf`, `Vec`, `Map`, `Chan`, `Str`) sont représentés au niveau machine par un pointeur opaque 64 bits (`types::I64`) pointant vers les structures de `knr-runtime`.
- **Traduction des Opcodes** :
  - Les opcodes arithmétiques (`add`, `sub`, `mul`, `cmp_lt`) sont traduits en une seule instruction processeur native via `FunctionBuilder::ins().iadd(...)`.
  - Les opcodes de structures de données (`map_set`, `vec_push`, `chan_send`) émettent un appel direct (`ins().call(...)`) vers les fonctions C-ABI exposées par `knr-runtime` (ex. : `extern "C" fn rt_map_set(map_ptr: *mut RtMap, key_ptr: *mut RtVal, val_ptr: *mut RtVal) -> *mut RtMap`).
- **Avantage (In-place Mutation)** :
  - Comme `map_set` consomme l'ancienne `Map` (garanti par `knr-verify`), la fonction `rt_map_set` modifie le dictionnaire sur place sans jamais cloner la structure ni utiliser de comptage de références (`Arc`/`Rc`). On obtient la vitesse du C/Rust pur avec une sémantique fonctionnelle SSA.

### 6.2 Cible 2 : Module WebAssembly via `wasm-encoder` (`--target wasm`)
- Génère directement les sections binaires `.wasm` (`TypeSection`, `ImportSection`, `FunctionSection`, `CodeSection`, `ExportSection`).
- Importe les fonctions du runtime (`env.rt_map_set`, `env.rt_ui_mount`, `env.rt_chan_recv`) depuis l'hôte JS ou WASI.
- Traduit chaque registre SSA `%n` en une variable locale WebAssembly (`local.get $n`, `local.set $n`).

---

## 7. Spécification du Tooling CLI (`crates/knr-cli`)

Le binaire `knr` est le point d'entrée unique en ligne de commande. Il expose 5 sous-commandes :

1. `knr check <fichier.ssa> [--json]`
   Parse et exécute `knr-verify`. Affiche les erreurs en JSON structuré (avec l'état des registres vivants) pour l'intégration dans un éditeur ou avec un agent.
2. `knr run <fichier.ssa> [--gas 1000000] [--trace]`
   Vérifie puis exécute immédiatement le programme dans la VM `knr-interp` connectée à `knr-runtime`.
3. `knr build <fichier.ssa> --target <native|wasm> -o <sortie>`
   Compile le programme en exécutable natif autonome ou en module `.wasm`.
4. `knr asm <fichier.ssa> -o <fichier.knrbin>`
   Convertit le texte SSA en bytecode binaire compact (8 octets par instruction).
5. `knr disasm <fichier.knrbin>`
   Désassemble le bytecode binaire vers le format texte SSA canonique (vérifie la bijection 1:1).

---

## 8. Plan d'Exécution Séquentiel

### Étape 1 — Initialisation & `knr-core`
- Créer le workspace Cargo et implémenter `lib.rs` dans `knr-core` ainsi que le parseur texte ligne par ligne et le sérialiseur binaire `.knrbin`.
- **Critère de validation** : Un test unitaire qui parse un programme texte de 15 lignes, l'encode en binaire `.knrbin`, le décode en texte et vérifie que les deux chaînes sont identiques à l'octet près.

### Étape 2 — Le Juge Statique (`knr-verify`)
- Implémenter les 4 passes de vérification et la sortie d'erreur JSON.
- **Critère de validation** : Écrire 20 tests unitaires couvrant un `UseAfterMove` sur une `Map`, un `LinearLeak` sur un `Buf` non libéré, une erreur de type sur `VecPush`, et un appel `Syscall(Net)` sans la permission `effects(Net)`.

### Étape 3 — L'Oracle Sémantique (`knr-interp` + `knr-cli`)
- Implémenter la boucle d'évaluation de la VM avec le compteur de gaz et les commandes `knr check` et `knr run`.
- **Critère de validation** : Exécuter un programme complet qui alloue un `Buf`, écrit des octets dedans, l'insère dans une `Map` et le retourne en moins de 1 ms.

### Étape 4 — Les Ponts Runtime (`knr-runtime`)
- Connecter le trait `HostPlatform` à `tokio` pour les canaux et le réseau, et créer le pont DOM/JSON pour l'effet `Ui`.
- **Critère de validation** : Faire tourner via `knr run` un serveur HTTP minimaliste qui reçoit une requête sur un canal et renvoie un arbre UI sous forme de `Map`.

### Étape 5 — La Compilation AOT (`knr-codegen` + Fuzzing Différentiel)
- Implémenter l'abaissement vers `wasm-encoder` puis `cranelift`.
- **Critère de validation** : Ajouter un test `proptest` qui génère 1 000 séquences d'instructions SSA valides, les exécute sur `knr-interp` et sur le binaire compilé par `knr-codegen`, et vérifie que les sorties sont strictement égales.
