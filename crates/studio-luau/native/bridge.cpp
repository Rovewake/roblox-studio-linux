#include "bridge.h"

#include "Luau/Allocator.h"
#include "Luau/Ast.h"
#include "Luau/BytecodeBuilder.h"
#include "Luau/Config.h"
#include "Luau/ConfigResolver.h"
#include "Luau/Compiler.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/Lexer.h"
#include "Luau/Linter.h"
#include "Luau/Location.h"
#include "Luau/Parser.h"
#include "Luau/ParseResult.h"
#include "lua.h"
#include "lualib.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <limits>
#include <new>
#include <string>
#include <vector>

struct StudioLuauControl {
    std::atomic<bool> cancelled{false};
};

namespace {

constexpr int32_t kDiagnosticSyntax = 1;
constexpr int32_t kDiagnosticCompile = 2;
constexpr int32_t kDiagnosticRuntime = 3;
constexpr int32_t kDiagnosticInternal = 4;
constexpr int32_t kDiagnosticType = 5;
constexpr int32_t kDiagnosticLintError = 6;
constexpr int32_t kDiagnosticLintWarning = 7;
constexpr int32_t kDiagnosticAnalysisTimeout = 8;

constexpr int32_t kRunCompleted = 0;
constexpr int32_t kRunRuntimeError = 1;
constexpr int32_t kRunCancelled = 2;
constexpr int32_t kRunTimedOut = 3;
constexpr int32_t kRunMemoryLimit = 4;
constexpr int32_t kRunInternalError = 5;

constexpr size_t kMaximumCapturedOutput = 1024 * 1024;
constexpr char kOutputTruncatedMarker[] = "\n[output truncated at 1 MiB]\n";
constexpr size_t kMaximumOutputBeforeMarker = kMaximumCapturedOutput - sizeof(kOutputTruncatedMarker) + 1;
constexpr size_t kMaximumDiagnosticBytes = 16 * 1024;
constexpr char kDiagnosticTruncatedMarker[] = "\n[diagnostic truncated at 16 KiB]";

struct DiagnosticValue {
    int32_t kind;
    Luau::Location location;
    std::string message;
};

struct MemoryQuota {
    size_t used = 0;
    size_t limit = 0;
    bool exceeded = false;
};

struct RunContext {
    StudioLuauControl* control = nullptr;
    std::chrono::steady_clock::time_point deadline;
    MemoryQuota memory;
    std::string output;
    bool output_truncated = false;
    bool timed_out = false;
    bool cancelled = false;
};

struct LuaStateGuard {
    lua_State* state;

    ~LuaStateGuard()
    {
        if (state)
            lua_close(state);
    }
};

struct InMemoryFileResolver final : Luau::FileResolver {
    explicit InMemoryFileResolver(std::string script_source)
        : source(std::move(script_source))
    {
    }

    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override
    {
        if (name != "Script")
            return std::nullopt;
        return Luau::SourceCode{source, Luau::SourceCode::Script};
    }

    std::string source;
};

struct InMemoryConfigResolver final : Luau::ConfigResolver {
    const Luau::Config& getConfig(const Luau::ModuleName&, const Luau::TypeCheckLimits&) const override
    {
        return config;
    }

