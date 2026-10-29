#include "bridge.h"

#include "Luau/Allocator.h"
#include "Luau/Cancellation.h"
#include "Luau/Ast.h"
#include "Luau/Autocomplete.h"
#include "Luau/BytecodeBuilder.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/AstQuery.h"
#include "Luau/Config.h"
#include "Luau/ConfigResolver.h"
#include "Luau/Compiler.h"
#include "Luau/Error.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/Lexer.h"
#include "Luau/Module.h"
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
#include <condition_variable>
#include <exception>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <string>
#include <string_view>
#include <stdexcept>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include <vector>

struct StudioLuauControl {
    std::atomic<bool> cancelled{false};
    std::mutex debug_mutex;
    std::condition_variable debug_changed;
    bool resume_requested = false;
};

std::string_view studio_luau_engine_type_definitions() noexcept;

namespace {

constexpr int32_t kDiagnosticSyntax = 1;
constexpr int32_t kDiagnosticCompile = 2;
constexpr int32_t kDiagnosticRuntime = 3;
constexpr int32_t kDiagnosticInternal = 4;
constexpr int32_t kDiagnosticType = 5;
constexpr int32_t kDiagnosticLintError = 6;
constexpr int32_t kDiagnosticLintWarning = 7;
constexpr int32_t kDiagnosticAnalysisTimeout = 8;
constexpr int32_t kDiagnosticAnalysisLimit = 9;

constexpr int32_t kRunCompleted = 0;
constexpr int32_t kRunRuntimeError = 1;
constexpr int32_t kRunCancelled = 2;
constexpr int32_t kRunTimedOut = 3;
constexpr int32_t kRunMemoryLimit = 4;
constexpr int32_t kRunInternalError = 5;
constexpr int32_t kRunInvalidBreakpoint = 6;

constexpr size_t kMaximumCapturedOutput = 1024 * 1024;
constexpr char kOutputTruncatedMarker[] = "\n[output truncated at 1 MiB]\n";
constexpr size_t kMaximumOutputBeforeMarker = kMaximumCapturedOutput - sizeof(kOutputTruncatedMarker) + 1;
constexpr size_t kMaximumDiagnosticBytes = 16 * 1024;
constexpr char kDiagnosticTruncatedMarker[] = "\n[diagnostic truncated at 16 KiB]";
constexpr size_t kMaximumScriptSourceBytes = 2 * 1024 * 1024;
constexpr size_t kMaximumProjectSourceBytes = 16 * 1024 * 1024;
constexpr size_t kMaximumProjectMetadataBytes = 128 * 1024 * 1024;
constexpr size_t kMaximumProjectNodes = 100'000;
constexpr size_t kMaximumProjectIdBytes = 256;
constexpr size_t kMaximumProjectNameBytes = 512;
constexpr size_t kMaximumProjectModules = 512;
constexpr size_t kMaximumExpressionDepth = 256;
constexpr size_t kMaximumResolverNodeVisits = 250'000;
constexpr size_t kMaximumCompletionItems = 256;
constexpr size_t kMaximumCompletionLabelBytes = 512;
constexpr size_t kMaximumDebugBreakpoints = 256;
constexpr auto kMaximumProjectAnalysisTime = std::chrono::seconds(15);
constexpr char kSingleScriptModuleId[] = "Script";

struct DiagnosticValue {
    int32_t kind;
    Luau::Location location;
    std::string message;
    std::string module_id;
};

struct MemoryQuota {
    size_t used = 0;
    size_t limit = 0;
    bool exceeded = false;
};

struct RunContext {
    StudioLuauControl* control = nullptr;
    StudioLuauDebugEventCallback debug_event_callback = nullptr;
    void* debug_event_user_data = nullptr;
    std::chrono::steady_clock::time_point deadline;
    MemoryQuota memory;
    std::string output;
    bool output_truncated = false;
    bool timed_out = false;
    bool cancelled = false;
    bool debug_enabled = false;
    bool skip_breakpoint_once = false;
    uint32_t last_breakpoint_line = 0;
    uint32_t paused_line = 0;
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

bool valid_utf8(const char* data, size_t size)
{
    if (!data && size != 0)
        return false;

    const auto* bytes = reinterpret_cast<const unsigned char*>(data);
    size_t index = 0;
    while (index < size) {
        const unsigned char first = bytes[index];
        if (first <= 0x7f) {
            ++index;
            continue;
        }

        size_t count = 0;
        unsigned char second_min = 0x80;
        unsigned char second_max = 0xbf;
        if (first >= 0xc2 && first <= 0xdf) {
            count = 2;
        } else if (first >= 0xe0 && first <= 0xef) {
            count = 3;
            if (first == 0xe0)
                second_min = 0xa0;
            else if (first == 0xed)
                second_max = 0x9f;
        } else if (first >= 0xf0 && first <= 0xf4) {
            count = 4;
            if (first == 0xf0)
                second_min = 0x90;
            else if (first == 0xf4)
                second_max = 0x8f;
        } else {
            return false;
        }

        if (count > size - index)
            return false;
        const unsigned char second = bytes[index + 1];
        if (second < second_min || second > second_max)
            return false;
        for (size_t offset = 2; offset < count; ++offset) {
            const unsigned char continuation = bytes[index + offset];
            if (continuation < 0x80 || continuation > 0xbf)
                return false;
        }
        index += count;
    }
    return true;
}

bool has_embedded_nul(const std::string& value)
{
    return value.find('\0') != std::string::npos;
}

struct ProjectNode {
    std::string id;
    std::string parent_id;
    std::string name;
    std::string source;
    int32_t kind = STUDIO_LUAU_ANALYSIS_CONTAINER;
    size_t parent = std::numeric_limits<size_t>::max();
    std::vector<size_t> children;
    bool in_project = false;
};

struct ProjectFileResolver final : Luau::FileResolver {
    bool initialize(
        const char* project_root_id,
        size_t project_root_id_size,
        const char* entry_script_id,
        size_t entry_script_id_size,
        const StudioLuauAnalysisNode* input_nodes,
        size_t node_count,
        std::string& error
    )
    {
        if (!project_root_id || project_root_id_size == 0 || project_root_id_size > kMaximumProjectIdBytes ||
            !valid_utf8(project_root_id, project_root_id_size)) {
            error = "project root ID must be nonempty valid UTF-8";
            return false;
        }
        if (!entry_script_id || entry_script_id_size == 0 || entry_script_id_size > kMaximumProjectIdBytes ||
            !valid_utf8(entry_script_id, entry_script_id_size)) {
            error = "entry script ID must be nonempty valid UTF-8";
            return false;
        }
        if (!input_nodes || node_count == 0 || node_count > kMaximumProjectNodes) {
            error = "project node count is outside the supported range";
            return false;
        }

        const std::string root_id(project_root_id, project_root_id_size);
        const std::string entry_id(entry_script_id, entry_script_id_size);
        if (has_embedded_nul(root_id) || has_embedded_nul(entry_id)) {
            error = "project and entry IDs must not contain NUL bytes";
            return false;
        }

        nodes.reserve(node_count);
        size_t total_source_bytes = 0;
        size_t total_metadata_bytes = 0;
        size_t module_count = 0;
        for (size_t index = 0; index < node_count; ++index) {
            const StudioLuauAnalysisNode& input = input_nodes[index];
            if (!input.id || input.id_size == 0 || input.id_size > kMaximumProjectIdBytes ||
                !valid_utf8(input.id, input.id_size)) {
                error = "every project node must have a bounded, nonempty UTF-8 ID";
                return false;
            }
            if (!input.name || input.name_size > kMaximumProjectNameBytes ||
                !valid_utf8(input.name, input.name_size) ||
                std::memchr(input.name, '\0', input.name_size) != nullptr) {
                error = "every project node must have a bounded valid UTF-8 name";
                return false;
            }
            if ((input.parent_id_size != 0 && !input.parent_id) || input.parent_id_size > kMaximumProjectIdBytes ||
                !valid_utf8(input.parent_id, input.parent_id_size) ||
                (input.parent_id_size != 0 && std::memchr(input.parent_id, '\0', input.parent_id_size) != nullptr)) {
                error = "project node parent IDs must be bounded valid UTF-8";
                return false;
            }
            if ((input.source_size != 0 && !input.source) || input.source_size > kMaximumScriptSourceBytes ||
                !valid_utf8(input.source, input.source_size)) {
                error = "project script source must be valid UTF-8 and no larger than 2 MiB";
                return false;
            }
            if (input.kind < STUDIO_LUAU_ANALYSIS_CONTAINER || input.kind > STUDIO_LUAU_ANALYSIS_LOCAL_SCRIPT) {
                error = "project node has an unsupported kind";
                return false;
            }
            if (input.kind == STUDIO_LUAU_ANALYSIS_CONTAINER && input.source_size != 0) {
                error = "container nodes cannot contain script source";
                return false;
            }
            if (input.kind == STUDIO_LUAU_ANALYSIS_MODULE_SCRIPT && ++module_count > kMaximumProjectModules) {
                error = "project contains more than 512 ModuleScripts";
                return false;
            }

            const size_t metadata_size = input.id_size + input.parent_id_size + input.name_size;
            if (metadata_size > kMaximumProjectMetadataBytes - total_metadata_bytes) {
                error = "project node metadata exceeds the 128 MiB limit";
                return false;
            }
            total_metadata_bytes += metadata_size;
            if (input.source_size > kMaximumProjectSourceBytes - total_source_bytes) {
                error = "combined project script source exceeds the 16 MiB limit";
                return false;
            }
            total_source_bytes += input.source_size;

            ProjectNode node;
            node.id.assign(input.id, input.id_size);
            if (input.parent_id_size != 0)
                node.parent_id.assign(input.parent_id, input.parent_id_size);
            node.name.assign(input.name, input.name_size);
            if (input.source_size != 0)
                node.source.assign(input.source, input.source_size);
            node.kind = input.kind;

            const size_t node_index = nodes.size();
            if (!node_indices.emplace(node.id, node_index).second) {
                error = "project node IDs must be unique";
                return false;
            }
            nodes.push_back(std::move(node));
        }

        auto root_it = node_indices.find(root_id);
        auto entry_it = node_indices.find(entry_id);
        if (root_it == node_indices.end()) {
            error = "project root ID does not exist in the supplied nodes";
            return false;
        }
        if (entry_it == node_indices.end()) {
            error = "entry script ID does not exist in the supplied nodes";
            return false;
        }
        project_root = root_it->second;
        entry_script = entry_it->second;
        entry_module_name = "@studio-project-entry";
        while (node_indices.find(entry_module_name) != node_indices.end())
            entry_module_name.insert(entry_module_name.begin(), '@');

        if (nodes[project_root].kind != STUDIO_LUAU_ANALYSIS_CONTAINER || !nodes[project_root].parent_id.empty()) {
            error = "project root must be a parentless container node";
            return false;
        }
        if (nodes[entry_script].kind == STUDIO_LUAU_ANALYSIS_CONTAINER) {
            error = "entry node must be a Script, LocalScript, or ModuleScript";
            return false;
        }

        for (size_t index = 0; index < nodes.size(); ++index) {
            ProjectNode& node = nodes[index];
            if (node.parent_id.empty())
                continue;
            auto parent_it = node_indices.find(node.parent_id);
            if (parent_it == node_indices.end()) {
                error = "project node refers to a missing parent";
                return false;
            }
            node.parent = parent_it->second;
            nodes[node.parent].children.push_back(index);
        }

        if (!has_acyclic_parents()) {
            error = "project node parent links contain a cycle";
            return false;
        }

        std::vector<size_t> pending{project_root};
        nodes[project_root].in_project = true;
        for (size_t cursor = 0; cursor < pending.size(); ++cursor) {
            const size_t parent_index = pending[cursor];
            for (size_t child_index : nodes[parent_index].children) {
                if (!nodes[child_index].in_project) {
                    nodes[child_index].in_project = true;
                    pending.push_back(child_index);
                }
            }
        }

        if (!nodes[entry_script].in_project) {
            error = "entry script must be a descendant of the project root";
            return false;
        }
        if (std::any_of(nodes.begin(), nodes.end(), [](const ProjectNode& node) { return !node.in_project; })) {
            error = "every project node must belong to the declared project tree";
            return false;
        }
        return true;
    }

    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override
    {
        const bool is_frontend_entry = name == entry_module_name;
        const auto node_index = node_index_for_module(name);
        if (!node_index)
            return std::nullopt;
        const ProjectNode& node = nodes[*node_index];
        if (!node.in_project || node.kind == STUDIO_LUAU_ANALYSIS_CONTAINER)
            return std::nullopt;

        if (node.kind != STUDIO_LUAU_ANALYSIS_MODULE_SCRIPT && !is_frontend_entry)
            return std::nullopt;

        const Luau::SourceCode::Type source_type = node.kind == STUDIO_LUAU_ANALYSIS_MODULE_SCRIPT
            ? Luau::SourceCode::Module
            : Luau::SourceCode::Script;
        return Luau::SourceCode{node.source, source_type};
    }

