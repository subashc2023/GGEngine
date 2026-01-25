#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include <glm/glm.hpp>
#include <cstdint>
#include <atomic>

namespace GGEngine {

    class Camera;
    class SceneCamera;

    // Per-instance data for GPU instancing (80 bytes, aligned)
    struct GG_API QuadInstanceData
    {
        // Transform (decomposed TRS - GPU reconstructs matrix)
        float Position[3];      // 12 bytes - world position (x, y, z)
        float Rotation;         // 4 bytes  - rotation in radians (Z-axis)
        float Scale[2];         // 8 bytes  - size (width, height)
        float _pad1[2];         // 8 bytes  - padding for alignment

        // Appearance
        float Color[4];         // 16 bytes - RGBA tint
        float TexCoords[4];     // 16 bytes - UV bounds (minU, minV, maxU, maxV)

        // Texture
        uint32_t TexIndex;      // 4 bytes  - bindless texture index
        float TilingFactor;     // 4 bytes  - texture tiling multiplier
        float _pad2[2];         // 8 bytes  - padding to 80 bytes

        // Helper to set from decomposed transform
        void SetTransform(float x, float y, float z, float rotation, float width, float height)
        {
            Position[0] = x;
            Position[1] = y;
            Position[2] = z;
            Rotation = rotation;
            Scale[0] = width;
            Scale[1] = height;
        }

        // Helper to set color
        void SetColor(float r, float g, float b, float a)
        {
            Color[0] = r;
            Color[1] = g;
            Color[2] = b;
            Color[3] = a;
        }

        // Helper to set full texture UVs
        void SetFullTexture(uint32_t texIndex, float tiling = 1.0f)
        {
            TexCoords[0] = 0.0f;  // minU
            TexCoords[1] = 0.0f;  // minV
            TexCoords[2] = 1.0f;  // maxU
            TexCoords[3] = 1.0f;  // maxV
            TexIndex = texIndex;
            TilingFactor = tiling;
        }

        // Helper to set atlas/subtexture UVs
        void SetTexCoords(float minU, float minV, float maxU, float maxV, uint32_t texIndex, float tiling = 1.0f)
        {
            TexCoords[0] = minU;
            TexCoords[1] = minV;
            TexCoords[2] = maxU;
            TexCoords[3] = maxV;
            TexIndex = texIndex;
            TilingFactor = tiling;
        }
    };

    static_assert(sizeof(QuadInstanceData) == 80, "QuadInstanceData must be 80 bytes for GPU alignment");

    class GG_API InstancedRenderer2D
    {
    public:
        // Lifecycle - called by Application
        static void Init(uint32_t initialMaxInstances = 100000);
        static void Shutdown();

        // Scene management
        static void BeginScene(const Camera& camera);
        static void BeginScene(const Camera& camera, RHIRenderPassHandle renderPass,
                              RHICommandBufferHandle cmd, uint32_t viewportWidth, uint32_t viewportHeight);
        static void BeginScene(const SceneCamera& camera, const glm::mat4& transform);
        static void BeginScene(const SceneCamera& camera, const glm::mat4& transform,
                              RHIRenderPassHandle renderPass, RHICommandBufferHandle cmd,
                              uint32_t viewportWidth, uint32_t viewportHeight);
        static void EndScene();

        // Thread-safe instance allocation for parallel preparation
        // Returns pointer to contiguous instance data for writing
        // Returns nullptr if capacity exceeded
        static QuadInstanceData* AllocateInstances(uint32_t count);

        // Single instance submission (convenience, not thread-safe with AllocateInstances)
        static void SubmitInstance(const QuadInstanceData& instance);

        // Get white texture index for solid color rendering
        static uint32_t GetWhiteTextureIndex();

        // Statistics
        struct Statistics
        {
            uint32_t DrawCalls = 0;
            uint32_t InstanceCount = 0;
            uint32_t MaxInstanceCapacity = 0;
        };

        static void ResetStats();
        static Statistics GetStats();
    };

}
