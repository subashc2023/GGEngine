#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/RHI/RHITypes.h"
#include <glm/glm.hpp>
#include <cstdint>

namespace GGEngine {

    class Texture;
    class SubTexture2D;
    class SceneCamera;

    // Unified specification for drawing a quad
    // This is the PREFERRED API for all quad rendering.
    // Use Renderer2D::DrawQuad(QuadSpec) for clean, maintainable code.
    struct GG_API QuadSpec
    {
        // Position (world space)
        float x = 0.0f;
        float y = 0.0f;
        float z = 0.0f;

        // Size
        float width = 1.0f;
        float height = 1.0f;

        // Rotation (radians, around center)
        float rotation = 0.0f;

        // Color/tint (RGBA)
        float color[4] = { 1.0f, 1.0f, 1.0f, 1.0f };

        // Texture (optional, nullptr = solid color)
        const Texture* texture = nullptr;
        float tilingFactor = 1.0f;

        // Custom UVs (optional, nullptr = full texture [0,0] to [1,1])
        // Format: 4 vertices * 2 floats (u,v) = float[4][2]
        const float (*texCoords)[2] = nullptr;

        // Pre-computed transform matrix (optional)
        // If set, overrides x/y/z, width/height, and rotation
        const glm::mat4* transform = nullptr;

        // SubTexture for sprite sheets (optional)
        // If set, extracts texture and UVs from the SubTexture2D
        const SubTexture2D* subTexture = nullptr;

        // Helper methods for fluent API
        QuadSpec& SetPosition(float px, float py, float pz = 0.0f)
        {
            x = px; y = py; z = pz;
            return *this;
        }

        QuadSpec& SetSize(float w, float h)
        {
            width = w; height = h;
            return *this;
        }

        QuadSpec& SetRotation(float radians)
        {
            rotation = radians;
            return *this;
        }

        QuadSpec& SetColor(float r, float g, float b, float a = 1.0f)
        {
            color[0] = r; color[1] = g; color[2] = b; color[3] = a;
            return *this;
        }

        QuadSpec& SetTexture(const Texture* tex, float tiling = 1.0f)
        {
            texture = tex;
            tilingFactor = tiling;
            return *this;
        }

        QuadSpec& SetTexCoords(const float (*coords)[2])
        {
            texCoords = coords;
            return *this;
        }

        QuadSpec& SetTransform(const glm::mat4* mat)
        {
            transform = mat;
            return *this;
        }

        QuadSpec& SetSubTexture(const SubTexture2D* sub)
        {
            subTexture = sub;
            return *this;
        }
    };

    class GG_API Renderer2D
    {
    public:
        // Lifecycle - called by Application
        static void Init();
        static void Shutdown();

        // Scene management
        // Default: renders to swapchain
        static void BeginScene(const Camera& camera);

        // Custom render pass (RHI handles)
        static void BeginScene(const Camera& camera, RHIRenderPassHandle renderPass,
                              RHICommandBufferHandle cmd, uint32_t viewportWidth, uint32_t viewportHeight);

        // SceneCamera + transform matrix (for ECS camera system)
        static void BeginScene(const SceneCamera& camera, const Mat4& transform);

        static void BeginScene(const SceneCamera& camera, const Mat4& transform,
                              RHIRenderPassHandle renderPass, RHICommandBufferHandle cmd,
                              uint32_t viewportWidth, uint32_t viewportHeight);

        static void EndScene();
        static void Flush();

        // =========================================================================
        // PREFERRED API: Use QuadSpec for all new code
        // =========================================================================
        static void DrawQuad(const QuadSpec& spec);

        // =========================================================================
        // DEPRECATED API: Use DrawQuad(QuadSpec) instead
        // These overloads are maintained for backwards compatibility
        // =========================================================================

        // Colored quads (uses white pixel texture internally)
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float width, float height,
                            float r, float g, float b, float a = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float z, float width, float height,
                            float r, float g, float b, float a = 1.0f);

        // Colored quads with rotation (radians)
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float width, float height,
                                   float rotationRadians,
                                   float r, float g, float b, float a = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float z, float width, float height,
                                   float rotationRadians,
                                   float r, float g, float b, float a = 1.0f);

        // Textured quads
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float width, float height,
                            const Texture* texture,
                            float tilingFactor = 1.0f,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float z, float width, float height,
                            const Texture* texture,
                            float tilingFactor = 1.0f,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        // Textured quads with rotation (radians)
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float width, float height,
                                   float rotationRadians,
                                   const Texture* texture,
                                   float tilingFactor = 1.0f,
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float z, float width, float height,
                                   float rotationRadians,
                                   const Texture* texture,
                                   float tilingFactor = 1.0f,
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        // Sub-textured quads (for sprite sheets / texture atlases)
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float width, float height,
                            const SubTexture2D* subTexture,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float z, float width, float height,
                            const SubTexture2D* subTexture,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        // Sub-textured quads with rotation
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float width, float height,
                                   float rotationRadians,
                                   const SubTexture2D* subTexture,
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float z, float width, float height,
                                   float rotationRadians,
                                   const SubTexture2D* subTexture,
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        // Raw UV quads (no allocation - for performance-critical paths)
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(float x, float y, float z, float width, float height,
                            const Texture* texture,
                            const float texCoords[4][2],
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawRotatedQuad(float x, float y, float z, float width, float height,
                                   float rotationRadians,
                                   const Texture* texture,
                                   const float texCoords[4][2],
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        // Matrix-based quads (for pre-computed transforms)
        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(const glm::mat4& transform,
                            float r, float g, float b, float a = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(const glm::mat4& transform,
                            const Texture* texture,
                            float tilingFactor = 1.0f,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        GG_DEPRECATED("Use DrawQuad(QuadSpec) instead")
        static void DrawQuad(const glm::mat4& transform,
                            const SubTexture2D* subTexture,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        // Statistics
        struct Statistics
        {
            uint32_t DrawCalls = 0;
            uint32_t QuadCount = 0;
            uint32_t MaxQuadCapacity = 0;  // Current buffer capacity
        };

        static void ResetStats();
        static Statistics GetStats();
    };

}