    std::optional<Luau::ModuleInfo> resolveModule(
        const Luau::ModuleInfo* context,
        Luau::AstExpr* expression,
        const Luau::TypeCheckLimits& limits
    ) override
    {
        (void)limits;
        if (!context || context->name.empty())
            return std::nullopt;

        const auto context_node = node_index_for_module(context->name);
        if (!context_node || !nodes[*context_node].in_project)
            return std::nullopt;

        std::optional<size_t> target;
        if (auto path = constant_string(expression))
            target = resolve_string_path(*path, *context_node);
        else
            target = resolve_instance_expression(expression, *context_node, kMaximumExpressionDepth);

        if (!target || !nodes[*target].in_project)
            return std::nullopt;
        return Luau::ModuleInfo{nodes[*target].id, false};
    }

    std::string getHumanReadableModuleName(const Luau::ModuleName& name) const override
    {
        if (name == entry_module_name)
            return nodes[entry_script].id;
        return name;
    }

    const std::string& frontend_entry_name() const
    {
        return entry_module_name;
    }

    std::string diagnostic_module_id(const Luau::ModuleName& name) const
    {
        if (name == entry_module_name)
            return nodes[entry_script].id;
        return name;
    }

    bool resolution_budget_exhausted() const
    {
        return resolver_budget_exhausted;
    }

