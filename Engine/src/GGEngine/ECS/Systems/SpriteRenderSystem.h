#pragma once

#include "RenderSystem.h"

namespace GGEngine {

    // =============================================================================
    // SpriteRenderSystem
    // =============================================================================
    // Renders all entities with SpriteRendererComponent and TransformComponent.
    // Supports both batched (Renderer2D) and instanced (InstancedRenderer2D) modes.
    //
    // Batched mode: Good for small numbers of sprites, simpler to debug
    // Instanced mode: Better for large numbers (10k+) sprites, parallel preparation
    //
    class GG_API SpriteRenderSystem : public IRenderSystem
    {
    public:
        enum class RenderMode
        {
            Batched,    // Use Renderer2D batched quads
            Instanced   // Use InstancedRenderer2D with parallel preparation
        };

        explicit SpriteRenderSystem(RenderMode mode = RenderMode::Batched);
        ~SpriteRenderSystem() override = default;

        // ISystem interface
        std::vector<ComponentRequirement> GetRequirements() const override;
        void Execute(Scene& scene, float deltaTime) override;
        const char* GetName() const override { return "SpriteRenderSystem"; }

        // Render mode control
        void SetRenderMode(RenderMode mode) { m_RenderMode = mode; }
        RenderMode GetRenderMode() const { return m_RenderMode; }

    private:
        void RenderBatched(Scene& scene);
        void RenderInstanced(Scene& scene);

        RenderMode m_RenderMode;
    };

}
