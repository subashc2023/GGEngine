#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/RHI/RHITypes.h"
#include <cstdint>

namespace GGEngine {

    class Texture;

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

        static void EndScene();
        static void Flush();

        // Colored quads (uses white pixel texture internally)
        static void DrawQuad(float x, float y, float width, float height,
                            float r, float g, float b, float a = 1.0f);

        static void DrawQuad(float x, float y, float z, float width, float height,
                            float r, float g, float b, float a = 1.0f);

        // Colored quads with rotation (radians)
        static void DrawRotatedQuad(float x, float y, float width, float height,
                                   float rotationRadians,
                                   float r, float g, float b, float a = 1.0f);

        static void DrawRotatedQuad(float x, float y, float z, float width, float height,
                                   float rotationRadians,
                                   float r, float g, float b, float a = 1.0f);

        // Textured quads
        static void DrawQuad(float x, float y, float width, float height,
                            const Texture* texture,
                            float tilingFactor = 1.0f,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        static void DrawQuad(float x, float y, float z, float width, float height,
                            const Texture* texture,
                            float tilingFactor = 1.0f,
                            float tintR = 1.0f, float tintG = 1.0f,
                            float tintB = 1.0f, float tintA = 1.0f);

        // Textured quads with rotation (radians)
        static void DrawRotatedQuad(float x, float y, float width, float height,
                                   float rotationRadians,
                                   const Texture* texture,
                                   float tilingFactor = 1.0f,
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        static void DrawRotatedQuad(float x, float y, float z, float width, float height,
                                   float rotationRadians,
                                   const Texture* texture,
                                   float tilingFactor = 1.0f,
                                   float tintR = 1.0f, float tintG = 1.0f,
                                   float tintB = 1.0f, float tintA = 1.0f);

        // Statistics
        struct Statistics
        {
            uint32_t DrawCalls = 0;
            uint32_t QuadCount = 0;
        };

        static void ResetStats();
        static Statistics GetStats();
    };

}