    std::string_view entry_source() const
    {
        return nodes[entry_script].source;
    }

private:
    std::optional<size_t> node_index_for_module(const Luau::ModuleName& name) const
    {
        if (name == entry_module_name)
            return entry_script;
        auto it = node_indices.find(name);
        if (it == node_indices.end())
            return std::nullopt;
        return it->second;
    }

    static std::optional<std::string> constant_string(Luau::AstExpr* expression)
    {
        if (!expression)
            return std::nullopt;
        const auto* literal = expression->as<Luau::AstExprConstantString>();
        if (!literal)
            return std::nullopt;
        if (literal->value.size == 0)
            return std::string{};
        if (!literal->value.data || !valid_utf8(literal->value.data, literal->value.size))
            return std::nullopt;
        return std::string(literal->value.data, literal->value.size);
    }

    bool has_acyclic_parents() const
    {
        std::vector<uint8_t> state(nodes.size(), 0);
        for (size_t start = 0; start < nodes.size(); ++start) {
            if (state[start] == 2)
                continue;

            std::vector<size_t> path;
            size_t current = start;
            while (current != std::numeric_limits<size_t>::max() && state[current] == 0) {
                state[current] = 1;
                path.push_back(current);
                current = nodes[current].parent;
            }
            if (current != std::numeric_limits<size_t>::max() && state[current] == 1)
                return false;
            for (size_t index : path)
                state[index] = 2;
        }
        return true;
    }

    bool consume_resolver_work()
    {
        if (resolver_budget_exhausted)
            return false;
        if (resolver_node_visits == kMaximumResolverNodeVisits) {
            resolver_budget_exhausted = true;
            return false;
        }
        ++resolver_node_visits;
        return true;
    }

    std::optional<size_t> unique_child(size_t parent_index, const std::string& name)
    {
        std::optional<size_t> found;
        for (size_t child_index : nodes[parent_index].children) {
            if (!consume_resolver_work())
                return std::nullopt;
            const ProjectNode& child = nodes[child_index];
            if (!child.in_project || child.name != name)
                continue;
            if (found)
                return std::nullopt;
            found = child_index;
        }
        return found;
    }

    std::optional<size_t> parent_of(size_t node_index) const
    {
        if (node_index == project_root || nodes[node_index].parent == std::numeric_limits<size_t>::max())
            return std::nullopt;
        const size_t parent_index = nodes[node_index].parent;
        if (!nodes[parent_index].in_project)
            return std::nullopt;
        return parent_index;
    }

    std::optional<size_t> unique_descendant(size_t ancestor_index, const std::string& name)
    {
        if (resolver_budget_exhausted)
            return std::nullopt;
        std::optional<size_t> found;
        std::vector<size_t> pending;
        pending.reserve(nodes[ancestor_index].children.size());
        for (size_t child_index : nodes[ancestor_index].children) {
            if (!consume_resolver_work())
                return std::nullopt;
            pending.push_back(child_index);
        }
        for (size_t cursor = 0; cursor < pending.size(); ++cursor) {
            const size_t candidate = pending[cursor];
            const ProjectNode& node = nodes[candidate];
            if (!node.in_project)
                continue;
            if (node.name == name) {
                if (found)
                    return std::nullopt;
                found = candidate;
            }
            for (size_t child_index : node.children) {
                if (!consume_resolver_work())
                    return std::nullopt;
                pending.push_back(child_index);
            }
        }
        return found;
    }

