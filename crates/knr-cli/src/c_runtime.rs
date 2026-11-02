pub const KNR_RT_C: &str = r#"/* knr runtime C library for native builds */
#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

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
