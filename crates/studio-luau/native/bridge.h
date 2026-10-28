#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct StudioLuauDiagnostic {
    int32_t kind;
    uint32_t start_line;
    uint32_t start_column;
    uint32_t end_line;
    uint32_t end_column;
    char* message;
} StudioLuauDiagnostic;

typedef struct StudioLuauCompileResult {
    uint8_t* bytecode;
    size_t bytecode_size;
    StudioLuauDiagnostic* diagnostics;
    size_t diagnostic_count;
} StudioLuauCompileResult;

typedef struct StudioLuauRunResult {
    int32_t status;
    uint32_t line;
    uint32_t column;
    uint8_t* output;
    size_t output_size;
    char* message;
} StudioLuauRunResult;

typedef struct StudioLuauControl StudioLuauControl;

int32_t studio_luau_compile(const char* source, size_t source_size, StudioLuauCompileResult* result);
void studio_luau_compile_result_free(StudioLuauCompileResult* result);
int32_t studio_luau_analyze(const char* source, size_t source_size, StudioLuauCompileResult* result);

StudioLuauControl* studio_luau_control_new(void);
void studio_luau_control_cancel(StudioLuauControl* control);
void studio_luau_control_free(StudioLuauControl* control);

int32_t studio_luau_run(
    const uint8_t* bytecode,
    size_t bytecode_size,
    uint64_t memory_limit_bytes,
    uint64_t timeout_millis,
    StudioLuauControl* control,
    StudioLuauRunResult* result
);
void studio_luau_run_result_free(StudioLuauRunResult* result);

#ifdef __cplusplus
}
#endif