    Luau::Config config;
};

char* copy_string(const char* data, size_t size)
{
    char* result = static_cast<char*>(std::malloc(size + 1));
    if (!result)
        return nullptr;
    std::memcpy(result, data, size);
    result[size] = '\0';
    return result;
}

char* copy_string(const std::string& value)
{
    return copy_string(value.data(), value.size());
}

std::string bounded_diagnostic(const char* message, size_t size)
{
    if (size <= kMaximumDiagnosticBytes)
        return std::string(message, size);

    const size_t prefix_size = kMaximumDiagnosticBytes - (sizeof(kDiagnosticTruncatedMarker) - 1);
    std::string result(message, prefix_size);
    result.append(kDiagnosticTruncatedMarker, sizeof(kDiagnosticTruncatedMarker) - 1);
    return result;
}

uint32_t one_based_coordinate(unsigned int coordinate)
{
    if (coordinate == std::numeric_limits<unsigned int>::max())
        return 0;
    return coordinate + 1;
}

void add_diagnostic(
    std::vector<DiagnosticValue>& diagnostics,
    int32_t kind,
    const Luau::Location& location,
    const std::string& message
)
{
    diagnostics.push_back({kind, location, message});
}

bool compile_source(const std::string& source, std::string& bytecode, std::vector<DiagnosticValue>& diagnostics)
{
    Luau::Allocator allocator;
    Luau::AstNameTable names(allocator);
    Luau::ParseResult parse_result = Luau::Parser::parse(source.data(), source.size(), names, allocator);

    if (!parse_result.errors.empty()) {
        for (const Luau::ParseError& error : parse_result.errors)
            add_diagnostic(diagnostics, kDiagnosticSyntax, error.getLocation(), error.getMessage());
        return false;
    }

    try {
        Luau::BytecodeBuilder builder;
        Luau::CompileOptions options;
        options.optimizationLevel = 0;
        options.debugLevel = 2;
        Luau::compileOrThrow(builder, parse_result, names, options);
        bytecode = builder.getBytecode();
        return true;
    } catch (const Luau::CompileError& error) {
        add_diagnostic(diagnostics, kDiagnosticCompile, error.getLocation(), error.what());
        return false;
    }
}

void analyze_source(const std::string& source, std::vector<DiagnosticValue>& diagnostics)
{
    InMemoryFileResolver file_resolver(source);
    InMemoryConfigResolver config_resolver;
    Luau::FrontendOptions options;
    options.runLintChecks = true;
    options.moduleTimeLimitSec = 2.0;

    Luau::Frontend frontend(Luau::SolverMode::New, &file_resolver, &config_resolver, options);
    Luau::CheckResult result = frontend.check("Script");

    for (const Luau::TypeError& error : result.errors) {
        if (const Luau::SyntaxError* syntax_error = Luau::get_if<Luau::SyntaxError>(&error.data)) {
            add_diagnostic(diagnostics, kDiagnosticSyntax, error.location, syntax_error->message);
        } else {
            add_diagnostic(
                diagnostics,
                kDiagnosticType,
                error.location,
                Luau::toString(error, Luau::TypeErrorToStringOptions{&file_resolver})
            );
        }
    }

    for (const Luau::LintWarning& error : result.lintResult.errors)
        add_diagnostic(diagnostics, kDiagnosticLintError, error.location, error.text);
    for (const Luau::LintWarning& warning : result.lintResult.warnings)
        add_diagnostic(diagnostics, kDiagnosticLintWarning, warning.location, warning.text);
    for (const Luau::ModuleName& name : result.timeoutHits) {
        add_diagnostic(
            diagnostics,
            kDiagnosticAnalysisTimeout,
            Luau::Location(),
            "Luau analysis exceeded the per-module time limit for '" + name + "'"
        );
    }
}

void set_diagnostics(StudioLuauCompileResult* result, const std::vector<DiagnosticValue>& diagnostics)
{
    if (diagnostics.empty())
        return;

    result->diagnostics = static_cast<StudioLuauDiagnostic*>(
        std::calloc(diagnostics.size(), sizeof(StudioLuauDiagnostic))
    );
    if (!result->diagnostics)
        throw std::bad_alloc();

    result->diagnostic_count = diagnostics.size();
    for (size_t index = 0; index < diagnostics.size(); ++index) {
        const DiagnosticValue& source = diagnostics[index];
        StudioLuauDiagnostic& target = result->diagnostics[index];
        target.kind = source.kind;
        target.start_line = one_based_coordinate(source.location.begin.line);
        target.start_column = one_based_coordinate(source.location.begin.column);
        target.end_line = one_based_coordinate(source.location.end.line);
        target.end_column = one_based_coordinate(source.location.end.column);
        target.message = copy_string(source.message);
        if (!target.message)
            throw std::bad_alloc();
    }
}

void set_compile_internal_error(StudioLuauCompileResult* result, const char* message)
{
    result->diagnostics = static_cast<StudioLuauDiagnostic*>(std::calloc(1, sizeof(StudioLuauDiagnostic)));
    if (!result->diagnostics)
        return;
    result->diagnostic_count = 1;
    result->diagnostics[0].kind = kDiagnosticInternal;
    result->diagnostics[0].message = copy_string(message, std::strlen(message));
    if (!result->diagnostics[0].message)
        result->diagnostic_count = 0;
}

void set_run_error(StudioLuauRunResult* result, int32_t status, const char* message)
{
    result->status = status;
    const std::string bounded = bounded_diagnostic(message, std::strlen(message));
    result->message = copy_string(bounded);
}

void set_run_error(StudioLuauRunResult* result, int32_t status, const std::string& message)
{
    result->status = status;
    const std::string bounded = bounded_diagnostic(message.data(), message.size());
    result->message = copy_string(bounded);
}

void* quota_allocator(void* userdata, void* pointer, size_t old_size, size_t new_size)
{
    auto* quota = static_cast<MemoryQuota*>(userdata);

    if (new_size == 0) {
        if (pointer) {
            quota->used -= std::min(quota->used, old_size);
            std::free(pointer);
        }
        return nullptr;
    }

    const size_t accounted_old_size = pointer ? std::min(quota->used, old_size) : 0;
    const size_t base = quota->used - accounted_old_size;
    if (new_size > quota->limit || base > quota->limit - new_size) {
        quota->exceeded = true;
        return nullptr;
    }

    void* replacement = std::realloc(pointer, new_size);
    if (replacement)
        quota->used = base + new_size;
    return replacement;
}

void interrupt_execution(lua_State* state, int)
{
    auto* context = static_cast<RunContext*>(lua_callbacks(state)->userdata);
    if (context->control && context->control->cancelled.load(std::memory_order_relaxed)) {
        context->cancelled = true;
        luaL_error(state, "execution cancelled");
        return;
    }
    if (std::chrono::steady_clock::now() >= context->deadline) {
        context->timed_out = true;
        luaL_error(state, "execution time limit exceeded");
    }
}

int missing_global(lua_State* state)
{
    const char* name = lua_tostring(state, 2);
    if (!name)
        name = "<non-string>";

    lua_pushvalue(state, lua_upvalueindex(1));
    lua_pushvalue(state, 2);
    lua_rawget(state, -2);
    if (!lua_isnil(state, -1))
        return 1;

    lua_pop(state, 2);
    luaL_error(
        state,
        "global '%s' is unavailable in this language-only Luau runtime; Roblox Engine APIs are not implemented",
        name
    );
    return 0;
}

void append_output(RunContext* context, const char* value, size_t size)
{
    if (context->output.size() >= kMaximumOutputBeforeMarker) {
        context->output_truncated = true;
        return;
    }

    const size_t available = kMaximumOutputBeforeMarker - context->output.size();
    const size_t copied = std::min(available, size);
    try {
        context->output.append(value, copied);
    } catch (...) {
        context->output_truncated = true;
        return;
    }
    if (copied < size)
        context->output_truncated = true;
}

int runtime_print(lua_State* state)
{
    auto* context = static_cast<RunContext*>(lua_touserdata(state, lua_upvalueindex(1)));
    const int argument_count = lua_gettop(state);

    for (int index = 1; index <= argument_count; ++index) {
        if (index > 1)
            append_output(context, " ", 1);

        const int type = lua_type(state, index);
        if (type == LUA_TSTRING || type == LUA_TNUMBER) {
            size_t size = 0;
            const char* value = lua_tolstring(state, index, &size);
            if (value)
                append_output(context, value, size);
        } else if (type == LUA_TBOOLEAN) {
            const char* value = lua_toboolean(state, index) ? "true" : "false";
            append_output(context, value, std::strlen(value));
        } else if (type == LUA_TNIL) {
            append_output(context, "nil", 3);
        } else {
            const char* value = lua_typename(state, type);
            append_output(context, value, std::strlen(value));
        }
    }

    append_output(context, "\n", 1);
    return 0;
}

void open_library(lua_State* state, lua_CFunction open, const char* name)
{
    lua_pushcfunction(state, open, name);
    lua_pushstring(state, name);
    lua_call(state, 1, 0);
}

void open_safe_libraries(lua_State* state)
{
    open_library(state, luaopen_base, "");
    open_library(state, luaopen_coroutine, LUA_COLIBNAME);
    open_library(state, luaopen_table, LUA_TABLIBNAME);
    open_library(state, luaopen_string, LUA_STRLIBNAME);
    open_library(state, luaopen_math, LUA_MATHLIBNAME);
    open_library(state, luaopen_utf8, LUA_UTF8LIBNAME);
    open_library(state, luaopen_bit32, LUA_BITLIBNAME);
    open_library(state, luaopen_buffer, LUA_BUFFERLIBNAME);
    open_library(state, luaopen_vector, LUA_VECLIBNAME);
    open_library(state, luaopen_integer, LUA_INTLIBNAME);
}

uint32_t parse_runtime_line(const char* message)
{
    unsigned int line = 0;
    if (message && std::sscanf(message, "Script:%u:", &line) == 1)
        return line;
    return 0;
}

void copy_output(StudioLuauRunResult* result, const std::string& output)
{
    if (output.empty())
        return;
    result->output = static_cast<uint8_t*>(std::malloc(output.size()));
    if (!result->output)
        throw std::bad_alloc();
    std::memcpy(result->output, output.data(), output.size());
    result->output_size = output.size();
}

void finish_output(RunContext& context)
{
    if (context.output_truncated)
        context.output.append(kOutputTruncatedMarker, sizeof(kOutputTruncatedMarker) - 1);
}

} // namespace