    std::optional<size_t> resolve_string_path(const std::string& path, size_t current_script)
    {
        size_t current = current_script;
        size_t offset = 0;
        if (path.compare(0, 6, "@self/") == 0) {
            offset = 6;
        } else if (path.compare(0, 6, "@game/") == 0) {
            current = project_root;
            offset = 6;
        } else if (path.compare(0, 2, "./") == 0) {
            auto parent = parent_of(current);
            if (!parent)
                return std::nullopt;
            current = *parent;
            offset = 2;
        } else if (path.compare(0, 3, "../") == 0) {
            auto parent = parent_of(current);
            if (!parent)
                return std::nullopt;
            parent = parent_of(*parent);
            if (!parent)
                return std::nullopt;
            current = *parent;
            offset = 3;
        } else {
            return std::nullopt;
        }

        while (offset < path.size()) {
            const size_t separator = path.find('/', offset);
            const size_t end = separator == std::string::npos ? path.size() : separator;
            if (end == offset)
                return std::nullopt;
            const std::string component = path.substr(offset, end - offset);
            if (component == "..") {
                auto parent = parent_of(current);
                if (!parent)
                    return std::nullopt;
                current = *parent;
            } else {
                auto child = unique_child(current, component);
                if (!child)
                    return std::nullopt;
                current = *child;
            }
            if (separator == std::string::npos)
                offset = path.size();
            else
                offset = separator + 1;
        }

        return current;
    }

    std::optional<size_t> resolve_instance_expression(
        Luau::AstExpr* expression,
        size_t resolved_receiver,
        size_t remaining_depth
    )
    {
        if (!expression || remaining_depth == 0)
            return std::nullopt;

        if (auto* group = expression->as<Luau::AstExprGroup>())
            return resolve_instance_expression(group->expr, resolved_receiver, remaining_depth - 1);

        if (auto* global = expression->as<Luau::AstExprGlobal>()) {
            if (global->name == "script" && nodes[resolved_receiver].kind != STUDIO_LUAU_ANALYSIS_CONTAINER)
                return resolved_receiver;
            if (global->name == "game")
                return project_root;
            return std::nullopt;
        }

        if (auto* index = expression->as<Luau::AstExprIndexName>()) {
            if (index->index == "Parent")
                return parent_of(resolved_receiver);
            return unique_child(resolved_receiver, index->index.value);
        }

        if (auto* index = expression->as<Luau::AstExprIndexExpr>()) {
            auto name = constant_string(index->index);
            if (!name)
                return std::nullopt;
            return unique_child(resolved_receiver, *name);
        }

        auto* call = expression->as<Luau::AstExprCall>();
        if (!call || !call->self || call->args.size < 1)
            return std::nullopt;
        auto* method = call->func->as<Luau::AstExprIndexName>();
        if (!method)
            return std::nullopt;

        if (method->index == "GetService") {
            if (call->args.size != 1)
                return std::nullopt;
            auto service_name = constant_string(call->args.data[0]);
            if (resolved_receiver != project_root || !service_name)
                return std::nullopt;
            return unique_child(project_root, *service_name);
        }

        if (method->index == "WaitForChild") {
            if (call->args.size > 2)
                return std::nullopt;
            auto child_name = constant_string(call->args.data[0]);
            if (!child_name)
                return std::nullopt;
            return unique_child(resolved_receiver, *child_name);
        }

        if (method->index == "FindFirstChild") {
            if (call->args.size > 2)
                return std::nullopt;
            auto child_name = constant_string(call->args.data[0]);
            if (!child_name)
                return std::nullopt;

            bool recursive = false;
            if (call->args.size == 2) {
                auto* recursive_argument = call->args.data[1]->as<Luau::AstExprConstantBool>();
                if (!recursive_argument)
                    return std::nullopt;
                recursive = recursive_argument->value;
            }
            return recursive ? unique_descendant(resolved_receiver, *child_name) : unique_child(resolved_receiver, *child_name);
        }

        return std::nullopt;
    }

    std::vector<ProjectNode> nodes;
    std::unordered_map<std::string, size_t> node_indices;
    std::string entry_module_name;
    size_t project_root = std::numeric_limits<size_t>::max();
    size_t entry_script = std::numeric_limits<size_t>::max();
    size_t resolver_node_visits = 0;
    bool resolver_budget_exhausted = false;
};

struct AnalysisDeadlineGuard {
    explicit AnalysisDeadlineGuard(std::shared_ptr<Luau::FrontendCancellationToken> token)
        : token(std::move(token))
        , watchdog([this] {
            std::unique_lock<std::mutex> lock(mutex);
            if (!condition.wait_for(lock, kMaximumProjectAnalysisTime, [this] { return finished; })) {
                timed_out.store(true, std::memory_order_release);
                this->token->cancel();
            }
        })
    {
    }

    ~AnalysisDeadlineGuard()
    {
        finish();
    }

    void finish()
    {
        if (!watchdog.joinable())
            return;
        {
            std::lock_guard<std::mutex> lock(mutex);
            finished = true;
        }
        condition.notify_one();
        if (watchdog.joinable())
            watchdog.join();
    }

    std::shared_ptr<Luau::FrontendCancellationToken> token;
    std::mutex mutex;
    std::condition_variable condition;
    bool finished = false;
    std::atomic<bool> timed_out{false};
    std::thread watchdog;
};

struct InMemoryConfigResolver final : Luau::ConfigResolver {
    const Luau::Config& getConfig(const Luau::ModuleName&, const Luau::TypeCheckLimits&) const override
    {
        return config;
    }

