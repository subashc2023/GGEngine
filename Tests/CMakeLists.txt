cmake_minimum_required(VERSION 3.20)

# =============================================================================
# Google Test Setup via FetchContent
# =============================================================================

include(FetchContent)
FetchContent_Declare(
    googletest
    GIT_REPOSITORY https://github.com/google/googletest.git
    GIT_TAG        v1.14.0
)

# Prevent overriding parent project's compiler/linker settings on Windows
set(gtest_force_shared_crt ON CACHE BOOL "" FORCE)
FetchContent_MakeAvailable(googletest)

# =============================================================================
# Test Executable
# =============================================================================

set(TEST_SOURCES
    TestMain.cpp

    # Phase 1: Core Math Tests
    Core/MathTests.cpp
    Renderer/Mat4Tests.cpp
    ECS/TransformComponentTests.cpp

    # Phase 2: Data Structure Tests
    Core/TimestepTests.cpp
    ECS/EntityTests.cpp
    ECS/GUIDTests.cpp
    ECS/ComponentStorageTests.cpp

    # Phase 3: Concurrent System Tests
    Concurrent/JobSystemTests.cpp
    Concurrent/TaskGraphTests.cpp
)

add_executable(GGEngineTests ${TEST_SOURCES})

# Link against Engine and Google Test
target_link_libraries(GGEngineTests PRIVATE
    GTest::gtest
    GTest::gmock
    Engine
)

# Include directories
target_include_directories(GGEngineTests PRIVATE
    ${CMAKE_SOURCE_DIR}/Engine/src
    ${CMAKE_SOURCE_DIR}/Tests
)

# Import ImGui symbols from Engine DLL/shared library
if(GGENGINE_BUILD_DLL)
    if(WIN32)
        target_compile_definitions(GGEngineTests PRIVATE IMGUI_API=__declspec\(dllimport\))
    else()
        target_compile_definitions(GGEngineTests PRIVATE IMGUI_API=)
    endif()
endif()

# Test discovery for CTest
include(GoogleTest)
gtest_discover_tests(GGEngineTests
    PROPERTIES
        LABELS "unit"
    DISCOVERY_TIMEOUT 30
)

# Output directory
set_target_properties(GGEngineTests PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Tests"
)

# Copy Engine DLL if building as shared library
if(GGENGINE_BUILD_DLL)
    add_custom_command(TARGET GGEngineTests POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
            $<TARGET_FILE:Engine>
            $<TARGET_FILE_DIR:GGEngineTests>
    )
endif()
