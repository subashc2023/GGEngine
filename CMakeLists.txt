cmake_minimum_required(VERSION 3.20)

project(GGEngine LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Output layout (as requested)
if(CMAKE_BUILD_TYPE STREQUAL "Release")
    set(BIN_CONFIG "Release-x64")
elseif(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(BIN_CONFIG "Debug-x64")
else()
    set(BIN_CONFIG "${CMAKE_BUILD_TYPE}-x64")
endif()
set(BIN_ROOT "${CMAKE_SOURCE_DIR}/bin/${BIN_CONFIG}")

option(GGENGINE_BUILD_SHARED "Build Engine as a shared library" ON)

if(GGENGINE_BUILD_SHARED)
    add_library(Engine SHARED
        Engine/src/test.cpp
        Engine/src/test.h
    )
else()
    add_library(Engine STATIC
        Engine/src/test.cpp
        Engine/src/test.h
    )
endif()

target_include_directories(Engine PUBLIC
    ${CMAKE_SOURCE_DIR}/Engine/src
)

set_target_properties(Engine PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Engine"
    ARCHIVE_OUTPUT_DIRECTORY "${BIN_ROOT}/Engine"
    LIBRARY_OUTPUT_DIRECTORY "${BIN_ROOT}/Engine"
    OUTPUT_NAME "GGEngine"
    PREFIX ""
)

if(GGENGINE_BUILD_SHARED)
    set_target_properties(Engine PROPERTIES
        WINDOWS_EXPORT_ALL_SYMBOLS ON
    )
endif()

add_executable(Sandbox
    Sandbox/src/main.cpp
)

target_link_libraries(Sandbox PRIVATE Engine)

set_target_properties(Sandbox PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Sandbox"
)

add_executable(Editor
    Editor/src/main.cpp
)

target_link_libraries(Editor PRIVATE Engine)

set_target_properties(Editor PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Editor"
)

if(GGENGINE_BUILD_SHARED)
    # Copy Engine DLL to Sandbox and Editor output dirs post-build
    add_custom_command(TARGET Sandbox POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
            $<TARGET_FILE:Engine>
            $<TARGET_FILE_DIR:Sandbox>
    )

    add_custom_command(TARGET Editor POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
            $<TARGET_FILE:Engine>
            $<TARGET_FILE_DIR:Editor>
    )
endif()
