/// Minimal entry shim when linking native binaries against the real Rust `libknr_runtime.a`.
pub const KNR_ENTRY_SHIM_C: &str = r#"#include <stdint.h>
#ifdef ENTRY_FN
extern int64_t ENTRY_FN(void);
#endif

__attribute__((weak)) int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
#ifdef ENTRY_FN
    return (int)ENTRY_FN();
#else
    return 0;
#endif
}
"#;

/// Fallback self-contained C runtime used only when `libknr_runtime.a` is unavailable.
pub const KNR_RT_C: &str = r#"/* knr fallback runtime C library for native builds */
#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <math.h>

double rt_sin(double x) { return sin(x); }
double rt_cos(double x) { return cos(x); }
double rt_tan(double x) { return tan(x); }
double rt_pow(double a, double b) { return pow(a, b); }
double rt_atan2(double y, double x) { return atan2(y, x); }

/* --- Struct Implementation --- */
typedef struct {
    int64_t* fields;
    size_t len;
} RtStruct;

int64_t rt_struct_new(int64_t field_count) {
    if (field_count < 1) field_count = 1;
    RtStruct* s = (RtStruct*)malloc(sizeof(RtStruct));
    if (!s) return 0;
    s->fields = (int64_t*)calloc((size_t)field_count, sizeof(int64_t));
    s->len = (size_t)field_count;
    return (int64_t)(uintptr_t)s;
}

int64_t rt_struct_get(int64_t ptr, int64_t idx) {
    RtStruct* s = (RtStruct*)(uintptr_t)ptr;
    if (!s || idx < 0 || (size_t)idx >= s->len) return 0;
    return s->fields[idx];
}

int64_t rt_struct_set(int64_t ptr, int64_t idx, int64_t val) {
    RtStruct* s = (RtStruct*)(uintptr_t)ptr;
    if (!s || idx < 0) return ptr;
    if ((size_t)idx >= s->len) {
        size_t new_len = (size_t)idx + 1;
        int64_t* nf = (int64_t*)realloc(s->fields, new_len * sizeof(int64_t));
        if (!nf) return ptr;
        memset(nf + s->len, 0, (new_len - s->len) * sizeof(int64_t));
        s->fields = nf;
        s->len = new_len;
    }
    s->fields[idx] = val;
    return ptr;
}

void rt_struct_free(int64_t ptr) {
    RtStruct* s = (RtStruct*)(uintptr_t)ptr;
    if (!s) return;
    if (s->fields) free(s->fields);
    free(s);
}

/* --- String Helpers --- */
int64_t rt_str_concat(int64_t a_ptr, int64_t b_ptr) {
    const char* a = a_ptr ? (const char*)(uintptr_t)a_ptr : "";
    const char* b = b_ptr ? (const char*)(uintptr_t)b_ptr : "";
    size_t la = strlen(a), lb = strlen(b);
    char* out = (char*)malloc(la + lb + 1);
    if (!out) return 0;
    memcpy(out, a, la);
    memcpy(out + la, b, lb + 1);
    return (int64_t)(uintptr_t)out;
}

int64_t rt_str_eq(int64_t a_ptr, int64_t b_ptr) {
    if (a_ptr == b_ptr) return 1;
    if (!a_ptr || !b_ptr) return 0;
    return strcmp((const char*)(uintptr_t)a_ptr, (const char*)(uintptr_t)b_ptr) == 0 ? 1 : 0;
}

int64_t rt_str_len(int64_t s_ptr) {
    if (!s_ptr) return 0;
    return (int64_t)strlen((const char*)(uintptr_t)s_ptr);
}

int64_t rt_int_to_str(int64_t val) {
    char* out = (char*)malloc(32);
    if (!out) return 0;
    snprintf(out, 32, "%lld", (long long)val);
    return (int64_t)(uintptr_t)out;
}

int64_t rt_bool_to_str(int64_t val) {
    const char* s = val ? "true" : "false";
    char* out = (char*)malloc(8);
    if (!out) return 0;
    strcpy(out, s);
    return (int64_t)(uintptr_t)out;
}

int64_t rt_f64_to_str(double val) {
    char* out = (char*)malloc(48);
    if (!out) return 0;
    snprintf(out, 48, "%g", val);
    return (int64_t)(uintptr_t)out;
}

/* --- Vector Implementation --- */
typedef struct {
    int64_t* items;
    size_t len;
    size_t cap;
} RtVec;

int64_t rt_vec_new(void) {
    RtVec* v = (RtVec*)malloc(sizeof(RtVec));
    if (!v) return 0;
    v->items = NULL;
    v->len = 0;
    v->cap = 0;
    return (int64_t)(uintptr_t)v;
}

int64_t rt_vec_push(int64_t vec_ptr, int64_t val) {
    RtVec* v = (RtVec*)(uintptr_t)vec_ptr;
    if (!v) return 0;
    if (v->len >= v->cap) {
        size_t new_cap = v->cap == 0 ? 8 : v->cap * 2;
        int64_t* new_items = (int64_t*)realloc(v->items, new_cap * sizeof(int64_t));
        if (!new_items) return (int64_t)(uintptr_t)v;
        v->items = new_items;
        v->cap = new_cap;
    }
    v->items[v->len++] = val;
    return (int64_t)(uintptr_t)v;
}

int64_t rt_vec_get(int64_t vec_ptr, int64_t index) {
    RtVec* v = (RtVec*)(uintptr_t)vec_ptr;
    if (!v || index < 0 || (size_t)index >= v->len) return 0;
    return v->items[index];
}