    Luau::Config config;
};

char* copy_string(const char* data, size_t size) noexcept
{
    char* result = static_cast<char*>(std::malloc(size + 1));
    if (!result)
        return nullptr;
    std::memcpy(result, data, size);
    result[size] = '\0';
    return result;
}

char* copy_string(const std::string& value) noexcept
{
    return copy_string(value.data(), value.size());
}

char* copy_bounded_diagnostic(const char* message, size_t size) noexcept
{
    if (size <= kMaximumDiagnosticBytes)
        return copy_string(message, size);

    constexpr size_t marker_size = sizeof(kDiagnosticTruncatedMarker) - 1;
    constexpr size_t prefix_size = kMaximumDiagnosticBytes - marker_size;
    char* result = static_cast<char*>(std::malloc(kMaximumDiagnosticBytes + 1));
    if (!result)
        return nullptr;
    std::memcpy(result, message, prefix_size);
    std::memcpy(result + prefix_size, kDiagnosticTruncatedMarker, marker_size);
    result[kMaximumDiagnosticBytes] = '\0';
    return result;
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
    const std::string& message,
    const std::string& module_id
)
{
    diagnostics.push_back({kind, location, message, module_id});
}

bool compile_source(const std::string& source, std::string& bytecode, std::vector<DiagnosticValue>& diagnostics)
{
    Luau::Allocator allocator;
    Luau::AstNameTable names(allocator);
    Luau::ParseResult parse_result = Luau::Parser::parse(source.data(), source.size(), names, allocator);

    if (!parse_result.errors.empty()) {
        for (const Luau::ParseError& error : parse_result.errors)
            add_diagnostic(diagnostics, kDiagnosticSyntax, error.getLocation(), error.getMessage(), kSingleScriptModuleId);
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
        add_diagnostic(diagnostics, kDiagnosticCompile, error.getLocation(), error.what(), kSingleScriptModuleId);
        return false;
    }
}

void append_check_diagnostics(
    const Luau::CheckResult& result,
    Luau::FileResolver& file_resolver,
    const std::string& default_module_id,
    std::vector<DiagnosticValue>& diagnostics,
    const ProjectFileResolver* project_file_resolver = nullptr
)
{
    for (const Luau::TypeError& error : result.errors) {
        const std::string module_id = error.moduleName.empty()
            ? default_module_id
            : (project_file_resolver
                    ? project_file_resolver->diagnostic_module_id(error.moduleName)
                    : error.moduleName);
        if (const Luau::SyntaxError* syntax_error = Luau::get_if<Luau::SyntaxError>(&error.data)) {
            add_diagnostic(diagnostics, kDiagnosticSyntax, error.location, syntax_error->message, module_id);
        } else {
            add_diagnostic(
                diagnostics,
                kDiagnosticType,
                error.location,
                Luau::toString(error, Luau::TypeErrorToStringOptions{&file_resolver}),
                module_id
            );
        }
    }

    // Luau returns lint results for the entry module; dependency type errors
    // carry their own module IDs through TypeError::moduleName.
    for (const Luau::LintWarning& error : result.lintResult.errors)
        add_diagnostic(diagnostics, kDiagnosticLintError, error.location, error.text, default_module_id);
    for (const Luau::LintWarning& warning : result.lintResult.warnings)
        add_diagnostic(diagnostics, kDiagnosticLintWarning, warning.location, warning.text, default_module_id);
    for (const Luau::ModuleName& name : result.timeoutHits) {
        add_diagnostic(
            diagnostics,
            kDiagnosticAnalysisTimeout,
            Luau::Location(),
            "Luau analysis exceeded the per-module time limit for '" + name + "'",
            project_file_resolver ? project_file_resolver->diagnostic_module_id(name) : name
        );
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
    Luau::unfreeze(frontend.globals.globalTypes);
    Luau::registerBuiltinGlobals(frontend, frontend.globals);
    Luau::freeze(frontend.globals.globalTypes);
    Luau::CheckResult result = frontend.check("Script");
    append_check_diagnostics(result, file_resolver, kSingleScriptModuleId, diagnostics);
}

void load_project_definitions(Luau::Frontend& frontend, bool for_autocomplete)
{
    Luau::unfreeze(frontend.globals.globalTypes);
    Luau::registerBuiltinGlobals(frontend, frontend.globals, for_autocomplete);
    const Luau::LoadDefinitionFileResult definitions = frontend.loadDefinitionFile(
        frontend.globals,
        frontend.globals.globalScope,
        studio_luau_engine_type_definitions(),
        "@roblox-engine-0.745.0.7451137",
        false,
        for_autocomplete
    );
    if (!definitions.success) {
        std::string reason = "failed to load the reviewed Roblox Engine type definitions";
        if (!definitions.parseResult.errors.empty())
            reason += ": " + definitions.parseResult.errors.front().getMessage();
        else if (definitions.module && !definitions.module->errors.empty())
            reason += ": " + Luau::toString(definitions.module->errors.front());
        throw std::runtime_error(reason);
    }
    Luau::freeze(frontend.globals.globalTypes);
}

int32_t completion_kind(Luau::AutocompleteEntryKind kind)
{
    switch (kind) {
    case Luau::AutocompleteEntryKind::Property:
        return 1;
    case Luau::AutocompleteEntryKind::Binding:
        return 2;
    case Luau::AutocompleteEntryKind::Keyword:
        return 3;
    case Luau::AutocompleteEntryKind::String:
        return 4;
    case Luau::AutocompleteEntryKind::Type:
        return 5;
    case Luau::AutocompleteEntryKind::Module:
        return 6;
    case Luau::AutocompleteEntryKind::GeneratedFunction:
        return 7;
    case Luau::AutocompleteEntryKind::RequirePath:
        return 8;
    case Luau::AutocompleteEntryKind::HotComment:
        return 9;
    }
    return 0;
}

int32_t completion_context(Luau::AutocompleteContext context)
{
    switch (context) {
    case Luau::AutocompleteContext::Unknown:
        return 0;
    case Luau::AutocompleteContext::Expression:
        return 1;
    case Luau::AutocompleteContext::Statement:
        return 2;
    case Luau::AutocompleteContext::Property:
        return 3;
    case Luau::AutocompleteContext::Type:
        return 4;
    case Luau::AutocompleteContext::Keyword:
        return 5;
    case Luau::AutocompleteContext::String:
        return 6;
    case Luau::AutocompleteContext::HotComment:
        return 7;
    }
    return 0;
}

bool valid_cursor_position(std::string_view source, uint32_t line, uint32_t column)
{
    size_t line_start = 0;
    for (uint32_t current_line = 0; current_line < line; ++current_line) {
        const size_t newline = source.find('\n', line_start);
        if (newline == std::string_view::npos)
            return false;
        line_start = newline + 1;
    }

    const size_t newline = source.find('\n', line_start);
    const size_t line_end = newline == std::string_view::npos ? source.size() : newline;
    const size_t line_size = line_end - line_start;
    if (column > line_size)
        return false;
    if (column == 0 || column == line_size)
        return true;
    const unsigned char byte = static_cast<unsigned char>(source[line_start + column]);
    return (byte & 0xc0) != 0x80;
}

void set_completion_result(
    StudioLuauCompletionResult* result,
    const Luau::AutocompleteResult& autocomplete
)
{
    struct CompletionValue {
        std::string label;
        std::optional<std::string> insert_text;
        int32_t kind;
    };

    std::vector<CompletionValue> values;
    values.reserve(autocomplete.entryMap.size());
    for (const auto& [label, entry] : autocomplete.entryMap) {
        if (label.empty() || label.size() > kMaximumCompletionLabelBytes || has_embedded_nul(label) ||
            !valid_utf8(label.data(), label.size())) {
            continue;
        }
        std::optional<std::string> insert_text = entry.insertText;
        if (insert_text &&
            (insert_text->size() > kMaximumCompletionLabelBytes || has_embedded_nul(*insert_text) ||
                !valid_utf8(insert_text->data(), insert_text->size()))) {
            insert_text.reset();
        }
        values.push_back({label, std::move(insert_text), completion_kind(entry.kind)});
    }
    std::sort(values.begin(), values.end(), [](const CompletionValue& left, const CompletionValue& right) {
        if (left.label != right.label)
            return left.label < right.label;
        return left.kind < right.kind;
    });
    if (values.size() > kMaximumCompletionItems)
        values.resize(kMaximumCompletionItems);

    result->context = completion_context(autocomplete.context);
    if (values.empty())
        return;

    result->items = static_cast<StudioLuauCompletionItem*>(
        std::calloc(values.size(), sizeof(StudioLuauCompletionItem))
    );
    if (!result->items)
        throw std::bad_alloc();
    result->item_count = values.size();

    for (size_t index = 0; index < values.size(); ++index) {
        const CompletionValue& source = values[index];
        StudioLuauCompletionItem& target = result->items[index];
        target.label = copy_string(source.label);
        target.kind = source.kind;
        if (source.insert_text)
            target.insert_text = copy_string(*source.insert_text);
        if (!target.label || (source.insert_text && !target.insert_text))
            throw std::bad_alloc();
    }
}

void analyze_project_source(
    const std::string& entry_script_id,
    ProjectFileResolver& file_resolver,
    std::vector<DiagnosticValue>& diagnostics
)
{
    InMemoryConfigResolver config_resolver;
    auto cancellation_token = std::make_shared<Luau::FrontendCancellationToken>();
    cancellation_token->cancelled.store(false, std::memory_order_relaxed);

    Luau::FrontendOptions options;
    options.runLintChecks = true;
    options.moduleTimeLimitSec = 2.0;
    options.cancellationToken = cancellation_token;

    AnalysisDeadlineGuard deadline(cancellation_token);
    try {
        Luau::Frontend frontend(Luau::SolverMode::New, &file_resolver, &config_resolver, options);
        load_project_definitions(frontend, false);
        const Luau::CheckResult result = frontend.check(file_resolver.frontend_entry_name());
        append_check_diagnostics(result, file_resolver, entry_script_id, diagnostics, &file_resolver);
        if (file_resolver.resolution_budget_exhausted()) {
            add_diagnostic(
                diagnostics,
                kDiagnosticAnalysisLimit,
                Luau::Location(),
                "project module resolution exceeded its bounded node-visit budget",
                entry_script_id
            );
        }
    } catch (...) {
        deadline.finish();
        if (!deadline.timed_out.load(std::memory_order_acquire))
            throw;
    }

    deadline.finish();

    if (deadline.timed_out.load(std::memory_order_acquire)) {
        add_diagnostic(
            diagnostics,
            kDiagnosticAnalysisTimeout,
            Luau::Location(),
            "Luau project analysis exceeded the cooperative 15-second deadline",
            entry_script_id
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
        target.module_id = copy_string(source.module_id);
        if (!target.message || !target.module_id)
            throw std::bad_alloc();
    }
}

void set_compile_internal_error(
    StudioLuauCompileResult* result,
    const char* message,
    const char* module_id = kSingleScriptModuleId
)
{
    result->diagnostics = static_cast<StudioLuauDiagnostic*>(std::calloc(1, sizeof(StudioLuauDiagnostic)));
    if (!result->diagnostics)
        return;
    result->diagnostic_count = 1;
    result->diagnostics[0].kind = kDiagnosticInternal;
    result->diagnostics[0].message = copy_string(message, std::strlen(message));
    result->diagnostics[0].module_id = copy_string(module_id, std::strlen(module_id));
    if (!result->diagnostics[0].message || !result->diagnostics[0].module_id) {
        std::free(result->diagnostics[0].message);
        std::free(result->diagnostics[0].module_id);
        result->diagnostics[0].message = nullptr;
        result->diagnostics[0].module_id = nullptr;
        result->diagnostic_count = 0;
    }
}

void set_run_error(StudioLuauRunResult* result, int32_t status, const char* message) noexcept
{
    result->status = status;
    std::free(result->message);
    const char* safe_message = message ? message : "";
    result->message = copy_bounded_diagnostic(safe_message, std::strlen(safe_message));
}

void set_run_error(StudioLuauRunResult* result, int32_t status, const std::string& message) noexcept
{
    result->status = status;
    std::free(result->message);
    result->message = copy_bounded_diagnostic(message.data(), message.size());
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

void debug_breakpoint(lua_State* state, lua_Debug* activation)
{
    auto* context = static_cast<RunContext*>(lua_callbacks(state)->userdata);
    if (!context || !context->debug_enabled || !activation || activation->currentline <= 0)
        return;

    const uint32_t line = static_cast<uint32_t>(activation->currentline);
    if (context->skip_breakpoint_once && line == context->last_breakpoint_line) {
        context->skip_breakpoint_once = false;
        return;
    }

    context->paused_line = line;
    lua_break(state);
}

bool wait_for_debug_resume(RunContext& context)
{
    if (!context.control)
        return false;

    std::unique_lock lock(context.control->debug_mutex);
    const bool signaled = context.control->debug_changed.wait_until(
        lock,
        context.deadline,
        [&context] {
            return context.control->cancelled.load(std::memory_order_relaxed) ||
                context.control->resume_requested;
        }
    );
    if (!signaled) {
        context.timed_out = true;
        return false;
    }
    if (context.control->cancelled.load(std::memory_order_relaxed)) {
        context.cancelled = true;
        return false;
    }

    context.control->resume_requested = false;
    return true;
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
    for (size_t index = 0; index < result->diagnostic_count; ++index) {
        std::free(result->diagnostics[index].message);
        std::free(result->diagnostics[index].module_id);
    }
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

extern "C" int32_t studio_luau_analyze_project(
    const char* project_root_id,
    size_t project_root_id_size,
    const char* entry_script_id,
    size_t entry_script_id_size,
    const StudioLuauAnalysisNode* nodes,
    size_t node_count,
    StudioLuauCompileResult* result
)
{
    if (!result)
        return -1;
    *result = {};

    try {
        ProjectFileResolver file_resolver;
        std::string validation_error;
        if (!file_resolver.initialize(
                project_root_id,
                project_root_id_size,
                entry_script_id,
                entry_script_id_size,
                nodes,
                node_count,
                validation_error
            )) {
            set_compile_internal_error(result, validation_error.c_str(), "Project");
            return -1;
        }

        const std::string entry_id(entry_script_id, entry_script_id_size);
        std::vector<DiagnosticValue> diagnostics;
        analyze_project_source(entry_id, file_resolver, diagnostics);
        set_diagnostics(result, diagnostics);
        return 0;
    } catch (const std::exception& error) {
        studio_luau_compile_result_free(result);
        set_compile_internal_error(result, error.what(), "Project");
        return -1;
    } catch (...) {
        studio_luau_compile_result_free(result);
        set_compile_internal_error(result, "unknown native project analyzer error", "Project");
        return -1;
    }
}

extern "C" int32_t studio_luau_complete_project(
    const char* project_root_id,
    size_t project_root_id_size,
    const char* entry_script_id,
    size_t entry_script_id_size,
    const StudioLuauAnalysisNode* nodes,
    size_t node_count,
    uint32_t zero_based_line,
    uint32_t zero_based_column,
    StudioLuauCompletionResult* result
)
{
    if (!result)
        return -1;
    *result = {};

    try {
        ProjectFileResolver file_resolver;
        std::string validation_error;
        if (!file_resolver.initialize(
                project_root_id,
                project_root_id_size,
                entry_script_id,
                entry_script_id_size,
                nodes,
                node_count,
                validation_error
            )) {
            return -1;
        }

        if (!valid_cursor_position(file_resolver.entry_source(), zero_based_line, zero_based_column))
            return -1;

        const std::string entry_id(entry_script_id, entry_script_id_size);
        InMemoryConfigResolver config_resolver;
        auto cancellation_token = std::make_shared<Luau::FrontendCancellationToken>();
        cancellation_token->cancelled.store(false, std::memory_order_relaxed);

        Luau::FrontendOptions options;
        options.runLintChecks = false;
        options.moduleTimeLimitSec = 2.0;
        options.cancellationToken = cancellation_token;
        options.forAutocomplete = true;
        options.retainFullTypeGraphs = true;

        AnalysisDeadlineGuard deadline(cancellation_token);
        try {
            Luau::Frontend frontend(Luau::SolverMode::New, &file_resolver, &config_resolver, options);
            load_project_definitions(frontend, true);
            const std::string& frontend_entry = file_resolver.frontend_entry_name();
            (void)frontend.check(frontend_entry, options);
            const Luau::AutocompleteResult autocomplete = Luau::autocomplete(
                frontend,
                frontend_entry,
                Luau::Position(zero_based_line, zero_based_column),
                {}
            );
            set_completion_result(result, autocomplete);
            if (file_resolver.resolution_budget_exhausted()) {
                studio_luau_completion_result_free(result);
                deadline.finish();
                return -3;
            }
        } catch (...) {
            deadline.finish();
            if (!deadline.timed_out.load(std::memory_order_acquire))
                throw;
        }

        deadline.finish();
        if (deadline.timed_out.load(std::memory_order_acquire)) {
            studio_luau_completion_result_free(result);
            return -2;
        }
        return 0;
    } catch (...) {
        studio_luau_completion_result_free(result);
        return -1;
    }
}

extern "C" void studio_luau_completion_result_free(StudioLuauCompletionResult* result)
{
    if (!result)
        return;
    for (size_t index = 0; index < result->item_count; ++index) {
        std::free(result->items[index].label);
        std::free(result->items[index].insert_text);
    }
    std::free(result->items);
    *result = {};
}

extern "C" int32_t studio_luau_find_local_definition(
    const char* source,
    size_t source_size,
    uint32_t zero_based_line,
    uint32_t zero_based_column,
    StudioLuauDefinitionResult* result
)
{
    if (!result || (!source && source_size != 0) || source_size > kMaximumScriptSourceBytes)
        return -1;
    *result = {};

    const std::string source_text(source ? source : "", source_size);
    if (!valid_utf8(source_text.data(), source_text.size()) ||
        !valid_cursor_position(source_text, zero_based_line, zero_based_column)) {
        return -1;
    }

    try {
        Luau::SourceModule source_module;
        source_module.name = "Script";
        source_module.type = Luau::SourceCode::Script;
        const Luau::ParseResult parsed = Luau::Parser::parse(
            source_text.data(),
            source_text.size(),
            *source_module.names,
            *source_module.allocator
        );
        source_module.root = parsed.root;
        source_module.parseErrors = parsed.errors;

        Luau::ExprOrLocal selected = Luau::findExprOrLocalAtPosition(
            source_module,
            Luau::Position(zero_based_line, zero_based_column)
        );
        Luau::AstLocal* local = selected.getLocal();
        if (!local) {
            if (Luau::AstExpr* expression = selected.getExpr()) {
                if (auto* local_expression = expression->as<Luau::AstExprLocal>())
                    local = local_expression->local;
            }
        }
        if (!local)
            return 0;

        result->found = 1;
        result->start_line = one_based_coordinate(local->location.begin.line);
        result->start_column = one_based_coordinate(local->location.begin.column);
        result->end_line = one_based_coordinate(local->location.end.line);
        result->end_column = one_based_coordinate(local->location.end.column);
        return 0;
    } catch (...) {
        *result = {};
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
    if (control) {
        {
            // Pair with the waiter's predicate check so cancellation cannot
            // notify in the gap immediately before condition_variable::wait.
            std::lock_guard lock(control->debug_mutex);
            control->cancelled.store(true, std::memory_order_relaxed);
        }
        control->debug_changed.notify_all();
    }
}

extern "C" void studio_luau_control_resume(StudioLuauControl* control)
{
    if (!control)
        return;
    {
        std::lock_guard lock(control->debug_mutex);
        control->resume_requested = true;
    }
    control->debug_changed.notify_one();
}

extern "C" void studio_luau_control_free(StudioLuauControl* control)
{
    delete control;
}

static int32_t studio_luau_run_internal(
    const uint8_t* bytecode,
    size_t bytecode_size,
    const uint32_t* breakpoint_lines,
    size_t breakpoint_count,
    uint64_t memory_limit_bytes,
    uint64_t timeout_millis,
    StudioLuauControl* control,
    bool debug_enabled,
    StudioLuauDebugEventCallback event_callback,
    void* event_user_data,
    StudioLuauRunResult* result
)
{
    if (!result || (!bytecode && bytecode_size != 0))
        return -1;
    *result = {};

    RunContext context;
    context.control = control;
    context.debug_enabled = debug_enabled;
    context.debug_event_callback = event_callback;
    context.debug_event_user_data = event_user_data;
    context.deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeout_millis);
    context.memory.limit = static_cast<size_t>(std::min<uint64_t>(
        memory_limit_bytes,
        static_cast<uint64_t>(std::numeric_limits<size_t>::max())
    ));

    try {
        if (debug_enabled && (!control || !event_callback ||
                breakpoint_count > kMaximumDebugBreakpoints ||
                (!breakpoint_lines && breakpoint_count != 0))) {
            set_run_error(result, kRunInternalError, "invalid native debugger configuration");
            return -1;
        }
        if (!debug_enabled && breakpoint_count != 0) {
            set_run_error(result, kRunInternalError, "breakpoints require debugger mode");
            return -1;
        }
        for (size_t index = 0; index < breakpoint_count; ++index) {
            if (breakpoint_lines[index] == 0 ||
                breakpoint_lines[index] > static_cast<uint32_t>(std::numeric_limits<int>::max())) {
                result->line = breakpoint_lines[index];
                set_run_error(result, kRunInvalidBreakpoint, "breakpoint line is outside the supported range");
                return -1;
            }
        }

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
        if (debug_enabled)
            lua_callbacks(guard.state)->debugbreak = debug_breakpoint;
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

        if (debug_enabled) {
            std::unordered_set<uint32_t> installed_lines;
            for (size_t index = 0; index < breakpoint_count; ++index) {
                const uint32_t requested_line = breakpoint_lines[index];
                if (!installed_lines.insert(requested_line).second)
                    continue;
                const int resolved_line = lua_breakpoint(
                    guard.state,
                    -1,
                    static_cast<int>(requested_line),
                    1
                );
                if (resolved_line < 1) {
                    result->line = requested_line;
                    set_run_error(result, kRunInvalidBreakpoint, "no executable script line exists at or after breakpoint");
                    return -1;
                }
            }
        }

        int call_status = debug_enabled
            ? lua_resume(guard.state, nullptr, 0)
            : lua_pcall(guard.state, 0, 0, 0);
        while (debug_enabled && call_status == LUA_BREAK) {
            if (context.paused_line == 0) {
                set_run_error(result, kRunInternalError, "Luau debugger stopped without a source line");
                return -1;
            }
            context.debug_event_callback(context.debug_event_user_data, context.paused_line);
            context.last_breakpoint_line = context.paused_line;
            if (!wait_for_debug_resume(context)) {
                const int32_t status = context.cancelled
                    ? kRunCancelled
                    : (context.timed_out ? kRunTimedOut : kRunInternalError);
                const char* message = context.cancelled
                    ? "execution cancelled while paused at a breakpoint"
                    : (context.timed_out ? "execution time limit expired while paused at a breakpoint"
                                         : "debugger resume wait failed");
                set_run_error(result, status, message);
                finish_output(context);
                copy_output(result, context.output);
                return 1;
            }
            context.skip_breakpoint_once = true;
            call_status = lua_resume(guard.state, nullptr, 0);
        }

        if (debug_enabled && call_status == LUA_YIELD) {
            set_run_error(result, kRunRuntimeError, "coroutine yield is unsupported in the language-only debugger");
            finish_output(context);
            copy_output(result, context.output);
            return 1;
        }

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

extern "C" int32_t studio_luau_run(
    const uint8_t* bytecode,
    size_t bytecode_size,
    uint64_t memory_limit_bytes,
    uint64_t timeout_millis,
    StudioLuauControl* control,
    StudioLuauRunResult* result
)
{
    return studio_luau_run_internal(
        bytecode,
        bytecode_size,
        nullptr,
        0,
        memory_limit_bytes,
        timeout_millis,
        control,
        false,
        nullptr,
        nullptr,
        result
    );
}

extern "C" int32_t studio_luau_debug_run(
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
)
{
    return studio_luau_run_internal(
        bytecode,
        bytecode_size,
        breakpoint_lines,
        breakpoint_count,
        memory_limit_bytes,
        timeout_millis,
        control,
        true,
        event_callback,
        event_user_data,
        result
    );
}

extern "C" void studio_luau_run_result_free(StudioLuauRunResult* result)
{
    if (!result)
        return;
    std::free(result->output);
    std::free(result->message);
    *result = {};
}
