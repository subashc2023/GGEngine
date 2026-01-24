#include "ggpch.h"
#include "SpriteRenderSystem.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Components.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Renderer/InstancedRenderer2D.h"
#include "GGEngine/Renderer/SubTexture2D.h"
#include "GGEngine/Renderer/SceneCamera.h"
#include "GGEngine/Asset/TextureLibrary.h"
#include "GGEngine/Core/Math.h"
#include "GGEngine/Core/TaskGraph.h"

namespace GGEngine {

    SpriteRenderSystem::SpriteRenderSystem(RenderMode mode)
        : m_RenderMode(mode)
    {
    }

    std::vector<ComponentRequirement> SpriteRenderSystem::GetRequirements() const
    {
        return {
            Require<SpriteRendererComponent>(AccessMode::Read),
            Require<TransformComponent>(AccessMode::Read)
        };
    }

    void SpriteRenderSystem::Execute(Scene& scene, float deltaTime)
    {
        (void)deltaTime;  // Not used for rendering

        if (!m_RenderContext.IsValid())
        {
            GG_CORE_WARN("SpriteRenderSystem::Execute - Invalid render context");
            return;
        }

        if (m_RenderMode == RenderMode::Batched)
        {
            RenderBatched(scene);
        }
        else
        {
            RenderInstanced(scene);
        }
    }

    void SpriteRenderSystem::RenderBatched(Scene& scene)
    {
        // Begin scene with appropriate camera
        Renderer2D::ResetStats();

        if (m_RenderContext.UsesRuntimeCamera())
        {
            Renderer2D::BeginScene(
                *m_RenderContext.RuntimeCamera,
                *m_RenderContext.CameraTransform,
                m_RenderContext.RenderPass,
                m_RenderContext.CommandBuffer,
                m_RenderContext.ViewportWidth,
                m_RenderContext.ViewportHeight
            );
        }
        else
        {
            Renderer2D::BeginScene(
                *m_RenderContext.ExternalCamera,
                m_RenderContext.RenderPass,
                m_RenderContext.CommandBuffer,
                m_RenderContext.ViewportWidth,
                m_RenderContext.ViewportHeight
            );
        }

        // Render sprites
        auto& textureLib = TextureLibrary::Get();
        auto& sprites = scene.GetStorage<SpriteRendererComponent>();
        auto& transforms = scene.GetStorage<TransformComponent>();

        for (size_t i = 0; i < sprites.Size(); i++)
        {
            Entity entity = sprites.GetEntity(i);

            const TransformComponent* transform = transforms.Get(entity);
            if (!transform) continue;

            const SpriteRendererComponent& sprite = sprites.Data()[i];

            float rotationRadians = Math::ToRadians(transform->Rotation);

            // Resolve texture from library
            Texture* texture = nullptr;
            if (!sprite.TextureName.empty())
            {
                texture = textureLib.GetTexturePtr(sprite.TextureName);
            }

            // Build QuadSpec for this sprite
            QuadSpec spec;
            spec.x = transform->Position[0];
            spec.y = transform->Position[1];
            spec.z = transform->Position[2];
            spec.width = transform->Scale[0];
            spec.height = transform->Scale[1];
            spec.rotation = rotationRadians;
            spec.color[0] = sprite.Color[0];
            spec.color[1] = sprite.Color[1];
            spec.color[2] = sprite.Color[2];
            spec.color[3] = sprite.Color[3];

            if (texture)
            {
                spec.texture = texture;

                if (sprite.UseAtlas)
                {
                    // Spritesheet/Atlas rendering - calculate UVs on stack
                    float texCoords[4][2];
                    SubTexture2D::CalculateGridUVs(
                        texture,
                        sprite.AtlasCellX, sprite.AtlasCellY,
                        sprite.AtlasCellWidth, sprite.AtlasCellHeight,
                        sprite.AtlasSpriteWidth, sprite.AtlasSpriteHeight,
                        texCoords
                    );
                    spec.texCoords = texCoords;
                }
                else
                {
                    spec.tilingFactor = sprite.TilingFactor;
                }
            }

            Renderer2D::DrawQuad(spec);
        }

        Renderer2D::EndScene();
    }

