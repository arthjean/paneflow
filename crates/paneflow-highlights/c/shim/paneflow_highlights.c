#include "paneflow_highlights.h"

#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#include "highlights.h"
#include "wasm-rt-impl.h"

#define EDIT_HEADER_BYTES 4u
#define EDIT_RECORD_BYTES 24u
#define STAGE_PADDING_BYTES 96u
#define WIDE_RECORDS_FLAG 4u
#define CHANGE_RECORD_WORDS 4u

struct w2c_env {
  wasm_rt_memory_t* memory;
  paneflow_hl_class id_start;
  paneflow_hl_class id_continue;
};

struct paneflow_hl {
  w2c_highlights instance;
  struct w2c_env env;
  uint64_t max_memory_bytes;
  bool instantiated;
  bool poisoned;
};

static u32 classify(const struct w2c_env* env, u32 ptr, u32 len, paneflow_hl_class class_fn) {
  const wasm_rt_memory_t* memory = env->memory;
  if (memory == NULL || len == 0 || len > 4 || (uint64_t)ptr + len > memory->size) return 0;
  return class_fn(memory->data + ptr, len) != 0;
}

u32 w2c_env_is_id_start(struct w2c_env* env, u32 ptr, u32 len) {
  return classify(env, ptr, len, env->id_start);
}

u32 w2c_env_is_id_continue(struct w2c_env* env, u32 ptr, u32 len) {
  return classify(env, ptr, len, env->id_continue);
}

static int32_t poison(paneflow_hl* hl) {
  hl->poisoned = true;
  return PANEFLOW_HL_TRAP;
}

static bool fits(const paneflow_hl* hl, uint64_t ptr, uint64_t len) {
  return ptr <= hl->env.memory->size && len <= hl->env.memory->size - ptr;
}

static bool within_limit(const paneflow_hl* hl, uint64_t staged) {
  uint64_t size = hl->env.memory->size;
  return staged <= hl->max_memory_bytes && size <= hl->max_memory_bytes - staged;
}

static void write_u32(uint8_t* at, uint32_t value) {
  at[0] = (uint8_t)(value & 0xffu);
  at[1] = (uint8_t)((value >> 8) & 0xffu);
  at[2] = (uint8_t)((value >> 16) & 0xffu);
  at[3] = (uint8_t)(value >> 24);
}

static uint32_t read_u32(const uint8_t* at) {
  return (uint32_t)at[0] | (uint32_t)at[1] << 8 | (uint32_t)at[2] << 16 | (uint32_t)at[3] << 24;
}

int32_t paneflow_hl_new(paneflow_hl_class id_start, paneflow_hl_class id_continue,
                        uint64_t max_memory_bytes, paneflow_hl** out) {
  *out = NULL;
  paneflow_hl* hl = calloc(1, sizeof(paneflow_hl));
  if (hl == NULL) return PANEFLOW_HL_ALLOCATION;
  hl->env.id_start = id_start;
  hl->env.id_continue = id_continue;
  hl->max_memory_bytes = max_memory_bytes;
  wasm_rt_init();
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) {
    if (hl->instantiated) wasm2c_highlights_free(&hl->instance);
    free(hl);
    return PANEFLOW_HL_TRAP;
  }
  wasm2c_highlights_instantiate(&hl->instance, &hl->env);
  hl->instantiated = true;
  hl->env.memory = w2c_highlights_memory(&hl->instance);
  uint64_t ceiling = max_memory_bytes / hl->env.memory->page_size;
  if (ceiling < hl->env.memory->max_pages) hl->env.memory->max_pages = ceiling;
  if (hl->env.memory->size > max_memory_bytes) {
    wasm2c_highlights_free(&hl->instance);
    free(hl);
    return PANEFLOW_HL_MEMORY_LIMIT;
  }
  *out = hl;
  return PANEFLOW_HL_OK;
}

void paneflow_hl_free(paneflow_hl* hl) {
  if (hl == NULL) return;
  if (hl->instantiated) wasm2c_highlights_free(&hl->instance);
  free(hl);
}