int64_t rt_vec_len(int64_t vec_ptr) {
    RtVec* v = (RtVec*)(uintptr_t)vec_ptr;
    if (!v) return 0;
    return (int64_t)v->len;
}

void rt_vec_free(int64_t vec_ptr) {
    RtVec* v = (RtVec*)(uintptr_t)vec_ptr;
    if (!v) return;
    if (v->items) free(v->items);
    free(v);
}

/* --- Map Implementation --- */
typedef struct MapEntry {
    uint8_t* key;
    size_t key_len;
    int64_t val;
    struct MapEntry* next;
} MapEntry;

typedef struct {
    MapEntry* head;
} RtMap;

int64_t rt_map_new(void) {
    RtMap* m = (RtMap*)malloc(sizeof(RtMap));
    if (!m) return 0;
    m->head = NULL;
    return (int64_t)(uintptr_t)m;
}

int64_t rt_map_set(int64_t map_ptr, int64_t key_ptr, int64_t key_len, int64_t val) {
    RtMap* m = (RtMap*)(uintptr_t)map_ptr;
    if (!m || !key_ptr) return map_ptr;
    if (key_len <= 0) {
        key_len = (int64_t)strlen((const char*)(uintptr_t)key_ptr);
    }
    const uint8_t* k = (const uint8_t*)(uintptr_t)key_ptr;
    for (MapEntry* curr = m->head; curr != NULL; curr = curr->next) {
        if (curr->key_len == (size_t)key_len && memcmp(curr->key, k, (size_t)key_len) == 0) {
            curr->val = val;
            return (int64_t)(uintptr_t)m;
        }
    }
    MapEntry* entry = (MapEntry*)malloc(sizeof(MapEntry));
    if (!entry) return (int64_t)(uintptr_t)m;
    entry->key = (uint8_t*)malloc((size_t)key_len);
    if (!entry->key) { free(entry); return (int64_t)(uintptr_t)m; }
    memcpy(entry->key, k, (size_t)key_len);
    entry->key_len = (size_t)key_len;
    entry->val = val;
    entry->next = m->head;
    m->head = entry;
    return (int64_t)(uintptr_t)m;
}

int64_t rt_map_get(int64_t map_ptr, int64_t key_ptr, int64_t key_len) {
    RtMap* m = (RtMap*)(uintptr_t)map_ptr;
    if (!m || !key_ptr) return 0;
    if (key_len <= 0) {
        key_len = (int64_t)strlen((const char*)(uintptr_t)key_ptr);
    }
    const uint8_t* k = (const uint8_t*)(uintptr_t)key_ptr;
    for (MapEntry* curr = m->head; curr != NULL; curr = curr->next) {
        if (curr->key_len == (size_t)key_len && memcmp(curr->key, k, (size_t)key_len) == 0) {
            return curr->val;
        }
    }
    return 0;
}

int64_t rt_map_del(int64_t map_ptr, int64_t key_ptr, int64_t key_len) {
    RtMap* m = (RtMap*)(uintptr_t)map_ptr;
    if (!m || !key_ptr) return map_ptr;
    if (key_len <= 0) {
        key_len = (int64_t)strlen((const char*)(uintptr_t)key_ptr);
    }
    const uint8_t* k = (const uint8_t*)(uintptr_t)key_ptr;
    MapEntry** prev = &m->head;
    while (*prev) {
        MapEntry* curr = *prev;
        if (curr->key_len == (size_t)key_len && memcmp(curr->key, k, (size_t)key_len) == 0) {
            *prev = curr->next;
            free(curr->key);
            free(curr);
            break;
        }
        prev = &curr->next;
    }
    return map_ptr;
}

void rt_map_free(int64_t map_ptr) {
    RtMap* m = (RtMap*)(uintptr_t)map_ptr;
    if (!m) return;
    MapEntry* curr = m->head;
    while (curr) {
        MapEntry* next = curr->next;
        if (curr->key) free(curr->key);
        free(curr);
        curr = next;
    }
    free(m);
}

/* --- Aligned Buffer Implementation --- */
int64_t rt_buf_alloc_aligned(int64_t size, int64_t align) {
    if (align <= 0) align = 64;
    if (size <= 0) size = align;
    if ((align & (align - 1)) != 0) return 0;
    void* ptr = NULL;
    int res = posix_memalign(&ptr, (size_t)align, (size_t)size);
    if (res != 0 || !ptr) return 0;
    memset(ptr, 0, (size_t)size);
    return (int64_t)(uintptr_t)ptr;
}

void rt_buf_free(int64_t ptr, int64_t size, int64_t align) {
    (void)size;
    (void)align;
    if (ptr) {
        free((void*)(uintptr_t)ptr);
    }
}

/* --- Channels & Syscall Stubs for C-only fallback --- */
int64_t rt_chan_new(int64_t cap) { (void)cap; return 1; }
int64_t rt_chan_send(int64_t id, int64_t val) { (void)id; (void)val; return 1; }
int64_t rt_chan_recv(int64_t id) { (void)id; return 0; }
int64_t rt_syscall(int64_t eff, int64_t op, int64_t payload, int64_t kind) {
    (void)eff; (void)op; (void)payload; (void)kind; return 0;
}

/* --- Fallback Main Entry --- */
#ifdef ENTRY_FN
extern int64_t ENTRY_FN(void);
#endif

__attribute__((weak)) int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
#ifdef ENTRY_FN
    return (int)ENTRY_FN();
#else
    return 0;
#endif
}
"#;
