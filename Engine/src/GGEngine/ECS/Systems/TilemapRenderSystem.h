#pragma once

#include "RenderSystem.h"

namespace GGEngine {

    // =============================================================================
    // TilemapRenderSystem
    // =============================================================================
    // Renders all entities with TilemapComponent and TransformComponent.
    // Uses Renderer2D batched rendering for tilemaps.
    //
    // Tilemaps are rendered before sprites (lower z-order typically) so this
    // system should be registered before SpriteRenderSystem in the scheduler.
    //
    class GG_API TilemapRenderSystem : public IRenderSystem
    {
    public:
        TilemapRenderSystem() = default;
        ~TilemapRenderSystem() override = default;

        // ISystem interface
        std::vector<ComponentRequirement> GetRequirements() const override;
        void Execute(Scene& scene, float deltaTime) override;
        const char* GetName() const override { return "TilemapRenderSystem"; }

    private:
        void RenderTilemaps(Scene& scene);
    };

}