    void SpriteRenderSystem::RenderInstanced(Scene& scene)
    {
        auto& textureLib = TextureLibrary::Get();
        auto& taskGraph = TaskGraph::Get();
        auto& sprites = scene.GetStorage<SpriteRendererComponent>();
        auto& transforms = scene.GetStorage<TransformComponent>();

        const size_t spriteCount = sprites.Size();
        if (spriteCount == 0)
            return;

        // Begin scene with appropriate camera
        InstancedRenderer2D::ResetStats();

        if (m_RenderContext.UsesRuntimeCamera())
        {
            InstancedRenderer2D::BeginScene(
                *m_RenderContext.RuntimeCamera,
                *m_RenderContext.CameraTransform,
                m_RenderContext.RenderPass,
                m_RenderContext.CommandBuffer,
                m_RenderContext.ViewportWidth,
                m_RenderContext.ViewportHeight
            );
        }
        else
        {
            InstancedRenderer2D::BeginScene(
                *m_RenderContext.ExternalCamera,
                m_RenderContext.RenderPass,
                m_RenderContext.CommandBuffer,
                m_RenderContext.ViewportWidth,
                m_RenderContext.ViewportHeight
            );
        }

        // Allocate instance buffer space (thread-safe)
        QuadInstanceData* instances = InstancedRenderer2D::AllocateInstances(static_cast<uint32_t>(spriteCount));
        if (!instances)
        {
            GG_CORE_WARN("SpriteRenderSystem::RenderInstanced - Failed to allocate instance buffer");
            InstancedRenderer2D::EndScene();
            return;
        }

        // Get raw data pointers for parallel access
        const SpriteRendererComponent* spriteData = sprites.Data();
        const uint32_t whiteTexIndex = InstancedRenderer2D::GetWhiteTextureIndex();

        // Determine chunk size based on worker count
        const uint32_t workerCount = taskGraph.GetWorkerCount();
        const size_t minChunkSize = 256;
        const size_t chunkSize = std::max(minChunkSize, (spriteCount + workerCount) / (workerCount + 1));

        // Create parallel tasks for instance buffer preparation
        std::vector<TaskID> tasks;
        tasks.reserve((spriteCount + chunkSize - 1) / chunkSize);

        for (size_t start = 0; start < spriteCount; start += chunkSize)
        {
            size_t end = std::min(start + chunkSize, spriteCount);

            TaskID taskId = taskGraph.CreateTask("PrepareInstances",
                [&textureLib, &sprites, &transforms, spriteData, instances, whiteTexIndex, start, end]() -> TaskResult
                {
                    for (size_t i = start; i < end; ++i)
                    {
                        Entity entity = sprites.GetEntity(i);

                        const TransformComponent* transform = transforms.Get(entity);
                        if (!transform)
                            continue;

                        const SpriteRendererComponent& sprite = spriteData[i];
                        QuadInstanceData& inst = instances[i];

                        // Set transform (position, rotation, scale)
                        inst.SetTransform(
                            transform->Position[0],
                            transform->Position[1],
                            transform->Position[2],
                            Math::ToRadians(transform->Rotation),
                            transform->Scale[0],
                            transform->Scale[1]
                        );

                        // Set color
                        inst.SetColor(
                            sprite.Color[0],
                            sprite.Color[1],
                            sprite.Color[2],
                            sprite.Color[3]
                        );

                        // Resolve texture and UVs
                        uint32_t texIndex = whiteTexIndex;
                        float minU = 0.0f, minV = 0.0f, maxU = 1.0f, maxV = 1.0f;
                        float tiling = sprite.TilingFactor;

                        if (!sprite.TextureName.empty())
                        {
                            Texture* texture = textureLib.GetTexturePtr(sprite.TextureName);
                            if (texture)
                            {
                                texIndex = texture->GetBindlessIndex();

                                if (sprite.UseAtlas && texture->GetWidth() > 0 && texture->GetHeight() > 0)
                                {
                                    // Calculate atlas UVs
                                    float texWidth = static_cast<float>(texture->GetWidth());
                                    float texHeight = static_cast<float>(texture->GetHeight());

                                    minU = (sprite.AtlasCellX * sprite.AtlasCellWidth) / texWidth;
                                    minV = (sprite.AtlasCellY * sprite.AtlasCellHeight) / texHeight;
                                    maxU = minU + (sprite.AtlasSpriteWidth * sprite.AtlasCellWidth) / texWidth;
                                    maxV = minV + (sprite.AtlasSpriteHeight * sprite.AtlasCellHeight) / texHeight;
                                }
                            }
                        }

                        inst.SetTexCoords(minU, minV, maxU, maxV, texIndex, tiling);
                    }

                    return TaskResult::Success();
                },
                JobPriority::High
            );

            tasks.push_back(taskId);
        }

        // Wait for all preparation tasks to complete
        taskGraph.WaitAll(tasks);

        InstancedRenderer2D::EndScene();
    }

}
