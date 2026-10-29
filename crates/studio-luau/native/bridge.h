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
    char* module_id;
} StudioLuauDiagnostic;

typedef struct StudioLuauCompileResult {
    uint8_t* bytecode;
    size_t bytecode_size;
    StudioLuauDiagnostic* diagnostics;
    size_t diagnostic_count;
} StudioLuauCompileResult;

typedef struct StudioLuauAnalysisNode {
    const char* id;
    size_t id_size;
    const char* parent_id;
    size_t parent_id_size;
    const char* name;
    size_t name_size;
    const char* source;
    size_t source_size;
    int32_t kind;
} StudioLuauAnalysisNode;

typedef struct StudioLuauCompletionItem {
    char* label;
    int32_t kind;
    char* insert_text;
} StudioLuauCompletionItem;

typedef struct StudioLuauCompletionResult {
    StudioLuauCompletionItem* items;
    size_t item_count;
    int32_t context;
} StudioLuauCompletionResult;

typedef struct StudioLuauDefinitionResult {
    int32_t found;
    uint32_t start_line;
    uint32_t start_column;
    uint32_t end_line;
    uint32_t end_column;
} StudioLuauDefinitionResult;

enum StudioLuauAnalysisNodeKind {
    STUDIO_LUAU_ANALYSIS_CONTAINER = 0,
    STUDIO_LUAU_ANALYSIS_MODULE_SCRIPT = 1,
    STUDIO_LUAU_ANALYSIS_SCRIPT = 2,
    STUDIO_LUAU_ANALYSIS_LOCAL_SCRIPT = 3,
};

typedef struct StudioLuauRunResult {
    int32_t status;
    uint32_t line;
    uint32_t column;
    uint8_t* output;
    size_t output_size;
    char* message;
} StudioLuauRunResult;

typedef struct StudioLuauControl StudioLuauControl;
typedef void (*StudioLuauDebugEventCallback)(void* user_data, uint32_t line);

int32_t studio_luau_compile(const char* source, size_t source_size, StudioLuauCompileResult* result);
void studio_luau_compile_result_free(StudioLuauCompileResult* result);
int32_t studio_luau_analyze(const char* source, size_t source_size, StudioLuauCompileResult* result);
int32_t studio_luau_analyze_project(
    const char* project_root_id,
    size_t project_root_id_size,
    const char* entry_script_id,
    size_t entry_script_id_size,
    const StudioLuauAnalysisNode* nodes,
    size_t node_count,
    StudioLuauCompileResult* result
);
int32_t studio_luau_complete_project(
    const char* project_root_id,
    size_t project_root_id_size,
    const char* entry_script_id,
    size_t entry_script_id_size,
    const StudioLuauAnalysisNode* nodes,
    size_t node_count,
    uint32_t zero_based_line,
    uint32_t zero_based_column,
    StudioLuauCompletionResult* result
);
void studio_luau_completion_result_free(StudioLuauCompletionResult* result);
int32_t studio_luau_find_local_definition(
    const char* source,
    size_t source_size,
    uint32_t zero_based_line,
    uint32_t zero_based_column,
    StudioLuauDefinitionResult* result
);

StudioLuauControl* studio_luau_control_new(void);
void studio_luau_control_cancel(StudioLuauControl* control);
void studio_luau_control_resume(StudioLuauControl* control);
void studio_luau_control_free(StudioLuauControl* control);

int32_t studio_luau_run(
    const uint8_t* bytecode,
    size_t bytecode_size,
    uint64_t memory_limit_bytes,
    uint64_t timeout_millis,
    StudioLuauControl* control,
    StudioLuauRunResult* result
);
int32_t studio_luau_debug_run(
    const uint8_t* bytecode,
    size_t bytecode_size,
    const uint32_t* breakpoint_lines,
    size_t breakpoint_count,
    uint64_t memory_limit_bytes,
    uint64_t timeout_millis,
    StudioLuauControl* control,
    StudioLuauDebugEventCallback event_callback,
    void* event_user_data,
    StudioLuauRunResult* result
);
void studio_luau_run_result_free(StudioLuauRunResult* result);

#ifdef __cplusplus
}
#endif
