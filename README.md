# GGEngine

A lightweight, cross-platform 2D game engine built with modern C++ and Vulkan.

## Features

- **High-Performance 2D Rendering** - Batched quad rendering with bindless textures, supporting 100K+ sprites per frame, and unlimited texture slots
- **Modern Vulkan Backend** - Built on Vulkan 1.4 with descriptor indexing for optimal GPU utilization
- **Entity Component System** - Simple, cache-friendly ECS for game object management
- **Async Asset Loading** - Non-blocking texture loading with callback support for smooth runtime performance
- **Hot Reload** - Automatic texture reloading when files change on disk (Debug & Release builds)
- **Cross-Platform** - Windows, Linux, and macOS support
- **Editor Application** - Visual scene editing with hierarchy panel and property inspector
- **Dear ImGui Integration** - Built-in debug UI and editor tooling

## Quick Start

### Prerequisites

| Tool | Download |
|------|----------|
| CMake 3.20+ | https://cmake.org/download/ |
| Vulkan SDK | https://vulkan.lunarg.com/sdk/home |
| LLVM/Clang | https://releases.llvm.org/download.html |
| Ninja | https://github.com/ninja-build/ninja/releases |
| MSYS2 (Windows) | https://www.msys2.org/ |

### 1. Clone the Repository

```bash
git clone --recurse-submodules <repo-url>
cd GGEngine
```

If you already cloned without submodules:
```bash
git submodule update --init --recursive
```

### 2. Install Build Tools (Windows)

After installing MSYS2, open "MSYS2 UCRT64" from your Start menu and run:
```bash
pacman -S mingw-w64-ucrt-x86_64-toolchain
```

### 3. Build

**Windows:**
```powershell
.\scripts\build-all.bat debug
```

**Linux/macOS:**
```bash
./scripts/build-all debug
```

### 4. Run

```powershell
# Interactive examples
.\bin\Debug-x64\Sandbox\Sandbox.exe

# Scene editor
.\bin\Debug-x64\Editor\Editor.exe
```

## Your First Application

Create a simple application that displays a colored quad:

```cpp
#include "GGEngine.h"
#include "GGEngine/Core/EntryPoint.h"

class GameLayer : public GGEngine::Layer
{
public:
    void OnAttach() override
    {
        // Set up an orthographic camera
        m_Camera.SetOrthographic(16.0f, 9.0f, -1.0f, 1.0f);
    }

    void OnRenderOffscreen(GGEngine::Timestep ts) override
    {
        // Begin rendering
        GGEngine::Renderer2D::BeginScene(m_Camera);

        // Draw a red quad at the center
        GGEngine::Renderer2D::DrawQuad(
            GGEngine::QuadSpec()
                .SetPosition(0.0f, 0.0f)
                .SetSize(2.0f, 2.0f)
                .SetColor(1.0f, 0.0f, 0.0f, 1.0f)
        );

        // End rendering
        GGEngine::Renderer2D::EndScene();
    }

private:
    GGEngine::Camera m_Camera;
};

class MyGame : public GGEngine::Application
{
public:
    MyGame()
    {
        PushLayer(new GameLayer());
    }
};

GGEngine::Application* GGEngine::CreateApplication()
{
    return new MyGame();
}
```

## Project Structure

```
GGEngine/
├── Engine/                 # Core engine library
│   ├── src/
│   │   ├── GGEngine/       # Engine systems (Core, Renderer, ECS, etc.)
│   │   └── Platform/       # Platform-specific code (Vulkan, Windows, GLFW)
│   └── assets/
│       └── shaders/        # GLSL shaders (compiled to SPIR-V at build time)
├── Editor/                 # Visual scene editor
├── Sandbox/                # Example application with demos
├── Vendor/                 # Third-party libraries (GLFW, ImGui, GLM, etc.)
└── scripts/                # Build scripts
```

## Examples

The Sandbox application includes several interactive examples:

- **Renderer2D Basics** - Drawing colored and rotated quads
- **Renderer2D Textures** - Loading and rendering textured sprites
- **ECS** - Creating entities with components
- **ECS Camera** - Scene camera system
- **Particle System** - Simple particle effects
- **Input Handling** - Keyboard and mouse input

Use the number keys (1-6) to switch between examples while running Sandbox.

## Documentation

For detailed technical documentation, architecture details, and API reference, see [CLAUDE.md](CLAUDE.md).

## Build Configurations

| Configuration | Command | Description |
|---------------|---------|-------------|
| Debug | `build-all.bat debug` | Full debugging, assertions enabled |
| Release | `build-all.bat release` | Optimized, static library |
| Dist | `build-all.bat dist` | Distribution build, logging stripped |

## Dependencies

GGEngine uses the following open-source libraries:

- [Vulkan](https://www.vulkan.org/) - Graphics API
- [GLFW](https://www.glfw.org/) - Window and input
- [Dear ImGui](https://github.com/ocornut/imgui) - Debug UI
- [GLM](https://github.com/g-truc/glm) - Math library
- [spdlog](https://github.com/gabime/spdlog) - Logging
- [VulkanMemoryAllocator](https://github.com/GPUOpen-LibrariesAndSDKs/VulkanMemoryAllocator) - GPU memory management
- [stb_image](https://github.com/nothings/stb) - Image loading
- [nlohmann/json](https://github.com/nlohmann/json) - JSON parsing

## License

[Add your license here]

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.