extern "C" int32_t studio_luau_compile(const char* source, size_t source_size, StudioLuauCompileResult* result)
{
    if (!result || (!source && source_size != 0))
        return -1;
    *result = {};

    try {
        const std::string source_text(source ? source : "", source_size);
        std::string bytecode;
        std::vector<DiagnosticValue> diagnostics;
        if (!compile_source(source_text, bytecode, diagnostics)) {
            set_diagnostics(result, diagnostics);
            return 1;
        }

        if (!bytecode.empty()) {
            result->bytecode = static_cast<uint8_t*>(std::malloc(bytecode.size()));
            if (!result->bytecode)
                throw std::bad_alloc();
            std::memcpy(result->bytecode, bytecode.data(), bytecode.size());
            result->bytecode_size = bytecode.size();
        }
        return 0;
    } catch (const std::exception& error) {
        studio_luau_compile_result_free(result);
        set_compile_internal_error(result, error.what());
        return -1;
    } catch (...) {
        studio_luau_compile_result_free(result);
        set_compile_internal_error(result, "unknown native compiler error");
        return -1;
    }
}

extern "C" void studio_luau_compile_result_free(StudioLuauCompileResult* result)
{
    if (!result)
        return;
    std::free(result->bytecode);
    for (size_t index = 0; index < result->diagnostic_count; ++index)
        std::free(result->diagnostics[index].message);
    std::free(result->diagnostics);
    *result = {};
}

