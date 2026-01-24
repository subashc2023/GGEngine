# AGENTS.md

Technical documentation for GGEngine. This file provides guidance to developers and agents and serves as a reference.

## Table of Contents

1. [Build System](#build-system)
2. [Architecture Overview](#architecture-overview)
3. [Core System](#core-system)
4. [RHI Abstraction Layer](#rhi-abstraction-layer)
5. [Vulkan Backend](#vulkan-backend)
6. [Renderer System](#renderer-system)
7. [Asset System](#asset-system)
8. [ECS System](#ecs-system)
9. [Shader System](#shader-system)
10. [Platform Layer](#platform-layer)
11. [Utilities](#utilities)
12. [Key Design Patterns](#key-design-patterns)
13. [Adding New Features](#adding-new-features)

---

## Build System

### Prerequisites

- CMake 3.20+
- LLVM/Clang
- Ninja
- MSYS2 MinGW-w64 (Windows only)
- Vulkan SDK (with `glslc` shader compiler)

### Build Commands

```bash
# Windows
.\scripts\build-all.bat debug     # Debug build (DLL)
.\scripts\build-all.bat release   # Release build (static library)
.\scripts\build-all.bat dist      # Distribution build (logging stripped)

# Linux/macOS
./scripts/build-all debug

# Individual targets
.\scripts\build-engine.bat debug
.\scripts\build-sandbox.bat debug
.\scripts\build-editor.bat debug

# Clean build
.\scripts\clean_and_build.bat debug

# Direct CMake
cmake --preset windows-clang-mingw64
cmake --build --preset windows-clang-mingw64-debug
```

### Build Targets

| Target | Type | Description |
|--------|------|-------------|
| Engine | DLL/Static | Core engine library (`GGENGINE_BUILD_DLL` controls type) |
| Editor | Executable | Visual scene editor |
| Sandbox | Executable | Example application |
| Shaders | Custom | SPIR-V compilation from GLSL |

### Output Structure

```
bin/
├── Debug-x64/
│   ├── Engine/GGEngine.dll
│   ├── Sandbox/Sandbox.exe
│   └── Editor/Editor.exe
└── Release-x64/
    └── ...
```

---

## Architecture Overview

GGEngine is a Vulkan 1.2+ based C++17 game engine optimized for 2D rendering. It uses a layered architecture:

```
┌─────────────────────────────────────────────────────────┐
│                    Application                          │
│              (Editor / Sandbox / Your Game)             │
├─────────────────────────────────────────────────────────┤
│                      Engine API                         │
│    (GGEngine.h - Renderer2D, ECS, Assets, Events)       │
├─────────────────────────────────────────────────────────┤
│                  RHI Abstraction                        │
│        (RHIDevice, RHITypes, RHIEnums)                  │
├─────────────────────────────────────────────────────────┤
│                   Vulkan Backend                        │
│     (VulkanContext, VulkanResourceRegistry)             │
├─────────────────────────────────────────────────────────┤
│                   Platform Layer                        │
│              (Window, Input - GLFW based)               │
└─────────────────────────────────────────────────────────┘
```

### Source Layout

```
Engine/src/
├── GGEngine/
│   ├── Core/           # Application, Layer, Events, Input, Logging, JobSystem
│   ├── Renderer/       # Renderer2D, Pipeline, Camera, Material, Buffers, TransferQueue
│   ├── Asset/          # AssetManager, Shader, Texture, Libraries
│   ├── ECS/            # Scene, Entity, Components, Serialization
│   ├── RHI/            # Backend-agnostic types and enums
│   ├── ImGui/          # ImGui integration layer
│   ├── ParticleSystem/ # Particle effects
│   ├── Utils/          # File dialogs, FileWatcher, helpers
│   └── Debug/          # Profiling instrumentation
├── Platform/
│   ├── Vulkan/         # VulkanContext, VulkanRHI, VulkanImGuiLayer
│   ├── Windows/        # WindowsWindow, WindowsInput
│   └── GLFW/           # Cross-platform GLFW implementations
├── GGEngine.h          # Main include header
└── ggpch.h             # Precompiled header
```

---

## Core System

Location: `Engine/src/GGEngine/Core/`

### Application (`Application.h/cpp`)

Singleton managing the engine lifecycle.

```cpp
// Access the singleton
Application& app = Application::Get();

// Key methods
void Run();                          // Main loop
void PushLayer(Layer* layer);        // Add game layer
void PushOverlay(Layer* layer);      // Add UI overlay
Window& GetWindow();                 // Get native window
ImGuiLayer* GetImGuiLayer();         // Get ImGui layer
MaterialLibrary& GetMaterialLibrary(); // Get material library
```

**Initialization Order:**
1. Window creation (GLFW)
2. VulkanContext initialization
3. RHIDevice initialization
4. BindlessTextureManager initialization
5. ShaderLibrary/TextureLibrary initialization
6. Renderer2D initialization
7. ImGuiLayer attachment

**Main Loop (`Run()`):**
```cpp
while (m_Running) {
    m_Window->OnUpdate();           // Poll events

    if (m_Minimized) {
        glfwWaitEvents();           // Block when minimized
        continue;
    }

    RHIDevice::Get().BeginFrame();

    // Phase 1: Offscreen rendering (framebuffers)
    for (Layer* layer : m_LayerStack)
        layer->OnRenderOffscreen(timestep);

    // Phase 2: Swapchain rendering (ImGui + direct)
    RHIDevice::Get().BeginSwapchainRenderPass();
    m_ImGuiLayer->Begin();
    for (Layer* layer : m_LayerStack)
        layer->OnUpdate(timestep);
    m_ImGuiLayer->End();

    RHIDevice::Get().EndFrame();
}
```

### Layer (`Layer.h/cpp`)

Base class for game logic modules.

```cpp
class GG_API Layer {
public:
    virtual void OnAttach() {}                              // Called when pushed
    virtual void OnDetach() {}                              // Called when popped
    virtual void OnUpdate(Timestep ts) {}                   // Per-frame update (swapchain phase)
    virtual void OnRenderOffscreen(Timestep ts) {}          // Offscreen rendering phase
    virtual void OnEvent(Event& event) {}                   // Event handling
    virtual void OnWindowResize(uint32_t w, uint32_t h) {}  // Resize notification
};
```

**Rendering Phases:**
- `OnRenderOffscreen()`: Render to framebuffers (e.g., editor viewport)
- `OnUpdate()`: Render to swapchain, update ImGui

### Event System (`Events/`)

Type-safe event dispatcher pattern.

```cpp
// Event types
enum class EventType {
    WindowClose, WindowResize, WindowFocus, WindowLostFocus, WindowMoved,
    AppTick, AppUpdate, AppRender,
    KeyPressed, KeyReleased, KeyTyped,
    MouseButtonPressed, MouseButtonReleased, MouseMoved, MouseScrolled
};

// Event categories (bitmask)
enum EventCategory {
    EventCategoryApplication = BIT(0),
    EventCategoryInput = BIT(1),
    EventCategoryKeyboard = BIT(2),
    EventCategoryMouse = BIT(3),
    EventCategoryMouseButton = BIT(4)
};

// Usage in Layer::OnEvent
void OnEvent(Event& e) override {
    EventDispatcher dispatcher(e);
    dispatcher.Dispatch<KeyPressedEvent>([this](KeyPressedEvent& e) {
        // Handle key press
        return true;  // Event handled
    });
}
```

### Smart Pointers (`Core.h`)

```cpp
namespace GGEngine {
    template<typename T> using Scope = std::unique_ptr<T>;
    template<typename T> using Ref = std::shared_ptr<T>;

    template<typename T, typename... Args>
    Scope<T> CreateScope(Args&&... args);

    template<typename T, typename... Args>
    Ref<T> CreateRef(Args&&... args);
}
```

### JobSystem (`Core/JobSystem.h`)

Thread-safe work queue for async operations (asset loading, etc.).

```cpp
JobSystem& jobs = JobSystem::Get();

// Initialize with worker thread count (usually 1 for I/O-bound work)
jobs.Init(1);

// Submit work to background thread
jobs.Submit(
    []() {
        // This runs on worker thread
        LoadFileFromDisk();
    },
    []() {
        // Optional: callback runs on main thread after job completes
        OnLoadComplete();
    },
    JobPriority::Normal  // or High, Low
);

// Call every frame from main thread to process callbacks
jobs.ProcessCompletedCallbacks();

// Shutdown (waits for pending jobs)
jobs.Shutdown();
```

**Thread Safety:**
- `Submit()` is thread-safe
- Callbacks always fire on main thread via `ProcessCompletedCallbacks()`
- Jobs execute in priority order (High > Normal > Low)

---

## RHI Abstraction Layer

Location: `Engine/src/GGEngine/RHI/`

The RHI (Render Hardware Interface) provides backend-agnostic graphics abstractions.

### Handle Types (`RHITypes.h`)

Opaque 64-bit handles that map to backend resources:

```cpp
struct RHICommandBufferHandle   { uint64_t id = 0; bool IsValid() const; };
struct RHIPipelineHandle        { uint64_t id = 0; bool IsValid() const; };
struct RHIPipelineLayoutHandle  { uint64_t id = 0; bool IsValid() const; };
struct RHIRenderPassHandle      { uint64_t id = 0; bool IsValid() const; };
struct RHIFramebufferHandle     { uint64_t id = 0; bool IsValid() const; };
struct RHIBufferHandle          { uint64_t id = 0; bool IsValid() const; };
struct RHITextureHandle         { uint64_t id = 0; bool IsValid() const; };
struct RHISamplerHandle         { uint64_t id = 0; bool IsValid() const; };
struct RHIShaderHandle          { uint64_t id = 0; bool IsValid() const; };
struct RHIShaderModuleHandle    { uint64_t id = 0; bool IsValid() const; };
struct RHIDescriptorSetLayoutHandle { uint64_t id = 0; bool IsValid() const; };
struct RHIDescriptorSetHandle   { uint64_t id = 0; bool IsValid() const; };
```

All handles are hashable for use as map keys.

### Enums (`RHIEnums.h`)

Complete enumeration set:

| Category | Enums |
|----------|-------|
| Pipeline | `PrimitiveTopology`, `PolygonMode`, `CullMode`, `FrontFace` |
| Blending | `BlendFactor`, `BlendOp`, `ColorComponent` |
| Depth | `CompareOp` |
| Sampling | `SampleCount`, `Filter`, `MipmapMode`, `AddressMode`, `BorderColor` |
| Shaders | `ShaderStage` (bitmask), `DescriptorType` |
| Textures | `TextureFormat` (70+ formats), `ImageLayout`, `LoadOp`, `StoreOp` |
| Buffers | `BufferUsage`, `IndexType` |

### RHIDevice (`RHIDevice.h`)

Singleton wrapping the graphics backend.

```cpp
RHIDevice& device = RHIDevice::Get();

// Frame management
device.BeginFrame();
device.BeginSwapchainRenderPass();
device.EndFrame();

// Resource creation
RHIBufferHandle buffer = device.CreateBuffer(spec);
RHITextureHandle texture = device.CreateTexture(spec);
RHISamplerHandle sampler = device.CreateSampler(spec);
RHIShaderModuleHandle shader = device.CreateShaderModule(stage, spirvCode);
auto [pipeline, layout] = device.CreateGraphicsPipeline(spec);

// Descriptor management
RHIDescriptorSetLayoutHandle layout = device.CreateDescriptorSetLayout(bindings);
RHIDescriptorSetHandle set = device.AllocateDescriptorSet(layout);
device.UpdateDescriptorSet(set, writes);

// Synchronization
device.ImmediateSubmit([](RHICommandBufferHandle cmd) { /* one-shot GPU work */ });
device.WaitIdle();

// State queries
uint32_t frameIndex = device.GetCurrentFrameIndex();  // 0 or 1
RHICommandBufferHandle cmd = device.GetCurrentCommandBuffer();
```

---

## Vulkan Backend

Location: `Engine/src/Platform/Vulkan/`

### VulkanContext (`VulkanContext.h/cpp`)

Low-level Vulkan management.

```cpp
VulkanContext& ctx = VulkanContext::Get();

// Vulkan handles
VkInstance instance = ctx.GetInstance();
VkDevice device = ctx.GetDevice();
VkPhysicalDevice physicalDevice = ctx.GetPhysicalDevice();
VkQueue graphicsQueue = ctx.GetGraphicsQueue();
VkRenderPass renderPass = ctx.GetRenderPass();
VkCommandBuffer cmd = ctx.GetCurrentCommandBuffer();
VkDescriptorPool pool = ctx.GetDescriptorPool();
VmaAllocator allocator = ctx.GetAllocator();

// Frame state
uint32_t frameIndex = ctx.GetCurrentFrameIndex();
VkExtent2D extent = ctx.GetSwapchainExtent();
static constexpr uint32_t MAX_FRAMES_IN_FLIGHT = 2;
```

**Key Features:**
- Double-buffered rendering (2 frames in flight)
- VMA (Vulkan Memory Allocator) integration
- Automatic swapchain recreation on resize
- Validation layers in debug builds

### VulkanResourceRegistry (`VulkanRHI.h/cpp`)

Maps opaque RHI handles to Vulkan objects.

```cpp
VulkanResourceRegistry& registry = VulkanResourceRegistry::Get();

// Registration
RHIPipelineHandle handle = registry.RegisterPipeline(vkPipeline, vkLayout);
RHITextureHandle texHandle = registry.RegisterTexture(image, view, sampler, alloc, w, h, fmt);

// Lookup
VkPipeline pipeline = registry.GetPipeline(handle);
VkImageView view = registry.GetTextureView(texHandle);

// Cleanup
registry.UnregisterPipeline(handle);
```

### Type Conversions

```cpp
// RHI enum to Vulkan
VkFormat vkFormat = ToVulkan(TextureFormat::R8G8B8A8_UNORM);
VkShaderStageFlags vkStages = ToVulkan(ShaderStage::VertexFragment);

// Vulkan to RHI enum
TextureFormat format = FromVulkan(VK_FORMAT_R8G8B8A8_UNORM);
```

---

## Renderer System

Location: `Engine/src/GGEngine/Renderer/`

### Renderer2D (`Renderer2D.h/cpp`)

Static batched 2D rendering API with bindless textures.

```cpp
// Basic usage
Renderer2D::BeginScene(camera);
Renderer2D::DrawQuad(QuadSpec().SetPosition(0, 0).SetSize(1, 1).SetColor(1, 0, 0, 1));
Renderer2D::EndScene();

// With custom render target
Renderer2D::BeginScene(camera, renderPass, cmd, width, height);

// With ECS camera
Renderer2D::BeginScene(sceneCamera, transformMatrix, renderPass, cmd, width, height);
```

**QuadSpec (Preferred API):**
```cpp
struct QuadSpec {
    float x, y, z = 0.0f;           // Position
    float width = 1.0f, height = 1.0f;
    float rotation = 0.0f;          // Radians
    float color[4] = {1,1,1,1};     // RGBA
    const Texture* texture = nullptr;
    float tilingFactor = 1.0f;
    const float (*texCoords)[2] = nullptr;
    const glm::mat4* transform = nullptr;  // Overrides position/size/rotation
    const SubTexture2D* subTexture = nullptr;

    // Fluent API
    QuadSpec& SetPosition(float x, float y, float z = 0);
    QuadSpec& SetSize(float w, float h);
    QuadSpec& SetRotation(float radians);
    QuadSpec& SetColor(float r, float g, float b, float a = 1);
    QuadSpec& SetTexture(const Texture* tex, float tiling = 1);
    QuadSpec& SetSubTexture(const SubTexture2D* sub);
    QuadSpec& SetTransform(const glm::mat4* mat);
};
```

**Internal Architecture:**
- Vertex format: position (vec3) + texCoord (vec2) + color (vec4) + tilingFactor (float) + texIndex (uint32)
- Per-frame vertex buffers to avoid GPU/CPU conflicts
- Dynamic buffer growth: 100K → 1M quads max
- Automatic batch flushing when full
- Bindless texture array (16K max textures)

**Statistics:**
```cpp
Renderer2D::ResetStats();
auto stats = Renderer2D::GetStats();
// stats.DrawCalls, stats.QuadCount, stats.MaxQuadCapacity
```

### Camera (`Camera.h`)

Native matrix math implementation.

```cpp
Camera camera;

// Orthographic (2D)
camera.SetOrthographic(width, height, nearPlane, farPlane);
camera.SetOrthoSize(width, height);
camera.SetRotation(radians);  // 2D rotation
camera.Translate(dx, dy);
camera.Zoom(delta);

// Perspective (3D)
camera.SetPerspective(fovDegrees, aspect, near, far);
camera.SetOrbitTarget(x, y, z);
camera.SetOrbitAngles(pitch, yaw);
camera.OrbitRotate(deltaPitch, deltaYaw);

// Get matrices
const Mat4& view = camera.GetViewMatrix();
const Mat4& proj = camera.GetProjectionMatrix();
const Mat4& viewProj = camera.GetViewProjectionMatrix();
CameraUBO ubo = camera.GetUBO();  // For shader uniform
```

### SceneCamera (`SceneCamera.h`)

Camera for ECS system.

```cpp
SceneCamera camera;
camera.SetProjectionType(SceneCamera::ProjectionType::Orthographic);
camera.SetOrthographicSize(10.0f);  // Half-height in world units
camera.SetViewportSize(width, height);
const Mat4& projection = camera.GetProjection();
```

### Pipeline (`Pipeline.h`)

Vulkan graphics pipeline wrapper.

```cpp
PipelineSpecification spec;
spec.shader = myShader;
spec.renderPass = renderPass;
spec.vertexLayout = &layout;
spec.cullMode = CullMode::Back;
spec.blendMode = BlendMode::Alpha;
spec.depthTestEnable = true;
spec.descriptorSetLayouts = { cameraLayout, textureLayout };
spec.pushConstantRanges = { {ShaderStage::Vertex, 0, sizeof(PushData)} };

Pipeline pipeline(spec);
pipeline.Bind(commandBuffer);
```

### VertexLayout (`VertexLayout.h`)

Builder pattern for vertex attributes.

```cpp
VertexLayout layout;
layout.Push("aPosition", VertexAttributeType::Float3)
      .Push("aTexCoord", VertexAttributeType::Float2)
      .Push("aColor", VertexAttributeType::Float4)
      .Push("aTexIndex", VertexAttributeType::UInt);

uint32_t stride = layout.GetStride();
auto binding = layout.GetBindingDescription();
auto attributes = layout.GetAttributeDescriptions();
```

**Attribute Types:**
- `Float`, `Float2`, `Float3`, `Float4`
- `Int`, `Int2`, `Int3`, `Int4`
- `UByte4Norm` (for packed colors)
- `UInt` (for bindless indices)

### Material (`Material.h`)

Shader + property system with push constants.

```cpp
Material material;

// Register properties (before Create)
material.RegisterProperty("uColor", PropertyType::Vec4, ShaderStage::Fragment, 0);
material.RegisterProperty("uTransform", PropertyType::Mat4, ShaderStage::Vertex, 16);

// Create pipeline
MaterialSpecification spec;
spec.shader = shader;
spec.renderPass = renderPass;
spec.blendMode = BlendMode::Alpha;
material.Create(spec);

// Set properties
material.SetVec4("uColor", 1, 0, 0, 1);
material.SetMat4("uTransform", matrix);

// Bind and draw
material.Bind(commandBuffer);
```

### BindlessTextureManager (`BindlessTextureManager.h`)

Global bindless descriptor set for all textures.

```cpp
BindlessTextureManager& btm = BindlessTextureManager::Get();

// Textures auto-register on creation
Texture* tex = ...;
BindlessTextureIndex idx = tex->GetBindlessIndex();

// Manual management
BindlessTextureIndex idx = btm.RegisterTexture(texture);
btm.UnregisterTexture(idx);

// Shader binding
void* descriptorSet = btm.GetDescriptorSet();
RHICmd::BindDescriptorSetRaw(cmd, layout, descriptorSet, 1);
```

---

## Asset System

Location: `Engine/src/GGEngine/Asset/`

### AssetManager (`AssetManager.h`)

Singleton for asset loading and caching.

```cpp
AssetManager& am = AssetManager::Get();

// Load assets (cached) - synchronous, blocks until loaded
AssetHandle<Texture> tex = am.Load<Texture>("assets/textures/sprite.png");
AssetHandle<Shader> shader = am.Load<Shader>("assets/shaders/compiled/quad2d");

// Get cached asset
AssetHandle<Texture> cached = am.GetHandle<Texture>("assets/textures/sprite.png");

// Path resolution
am.SetAssetRoot("C:/Game/");
am.AddSearchPath("assets");
std::filesystem::path resolved = am.ResolvePath("textures/sprite.png");

// Raw file reading
std::vector<char> data = am.ReadFileRaw("shaders/compiled/quad2d.vert.spv");
```

### Async Loading

Load textures without blocking the main thread:

```cpp
// Returns immediately - texture loads in background
AssetHandle<Texture> tex = AssetManager::Get().LoadTextureAsync("sprite.png");

// Check if ready (polling)
if (tex.IsValid() && tex.Get()->IsReady()) {
    // Safe to use texture
}

// Or use callback (fires on main thread)
AssetManager::Get().OnAssetReady(tex.GetID(), [](AssetID id, bool success) {
    if (success) {
        GG_CORE_INFO("Texture loaded!");
    }
});
```

**How it works:**
1. `LoadTextureAsync()` creates the asset handle immediately (state: `Loading`)
2. Worker thread loads file from disk via `Texture::LoadCPU()` (stb_image)
3. CPU data queued for main thread upload
4. `AssetManager::Update()` processes the queue, calls `Texture::UploadGPU()`
5. Callbacks fire after GPU upload completes

### Hot Reload (Debug & Release builds)

Automatic texture reloading when files change on disk. Excluded from Dist builds.

```cpp
#ifndef GG_DIST
// Enable hot reload
AssetManager::Get().EnableHotReload(true);

// Watch a directory for changes
AssetManager::Get().WatchDirectory("assets/textures");

// Optional: get notified when specific asset reloads
AssetManager::Get().OnAssetReload(tex.GetID(), [](AssetID id) {
    GG_CORE_INFO("Texture was reloaded!");
});
#endif
```

**Features:**
- Uses native OS APIs (Windows: `ReadDirectoryChangesW`, others: polling fallback)
- 100ms debounce to handle rapid file changes (e.g., editor auto-save)
- Preserves bindless texture index - no shader reference updates needed
- Callbacks fire on main thread

**Supported formats:** PNG, JPG, JPEG, BMP, TGA, GIF

### AssetHandle (`AssetHandle.h`)

Generation-based handle for stale reference detection.

```cpp
AssetHandle<Texture> handle = AssetManager::Get().Load<Texture>("sprite.png");

if (handle.IsValid()) {
    Texture* tex = handle.Get();
    // Use texture...
}

// Handles become invalid after asset unload
AssetManager::Get().Unload("sprite.png");
// handle.IsValid() now returns false
```

### Shader (`Shader.h`)

SPIR-V shader loading.

```cpp
// Load via asset system (preferred)
AssetHandle<Shader> shader = Shader::Create("assets/shaders/compiled/quad2d");

// Access stages
for (const ShaderStageInfo& stage : shader->GetStages()) {
    ShaderStage type = stage.stage;  // Vertex, Fragment, etc.
    RHIShaderModuleHandle handle = stage.handle;
}

// Check for specific stage
if (shader->HasStage(ShaderStage::Fragment)) {
    RHIShaderModuleHandle frag = shader->GetStageHandle(ShaderStage::Fragment);
}
```

### Texture (`Texture.h`)

Image loading with automatic bindless registration.

```cpp
// Load from file
AssetHandle<Texture> tex = Texture::Create("assets/textures/sprite.png");

// Create programmatically (not managed by AssetManager)
uint32_t whitePixel = 0xFFFFFFFF;
Scope<Texture> whiteTex = Texture::Create(1, 1, &whitePixel, Filter::Nearest, Filter::Nearest);

// Properties
uint32_t w = tex->GetWidth();
uint32_t h = tex->GetHeight();
TextureFormat fmt = tex->GetFormat();
BindlessTextureIndex idx = tex->GetBindlessIndex();

// Filter settings (before load)
tex->SetFilter(Filter::Linear, Filter::Linear);
```

### Libraries

Pre-loaded asset collections.

```cpp
// Shaders
ShaderLibrary& shaders = ShaderLibrary::Get();
shaders.Load("quad2d", "assets/shaders/compiled/quad2d");
AssetHandle<Shader> shader = shaders.Get("quad2d");

// Textures
TextureLibrary& textures = TextureLibrary::Get();
textures.Load("player", "assets/textures/player.png");
AssetHandle<Texture> tex = textures.Get("player");
```

---

## ECS System

Location: `Engine/src/GGEngine/ECS/`

### Entity (`Entity.h`)

```cpp
using Entity = uint32_t;  // Index into component arrays
constexpr Entity InvalidEntity = UINT32_MAX;

struct EntityID {
    uint32_t Index = InvalidEntity;
    uint32_t Generation = 0;  // For stale reference detection
    bool IsValid() const;
};
```

### Scene (`Scene.h`)

Entity and component management.

```cpp
Scene scene("My Scene");

// Entity lifecycle
EntityID player = scene.CreateEntity("Player");
EntityID enemy = scene.CreateEntityWithGUID("Enemy", existingGUID);
scene.DestroyEntity(player);
bool valid = scene.IsEntityValid(player);

// Component access
auto& transform = scene.AddComponent<TransformComponent>(player);
auto& sprite = scene.AddComponent<SpriteRendererComponent>(player, defaultSprite);
scene.RemoveComponent<SpriteRendererComponent>(player);

if (scene.HasComponent<TransformComponent>(player)) {
    TransformComponent* tc = scene.GetComponent<TransformComponent>(player);
}

// Entity lookup
EntityID found = scene.FindEntityByName("Player");
EntityID byGuid = scene.FindEntityByGUID(guid);

// Scene operations
scene.OnUpdate(timestep);
scene.OnRender(camera, renderPass, cmd, width, height);
scene.OnRenderRuntime(renderPass, cmd, width, height);  // Uses primary camera entity
scene.OnViewportResize(width, height);
scene.Clear();

// Iteration
for (Entity idx : scene.GetAllEntities()) {
    EntityID id = scene.GetEntityID(idx);
    // ...
}
```

### Components

**TransformComponent:**
```cpp
struct TransformComponent {
    float Position[3] = {0, 0, 0};  // x, y, z
    float Rotation = 0.0f;          // Degrees (2D, around Z)
    float Scale[2] = {1, 1};        // width, height

    glm::mat4 GetMatrix() const;    // Full transform matrix
    Mat4 GetMat4() const;           // Native Mat4 version
};
```

**SpriteRendererComponent:**
```cpp
struct SpriteRendererComponent {
    float Color[4] = {1, 1, 1, 1};  // RGBA tint
    std::string TextureName;        // From TextureLibrary
    float TilingFactor = 1.0f;

    // Atlas/spritesheet support
    bool UseAtlas = false;
    uint32_t AtlasCellX = 0, AtlasCellY = 0;
    float AtlasCellWidth = 64.0f, AtlasCellHeight = 64.0f;
    float AtlasSpriteWidth = 1.0f, AtlasSpriteHeight = 1.0f;
};
```

**CameraComponent:**
```cpp
struct CameraComponent {
    SceneCamera Camera;
    bool Primary = true;           // Primary camera for scene
    bool FixedAspectRatio = false;
};
```

**TagComponent:**
```cpp
struct TagComponent {
    std::string Name;
    GUID ID;  // Unique identifier for serialization
};
```

**TilemapComponent:**
```cpp
struct TilemapComponent {
    // Grid-based tile data for 2D maps
    // ...
};
```

### SceneSerializer

JSON-based scene save/load.

```cpp
Scene scene;
SceneSerializer serializer(&scene);

serializer.Serialize("scene.json");
serializer.Deserialize("scene.json");
```

---

## Shader System

### GLSL Requirements

- Version: GLSL 4.5
- Extension: `GL_EXT_nonuniform_qualifier` (for bindless textures)

### Shader File Structure

```
Engine/assets/shaders/
├── quad2d.vert          # Source GLSL
├── quad2d.frag
└── compiled/
    ├── quad2d.vert.spv  # Compiled SPIR-V (generated at build time)
    └── quad2d.frag.spv
```

### Adding New Shaders

1. Create `.vert` and `.frag` files in `Engine/assets/shaders/`
2. Add to `SHADER_SOURCES` in `CMakeLists.txt`:
   ```cmake
   set(SHADER_SOURCES
       ${SHADER_SOURCE_DIR}/myshader.vert
       ${SHADER_SOURCE_DIR}/myshader.frag
   )
   ```
3. Rebuild - shaders are compiled automatically via `glslc`

### Renderer2D Shader Layout

**Vertex Shader (quad2d.vert):**
```glsl
#version 450
#extension GL_EXT_nonuniform_qualifier : require

layout(set = 0, binding = 0) uniform CameraUBO {
    mat4 view;
    mat4 projection;
    mat4 viewProjection;
} camera;

layout(location = 0) in vec3 aPosition;
layout(location = 1) in vec2 aTexCoord;
layout(location = 2) in vec4 aColor;
layout(location = 3) in float aTilingFactor;
layout(location = 4) in uint aTexIndex;

layout(location = 0) out vec2 vTexCoord;
layout(location = 1) out vec4 vColor;
layout(location = 2) out float vTilingFactor;
layout(location = 3) flat out uint vTexIndex;

void main() {
    gl_Position = camera.viewProjection * vec4(aPosition, 1.0);
    vTexCoord = aTexCoord;
    vColor = aColor;
    vTilingFactor = aTilingFactor;
    vTexIndex = aTexIndex;
}
```

**Fragment Shader (quad2d.frag):**
```glsl
#version 450
#extension GL_EXT_nonuniform_qualifier : require

// Separate sampler + texture array (MoltenVK compatible)
layout(set = 1, binding = 0) uniform sampler uSampler;
layout(set = 1, binding = 1) uniform texture2D uTextures[];

layout(location = 0) in vec2 vTexCoord;
layout(location = 1) in vec4 vColor;
layout(location = 2) in float vTilingFactor;
layout(location = 3) flat in uint vTexIndex;

layout(location = 0) out vec4 outColor;

void main() {
    vec4 texColor = texture(
        sampler2D(uTextures[nonuniformEXT(vTexIndex)], uSampler),
        vTexCoord * vTilingFactor
    );
    outColor = texColor * vColor;
}
```

---

## Platform Layer

Location: `Engine/src/Platform/`

### Window Interface (`Core/Window.h`)

```cpp
class Window {
public:
    virtual void OnUpdate() = 0;              // Poll events
    virtual uint32_t GetWidth() const = 0;
    virtual uint32_t GetHeight() const = 0;
    virtual void GetContentScale(float* x, float* y) const = 0;  // HiDPI
    virtual void SetEventCallback(const EventCallbackFn& cb) = 0;
    virtual void SetVSync(bool enabled) = 0;
    virtual bool IsVSync() const = 0;
    virtual void* GetNativeWindow() const = 0;  // GLFWwindow*

    static Window* Create(const WindowProps& props = WindowProps());
};
```

### Input (`Core/Input.h`)

Polling-based input API.

```cpp
// Keyboard
bool pressed = Input::IsKeyPressed(KeyCode::W);
bool pressed = Input::IsKeyPressed(Key::Space);

// Mouse
bool clicked = Input::IsMouseButtonPressed(MouseButton::Left);
auto [x, y] = Input::GetMousePosition();
float x = Input::GetMouseX();
float y = Input::GetMouseY();
```

### Platform Implementations

| Platform | Window | Input |
|----------|--------|-------|
| Windows | `WindowsWindow.cpp` | `WindowsInput.cpp` |
| Linux/macOS | `GLFWWindow.cpp` | `GLFWInput.cpp` |

---

## Utilities

Location: `Engine/src/GGEngine/Utils/`

### FileWatcher (`Utils/FileWatcher.h`)

Cross-platform file change detection for hot reload support.

```cpp
FileWatcher watcher;

// Watch a directory (recursive)
watcher.Watch("C:/Game/assets/textures", [](const std::filesystem::path& path, FileChangeType type) {
    switch (type) {
        case FileChangeType::Modified: /* file changed */ break;
        case FileChangeType::Created:  /* new file */ break;
        case FileChangeType::Deleted:  /* file removed */ break;
        case FileChangeType::Renamed:  /* file renamed */ break;
    }
});

// Poll for changes (call every frame)
uint32_t changeCount = watcher.Update();

// Stop watching
watcher.Unwatch("C:/Game/assets/textures");
watcher.UnwatchAll();

// Enable/disable
watcher.SetEnabled(false);
```

**Platform implementations:**
- **Windows:** Native `ReadDirectoryChangesW` with async I/O (low latency)
- **Linux/macOS:** Polling fallback (500ms interval)

---

## Key Design Patterns

### 1. Singleton Pattern

Used for global managers:
```cpp
// Access pattern
Application::Get()
VulkanContext::Get()
RHIDevice::Get()
AssetManager::Get()
BindlessTextureManager::Get()
ShaderLibrary::Get()
TextureLibrary::Get()
VulkanResourceRegistry::Get()
```

### 2. Handle-Based Resources

Opaque 64-bit IDs avoid virtual dispatch and header pollution:
```cpp
RHITextureHandle handle = device.CreateTexture(spec);
// handle.id = 42, maps to VkImage internally
device.DestroyTexture(handle);
```

### 3. Builder Pattern

For complex object construction:
```cpp
VertexLayout layout;
layout.Push("pos", Float3).Push("uv", Float2);

QuadSpec spec;
spec.SetPosition(0, 0).SetSize(2, 2).SetColor(1, 0, 0, 1);
```

### 4. Generation IDs

Detect stale references to destroyed entities/assets:
```cpp
struct EntityID {
    uint32_t Index;       // Slot index
    uint32_t Generation;  // Incremented on destroy
};

// Validation: stored generation must match current
bool IsValid(EntityID id) {
    return m_Generations[id.Index] == id.Generation;
}
```

### 5. SoA ECS (Structure of Arrays)

Cache-friendly component storage:
```cpp
class Scene {
    ComponentStorage<TransformComponent> m_Transforms;
    ComponentStorage<SpriteRendererComponent> m_Sprites;
    // ...
};
```

### 6. Double Buffering

Per-frame resources to avoid GPU/CPU conflicts:
```cpp
static constexpr uint32_t MaxFramesInFlight = 2;
Scope<VertexBuffer> QuadVertexBuffers[MaxFramesInFlight];
Scope<UniformBuffer> CameraUniformBuffers[MaxFramesInFlight];

// Select buffer based on current frame
uint32_t frame = RHIDevice::Get().GetCurrentFrameIndex();
QuadVertexBuffers[frame]->SetData(...);
```

---

## Adding New Features

### Adding a New Component

1. Create header in `Engine/src/GGEngine/ECS/Components/`:
   ```cpp
   // MyComponent.h
   #pragma once
   namespace GGEngine {
       struct MyComponent {
           float Value = 0.0f;
       };
   }
   ```

2. Include in `Components.h`:
   ```cpp
   #include "Components/MyComponent.h"
   ```

3. Add storage in `Scene.h`:
   ```cpp
   ComponentStorage<MyComponent> m_MyComponents;
   ```

4. Add template specializations in `Scene.h`:
   ```cpp
   template<>
   inline ComponentStorage<MyComponent>& Scene::GetStorage<MyComponent>() {
       return m_MyComponents;
   }
   template<>
   inline const ComponentStorage<MyComponent>& Scene::GetStorage<MyComponent>() const {
       return m_MyComponents;
   }
   ```

5. Update `SceneSerializer` if component should be saved.

### Adding a New Shader

1. Create GLSL files in `Engine/assets/shaders/`
2. Add to `CMakeLists.txt` `SHADER_SOURCES`
3. Create corresponding Material or Pipeline usage

### Adding a New Asset Type

1. Create class inheriting `Asset`:
   ```cpp
   class MyAsset : public Asset {
   public:
       AssetType GetType() const override { return AssetType::Custom; }
       bool Load(const std::string& path) override;
       void Unload() override;
   };
   ```

2. Add type specialization:
   ```cpp
   template<>
   constexpr AssetType GetAssetType<MyAsset>() { return AssetType::Custom; }
   ```

3. Add to `AssetType` enum if needed.

### Adding Platform Support

1. Create implementation in `Engine/src/Platform/YourPlatform/`
2. Update `CMakeLists.txt` with platform conditionals
3. Add platform detection macro in `Core.h`
