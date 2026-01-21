cmake_minimum_required(VERSION 3.20)

project(GGEngine LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Dist Build Type Configuration
set(CMAKE_CXX_FLAGS_DIST "${CMAKE_CXX_FLAGS_RELEASE}")
set(CMAKE_C_FLAGS_DIST "${CMAKE_C_FLAGS_RELEASE}")
set(CMAKE_EXE_LINKER_FLAGS_DIST "${CMAKE_EXE_LINKER_FLAGS_RELEASE}")
set(CMAKE_SHARED_LINKER_FLAGS_DIST "${CMAKE_SHARED_LINKER_FLAGS_RELEASE}")

if(CMAKE_BUILD_TYPE STREQUAL "Dist")
    add_compile_definitions(GG_DIST)
endif()

# Output layout (as requested)
if(CMAKE_BUILD_TYPE STREQUAL "Release")
    set(BIN_CONFIG "Release-x64")
elseif(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(BIN_CONFIG "Debug-x64")
else()
    set(BIN_CONFIG "${CMAKE_BUILD_TYPE}-x64")
endif()
set(BIN_ROOT "${CMAKE_SOURCE_DIR}/bin/${BIN_CONFIG}")

option(GGENGINE_BUILD_DLL "Build Engine as a shared library" ON)

# Find Vulkan SDK
find_package(Vulkan REQUIRED)

# Shader compilation
find_program(GLSLC glslc HINTS $ENV{VULKAN_SDK}/Bin)
if(NOT GLSLC)
    message(FATAL_ERROR "glslc not found! Make sure Vulkan SDK is installed and VULKAN_SDK environment variable is set.")
endif()

set(SHADER_SOURCE_DIR ${CMAKE_SOURCE_DIR}/Engine/assets/shaders)
set(SHADER_OUTPUT_DIR ${CMAKE_SOURCE_DIR}/Engine/assets/shaders/compiled)

file(MAKE_DIRECTORY ${SHADER_OUTPUT_DIR})

set(SHADER_SOURCES
    ${SHADER_SOURCE_DIR}/triangle.vert
    ${SHADER_SOURCE_DIR}/triangle.frag
    ${SHADER_SOURCE_DIR}/basic.vert
    ${SHADER_SOURCE_DIR}/basic.frag
    ${SHADER_SOURCE_DIR}/texture.vert
    ${SHADER_SOURCE_DIR}/texture.frag
    ${SHADER_SOURCE_DIR}/quad2d.vert
    ${SHADER_SOURCE_DIR}/quad2d.frag
)

foreach(SHADER ${SHADER_SOURCES})
    get_filename_component(SHADER_NAME ${SHADER} NAME)
    set(SHADER_OUTPUT ${SHADER_OUTPUT_DIR}/${SHADER_NAME}.spv)
    add_custom_command(
        OUTPUT ${SHADER_OUTPUT}
        COMMAND ${GLSLC} ${SHADER} -o ${SHADER_OUTPUT}
        DEPENDS ${SHADER}
        COMMENT "Compiling shader ${SHADER_NAME}"
    )
    list(APPEND SHADER_OUTPUTS ${SHADER_OUTPUT})
endforeach()

add_custom_target(Shaders ALL DEPENDS ${SHADER_OUTPUTS})

# GLFW (before ImGui since ImGui needs it)
set(GLFW_BUILD_DOCS OFF CACHE BOOL "" FORCE)
set(GLFW_INSTALL OFF CACHE BOOL "" FORCE)
set(GLFW_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(GLFW_BUILD_TESTS OFF CACHE BOOL "" FORCE)
add_subdirectory(Vendor/glfw)

# ImGui as a static library (linked into Engine DLL, symbols exported)
set(IMGUI_DIR ${CMAKE_SOURCE_DIR}/Vendor/imgui)
add_library(ImGui STATIC
    ${IMGUI_DIR}/imgui.cpp
    ${IMGUI_DIR}/imgui_demo.cpp
    ${IMGUI_DIR}/imgui_draw.cpp
    ${IMGUI_DIR}/imgui_tables.cpp
    ${IMGUI_DIR}/imgui_widgets.cpp
    ${IMGUI_DIR}/backends/imgui_impl_glfw.cpp
    ${IMGUI_DIR}/backends/imgui_impl_vulkan.cpp
)
target_include_directories(ImGui PUBLIC
    ${IMGUI_DIR}
    ${IMGUI_DIR}/backends
)
target_link_libraries(ImGui PUBLIC Vulkan::Headers glfw)

# ImGui DLL export/import - use same pattern as GG_API
if(WIN32 AND GGENGINE_BUILD_DLL)
    # ImGui static lib itself exports (will be linked into Engine.dll)
    target_compile_definitions(ImGui PRIVATE IMGUI_API=__declspec\(dllexport\))
endif()

# Engine source files
set(ENGINE_SOURCES
    Engine/src/GGEngine.h
    Engine/src/GGEngine/Core/Application.h
    Engine/src/GGEngine/Core/Application.cpp
    Engine/src/GGEngine/Core/Core.h
    Engine/src/GGEngine/Core/Log.h
    Engine/src/GGEngine/Core/Log.cpp
    Engine/src/GGEngine/Events/Event.h
    Engine/src/GGEngine/Events/ApplicationEvent.h
    Engine/src/GGEngine/Events/KeyEvent.h
    Engine/src/GGEngine/Events/MouseEvent.h
    Engine/src/GGEngine/Core/Layer.cpp
    Engine/src/GGEngine/Core/Layer.h
    Engine/src/GGEngine/Core/LayerStack.cpp
    Engine/src/GGEngine/Core/LayerStack.h
    Engine/src/GGEngine/Core/Window.h
    Engine/src/GGEngine/Input.h
    Engine/src/GGEngine/Asset/Asset.h
    Engine/src/GGEngine/Asset/AssetHandle.h
    Engine/src/GGEngine/Asset/AssetManager.h
    Engine/src/GGEngine/Asset/AssetManager.cpp
    Engine/src/GGEngine/Asset/Shader.h
    Engine/src/GGEngine/Asset/Shader.cpp
    Engine/src/GGEngine/Asset/ShaderLibrary.h
    Engine/src/GGEngine/Asset/ShaderLibrary.cpp
    Engine/src/GGEngine/Asset/Texture.h
    Engine/src/GGEngine/Asset/Texture.cpp
    Engine/src/GGEngine/Asset/stb_image.cpp
    Engine/src/Platform/Windows/WindowsWindow.h
    Engine/src/Platform/Windows/WindowsWindow.cpp
    Engine/src/Platform/Windows/WindowsInput.h
    Engine/src/Platform/Windows/WindowsInput.cpp
    Engine/src/Platform/Vulkan/VulkanContext.h
    Engine/src/Platform/Vulkan/VulkanContext.cpp
    Engine/src/Platform/Vulkan/vma.cpp
    Engine/src/GGEngine/ImGui/ImGuiLayer.h
    Engine/src/GGEngine/ImGui/ImGuiLayer.cpp
    Engine/src/GGEngine/ImGui/DebugUI.h
    Engine/src/GGEngine/ImGui/DebugUI.cpp
    Engine/src/GGEngine/Renderer/Framebuffer.h
    Engine/src/GGEngine/Renderer/Framebuffer.cpp
    Engine/src/GGEngine/Renderer/Pipeline.h
    Engine/src/GGEngine/Renderer/Pipeline.cpp
    Engine/src/GGEngine/Renderer/VertexLayout.h
    Engine/src/GGEngine/Renderer/VertexLayout.cpp
    Engine/src/GGEngine/Renderer/Buffer.h
    Engine/src/GGEngine/Renderer/Buffer.cpp
    Engine/src/GGEngine/Renderer/VertexBuffer.h
    Engine/src/GGEngine/Renderer/VertexBuffer.cpp
    Engine/src/GGEngine/Renderer/IndexBuffer.h
    Engine/src/GGEngine/Renderer/IndexBuffer.cpp
    Engine/src/GGEngine/Renderer/RenderCommand.h
    Engine/src/GGEngine/Renderer/RenderCommand.cpp
    Engine/src/GGEngine/Renderer/UniformBuffer.h
    Engine/src/GGEngine/Renderer/UniformBuffer.cpp
    Engine/src/GGEngine/Renderer/DescriptorSet.h
    Engine/src/GGEngine/Renderer/DescriptorSet.cpp
    Engine/src/GGEngine/Renderer/Camera.h
    Engine/src/GGEngine/Renderer/Camera.cpp
    Engine/src/GGEngine/Renderer/OrthographicCameraController.h
    Engine/src/GGEngine/Renderer/OrthographicCameraController.cpp
    Engine/src/GGEngine/Renderer/Material.h
    Engine/src/GGEngine/Renderer/Material.cpp
    Engine/src/GGEngine/Renderer/MaterialLibrary.h
    Engine/src/GGEngine/Renderer/MaterialLibrary.cpp
    Engine/src/GGEngine/Renderer/Renderer2D.h
    Engine/src/GGEngine/Renderer/Renderer2D.cpp
    Engine/src/ggpch.h
    Engine/src/ggpch.cpp
)

if(GGENGINE_BUILD_DLL)
    add_library(Engine SHARED ${ENGINE_SOURCES})
    target_compile_definitions(Engine PRIVATE GG_BUILD_DLL)
    target_compile_definitions(Engine PUBLIC GG_DLL)
    # Engine sources that include imgui.h need to see export definition
    target_compile_definitions(Engine PRIVATE IMGUI_API=__declspec\(dllexport\))
else()
    add_library(Engine STATIC ${ENGINE_SOURCES})
endif()

target_compile_definitions(Engine PUBLIC GG_PLATFORM_WINDOWS)

# Dependencies
add_subdirectory(${CMAKE_SOURCE_DIR}/Vendor/spdlog)
target_link_libraries(Engine PUBLIC spdlog::spdlog ImGui)

# GLFW (defined earlier, just link here)
target_link_libraries(Engine PRIVATE glfw)

# Vulkan - PUBLIC so apps can use VulkanContext
target_link_libraries(Engine PUBLIC Vulkan::Vulkan)

# Windows-specific libraries
if(WIN32)
    target_link_libraries(Engine PRIVATE dwmapi)
endif()

target_include_directories(Engine PUBLIC
    ${CMAKE_SOURCE_DIR}/Engine/src
    ${CMAKE_SOURCE_DIR}/Vendor/VulkanMemoryAllocator/include
    ${CMAKE_SOURCE_DIR}/Vendor/stb_image
)

target_precompile_headers(Engine PUBLIC Engine/src/ggpch.h)

set_target_properties(Engine PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Engine"
    ARCHIVE_OUTPUT_DIRECTORY "${BIN_ROOT}/Engine"
    LIBRARY_OUTPUT_DIRECTORY "${BIN_ROOT}/Engine"
    OUTPUT_NAME "GGEngine"
    PREFIX ""
)

if(GGENGINE_BUILD_DLL)
    set_target_properties(Engine PROPERTIES
        WINDOWS_EXPORT_ALL_SYMBOLS ON
    )
endif()

add_dependencies(Engine Shaders)

add_executable(Sandbox
    Sandbox/src/main.cpp
    Sandbox/src/TriangleLayer.h
    Sandbox/src/TriangleLayer.cpp
)

target_link_libraries(Sandbox PRIVATE Engine)

# Import ImGui symbols from Engine DLL
if(WIN32 AND GGENGINE_BUILD_DLL)
    target_compile_definitions(Sandbox PRIVATE IMGUI_API=__declspec\(dllimport\))
endif()

set_target_properties(Sandbox PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Sandbox"
)

# Copy compiled shaders to output directories (Dist builds only - Debug/Release read from source)
if(CMAKE_BUILD_TYPE STREQUAL "Dist")
    add_custom_command(TARGET Sandbox POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E make_directory $<TARGET_FILE_DIR:Sandbox>/assets/shaders/compiled
        COMMAND ${CMAKE_COMMAND} -E copy_directory
            ${SHADER_OUTPUT_DIR}
            $<TARGET_FILE_DIR:Sandbox>/assets/shaders/compiled
        COMMENT "Copying shaders to Sandbox output directory"
    )
endif()

add_executable(Editor
    Editor/src/main.cpp
    Editor/src/EditorLayer.h
    Editor/src/EditorLayer.cpp
)

target_link_libraries(Editor PRIVATE Engine)

# Import ImGui symbols from Engine DLL
if(WIN32 AND GGENGINE_BUILD_DLL)
    target_compile_definitions(Editor PRIVATE IMGUI_API=__declspec\(dllimport\))
endif()

set_target_properties(Editor PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${BIN_ROOT}/Editor"
)

# Copy compiled shaders to Editor output directory (Dist builds only)
if(CMAKE_BUILD_TYPE STREQUAL "Dist")
    add_custom_command(TARGET Editor POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E make_directory $<TARGET_FILE_DIR:Editor>/assets/shaders/compiled
        COMMAND ${CMAKE_COMMAND} -E copy_directory
            ${SHADER_OUTPUT_DIR}
            $<TARGET_FILE_DIR:Editor>/assets/shaders/compiled
        COMMENT "Copying shaders to Editor output directory"
    )
endif()

if(GGENGINE_BUILD_DLL)
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