extern "C" int32_t studio_luau_analyze(const char* source, size_t source_size, StudioLuauCompileResult* result)
{
    if (!result || (!source && source_size != 0))
        return -1;
    *result = {};

    try {
        const std::string source_text(source ? source : "", source_size);
        std::vector<DiagnosticValue> diagnostics;
        analyze_source(source_text, diagnostics);
        set_diagnostics(result, diagnostics);
        return 0;
    } catch (const std::exception& error) {
        studio_luau_compile_result_free(result);
        set_compile_internal_error(result, error.what());
        return -1;
    } catch (...) {
        studio_luau_compile_result_free(result);
        set_compile_internal_error(result, "unknown native analyzer error");
        return -1;
    }
}

extern "C" StudioLuauControl* studio_luau_control_new(void)
{
    try {
        return new StudioLuauControl();
    } catch (...) {
        return nullptr;
    }
}

extern "C" void studio_luau_control_cancel(StudioLuauControl* control)
{
    if (control)
        control->cancelled.store(true, std::memory_order_relaxed);
}

extern "C" void studio_luau_control_free(StudioLuauControl* control)
{
    delete control;
}

extern "C" int32_t studio_luau_run(
    const uint8_t* bytecode,
    size_t bytecode_size,
    uint64_t memory_limit_bytes,
    uint64_t timeout_millis,
    StudioLuauControl* control,
    StudioLuauRunResult* result
)
{
    if (!result || (!bytecode && bytecode_size != 0))
        return -1;
    *result = {};

    RunContext context;
    context.control = control;
    context.deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeout_millis);
    context.memory.limit = static_cast<size_t>(std::min<uint64_t>(
        memory_limit_bytes,
        static_cast<uint64_t>(std::numeric_limits<size_t>::max())
    ));

    try {
        context.output.reserve(kMaximumCapturedOutput);
        LuaStateGuard guard{lua_newstate(quota_allocator, &context.memory)};
        if (!guard.state) {
            set_run_error(
                result,
                context.memory.exceeded ? kRunMemoryLimit : kRunInternalError,
                context.memory.exceeded ? "Luau VM memory limit exceeded" : "could not create Luau VM"
            );
            return -1;
        }

        lua_callbacks(guard.state)->userdata = &context;
        lua_callbacks(guard.state)->interrupt = interrupt_execution;
        open_safe_libraries(guard.state);
        luaL_sandbox(guard.state);

        lua_pushvalue(guard.state, LUA_GLOBALSINDEX);
        const int base_globals = lua_absindex(guard.state, -1);

        lua_newtable(guard.state);
        const int environment = lua_absindex(guard.state, -1);
        lua_newtable(guard.state);
        const int metatable = lua_absindex(guard.state, -1);
        lua_pushvalue(guard.state, base_globals);
        lua_pushcclosure(guard.state, missing_global, "unavailable_global", 1);
        lua_setfield(guard.state, metatable, "__index");
        lua_setreadonly(guard.state, metatable, true);
        lua_setmetatable(guard.state, environment);

        lua_pushlightuserdata(guard.state, &context);
        lua_pushcclosure(guard.state, runtime_print, "print", 1);
        lua_setfield(guard.state, environment, "print");

        const int load_status = luau_load(
            guard.state,
            "@Script",
            reinterpret_cast<const char*>(bytecode),
            bytecode_size,
            environment
        );
        if (load_status != LUA_OK) {
            size_t message_size = 0;
            const char* message_data = lua_tolstring(guard.state, -1, &message_size);
            const std::string message = message_data
                ? bounded_diagnostic(message_data, message_size)
                : "invalid Luau bytecode";
            set_run_error(result, kRunInternalError, message);
            return -1;
        }

        const int call_status = lua_pcall(guard.state, 0, 0, 0);
        if (call_status == LUA_OK) {
            result->status = kRunCompleted;
            finish_output(context);
            copy_output(result, context.output);
            return 0;
        }

        size_t message_size = 0;
        const char* message_data = lua_tolstring(guard.state, -1, &message_size);
        const std::string message = message_data
            ? bounded_diagnostic(message_data, message_size)
            : "Luau script failed";
        const uint32_t line = parse_runtime_line(message.c_str());
        result->line = line;

        if (context.cancelled) {
            set_run_error(result, kRunCancelled, message);
        } else if (context.timed_out) {
            set_run_error(result, kRunTimedOut, message);
        } else if (context.memory.exceeded || call_status == LUA_ERRMEM) {
            set_run_error(result, kRunMemoryLimit, "Luau VM memory limit exceeded");
        } else {
            set_run_error(result, kRunRuntimeError, message);
        }
        finish_output(context);
        copy_output(result, context.output);
        return 1;
    } catch (const std::exception& error) {
        studio_luau_run_result_free(result);
        set_run_error(result, kRunInternalError, error.what());
        return -1;
    } catch (...) {
        studio_luau_run_result_free(result);
        set_run_error(result, kRunInternalError, "unknown native runtime error");
        return -1;
    }
}

extern "C" void studio_luau_run_result_free(StudioLuauRunResult* result)
{
    if (!result)
        return;
    std::free(result->output);
    std::free(result->message);
    *result = {};
}
