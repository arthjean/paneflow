#ifndef PANEFLOW_HIGHLIGHTS_H
#define PANEFLOW_HIGHLIGHTS_H

#include <stdint.h>

#define PANEFLOW_HL_OK 0
#define PANEFLOW_HL_TRAP 1
#define PANEFLOW_HL_MEMORY_LIMIT 2
#define PANEFLOW_HL_OUT_OF_BOUNDS 3
#define PANEFLOW_HL_POISONED 4
#define PANEFLOW_HL_ALLOCATION 5

typedef uint32_t (*paneflow_hl_class)(const uint8_t* bytes, uint32_t len);

typedef struct paneflow_hl paneflow_hl;

typedef struct {
  const uint8_t* text;
  uint32_t byte_len;
  uint32_t utf16_len;
  uint32_t flags;
  uint32_t record_count;
  const uint32_t* records;
} paneflow_hl_line;

typedef struct {
  const uint8_t* data;
  uint64_t size;
} paneflow_hl_memory;

int32_t paneflow_hl_new(paneflow_hl_class id_start, paneflow_hl_class id_continue,
                        uint64_t max_memory_bytes, paneflow_hl** out);
void paneflow_hl_free(paneflow_hl* hl);
int32_t paneflow_hl_init(paneflow_hl* hl, const uint8_t* text, uint32_t len, uint32_t language);
int32_t paneflow_hl_edit(paneflow_hl* hl, uint32_t start_line, uint32_t start_column,
                         uint32_t end_line, uint32_t end_column, const uint8_t* text,
                         uint32_t len);
int32_t paneflow_hl_run(paneflow_hl* hl, uint32_t line_budget, uint32_t end_line,
                        uint32_t byte_budget);
int32_t paneflow_hl_stat(paneflow_hl* hl, uint32_t index, uint32_t* out);
int32_t paneflow_hl_line_count(paneflow_hl* hl, uint32_t* out);
int32_t paneflow_hl_read_line(paneflow_hl* hl, uint32_t line, paneflow_hl_line* out);
int32_t paneflow_hl_read_lines(paneflow_hl* hl, uint32_t first, uint32_t count,
                               paneflow_hl_line* out);
int32_t paneflow_hl_changes(paneflow_hl* hl, const uint32_t** records, uint32_t* count);
int32_t paneflow_hl_memory_view(const paneflow_hl* hl, paneflow_hl_memory* out);
int32_t paneflow_hl_probe_trap(paneflow_hl* hl);

#endif