int32_t paneflow_hl_init(paneflow_hl* hl, const uint8_t* text, uint32_t len, uint32_t language) {
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  if (!within_limit(hl, (uint64_t)len + STAGE_PADDING_BYTES)) return PANEFLOW_HL_MEMORY_LIMIT;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  u32 ptr = w2c_highlights_liveStage(&hl->instance, len);
  if (!fits(hl, ptr, len)) return PANEFLOW_HL_OUT_OF_BOUNDS;
  if (len > 0) memcpy(hl->env.memory->data + ptr, text, len);
  w2c_highlights_liveInitDoc(&hl->instance, ptr, len, language);
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_edit(paneflow_hl* hl, uint32_t start_line, uint32_t start_column,
                         uint32_t end_line, uint32_t end_column, const uint8_t* text,
                         uint32_t len) {
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  uint64_t block = (uint64_t)EDIT_HEADER_BYTES + EDIT_RECORD_BYTES + len;
  if (block > UINT32_MAX || !within_limit(hl, block + STAGE_PADDING_BYTES)) {
    return PANEFLOW_HL_MEMORY_LIMIT;
  }
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  u32 ptr = w2c_highlights_liveStage(&hl->instance, (u32)block);
  if (!fits(hl, ptr, block)) return PANEFLOW_HL_OUT_OF_BOUNDS;
  uint8_t* at = hl->env.memory->data + ptr;
  write_u32(at, 1u);
  write_u32(at + 4, start_line);
  write_u32(at + 8, start_column);
  write_u32(at + 12, end_line);
  write_u32(at + 16, end_column);
  write_u32(at + 20, EDIT_HEADER_BYTES + EDIT_RECORD_BYTES);
  write_u32(at + 24, len);
  if (len > 0) memcpy(at + EDIT_HEADER_BYTES + EDIT_RECORD_BYTES, text, len);
  w2c_highlights_liveApplyEdits(&hl->instance, ptr, 0u);
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_run(paneflow_hl* hl, uint32_t line_budget, uint32_t end_line,
                        uint32_t byte_budget) {
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  w2c_highlights_liveRun(&hl->instance, line_budget, end_line, byte_budget);
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_stat(paneflow_hl* hl, uint32_t index, uint32_t* out) {
  *out = 0;
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  *out = w2c_highlights_liveStats(&hl->instance, index);
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_line_count(paneflow_hl* hl, uint32_t* out) {
  *out = 0;
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  *out = w2c_highlights_liveLineCount(&hl->instance);
  return PANEFLOW_HL_OK;
}

static int32_t describe_line(paneflow_hl* hl, uint32_t line, paneflow_hl_line* out) {
  w2c_highlights* instance = &hl->instance;
  u32 byte_len = w2c_highlights_liveLineByteLen(instance, line);
  u32 utf16_len = w2c_highlights_liveLineLen(instance, line);
  u32 text = byte_len == 0 ? 0u : w2c_highlights_liveLineTextPtr(instance, line);
  u32 flags = w2c_highlights_liveLineFlags(instance, line);
  u32 count = w2c_highlights_liveLineTokCount(instance, line);
  u32 records = count == 0 ? 0u : w2c_highlights_liveLineTokPtr(instance, line);
  uint64_t record_bytes = (uint64_t)count * ((flags & WIDE_RECORDS_FLAG) != 0 ? 8u : 4u);
  if (!fits(hl, text, byte_len) || !fits(hl, records, record_bytes) || records % 4 != 0) {
    return PANEFLOW_HL_OUT_OF_BOUNDS;
  }
  const uint8_t* data = hl->env.memory->data;
  out->text = data + text;
  out->byte_len = byte_len;
  out->utf16_len = utf16_len;
  out->flags = flags;
  out->record_count = count;
  out->records = (const uint32_t*)(const void*)(data + records);
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_read_line(paneflow_hl* hl, uint32_t line, paneflow_hl_line* out) {
  memset(out, 0, sizeof(*out));
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  return describe_line(hl, line, out);
}

int32_t paneflow_hl_read_lines(paneflow_hl* hl, uint32_t first, uint32_t count,
                               paneflow_hl_line* out) {
  memset(out, 0, sizeof(*out) * count);
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  for (uint32_t index = 0; index < count; index++) {
    int32_t status = describe_line(hl, first + index, &out[index]);
    if (status != PANEFLOW_HL_OK) return status;
  }
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_changes(paneflow_hl* hl, const uint32_t** records, uint32_t* count) {
  *records = NULL;
  *count = 0;
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  u32 base = w2c_highlights_liveChangesPtr(&hl->instance);
  if (!fits(hl, base, 4u) || base % 4 != 0) return PANEFLOW_HL_OUT_OF_BOUNDS;
  uint32_t changes = read_u32(hl->env.memory->data + base);
  if (!fits(hl, (uint64_t)base + 4u, (uint64_t)changes * CHANGE_RECORD_WORDS * 4u)) {
    return PANEFLOW_HL_OUT_OF_BOUNDS;
  }
  *records = (const uint32_t*)(const void*)(hl->env.memory->data + base + 4u);
  *count = changes;
  return PANEFLOW_HL_OK;
}

int32_t paneflow_hl_memory_view(const paneflow_hl* hl, paneflow_hl_memory* out) {
  out->data = hl->env.memory->data;
  out->size = hl->env.memory->size;
  return hl->poisoned ? PANEFLOW_HL_POISONED : PANEFLOW_HL_OK;
}

int32_t paneflow_hl_probe_trap(paneflow_hl* hl) {
  if (hl->poisoned) return PANEFLOW_HL_POISONED;
  WASM_RT_SAVE_STACK_DEPTH();
  if (WASM_RT_SETJMP(g_wasm_rt_jmp_buf) != WASM_RT_TRAP_NONE) return poison(hl);
  wasm_rt_trap(WASM_RT_TRAP_UNREACHABLE);
  return PANEFLOW_HL_OK;
}
