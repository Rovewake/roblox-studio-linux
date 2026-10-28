cmake_minimum_required(VERSION 3.20)
project(StudioLuauBridge LANGUAGES C CXX)

if(NOT DEFINED LUAU_SOURCE_DIR OR NOT EXISTS "${LUAU_SOURCE_DIR}/CMakeLists.txt")
    message(FATAL_ERROR "LUAU_SOURCE_DIR must point to the pinned vendor/luau submodule")
endif()

set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_WEB OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_SHARED OFF CACHE BOOL "" FORCE)

add_subdirectory("${LUAU_SOURCE_DIR}" "${CMAKE_BINARY_DIR}/luau")

# Luau defines additional libraries even when CLI/tests are disabled. Keep the
# initial native build limited to the compiler and VM dependency graph.
foreach(optional_target IN ITEMS Luau.CLI.lib Luau.Inliner Luau.CodeGen Luau.Require isocline)
    if(TARGET ${optional_target})
        set_property(TARGET ${optional_target} PROPERTY EXCLUDE_FROM_ALL TRUE)
    endif()
endforeach()

add_library(studio_luau_native STATIC bridge.cpp)
target_compile_features(studio_luau_native PRIVATE cxx_std_17)
target_compile_options(studio_luau_native PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(studio_luau_native PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
target_link_libraries(studio_luau_native PRIVATE Luau.Analysis Luau.Compiler Luau.VM)

install(
    TARGETS studio_luau_native Luau.Common Luau.Ast Luau.Bytecode Luau.Compiler Luau.Config Luau.Analysis Luau.VM
    ARCHIVE DESTINATION lib
)
