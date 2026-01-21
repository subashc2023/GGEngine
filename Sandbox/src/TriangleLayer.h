#pragma once

#include "GGEngine/Core/Layer.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Events/MouseEvent.h"
#include "GGEngine/Renderer/Material.h"
#include "GGEngine/Renderer/VertexBuffer.h"
#include "GGEngine/Renderer/IndexBuffer.h"
#include "GGEngine/Renderer/VertexLayout.h"
#include "GGEngine/Renderer/UniformBuffer.h"
#include "GGEngine/Renderer/DescriptorSet.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/Asset/AssetHandle.h"

class TriangleLayer : public GGEngine::Layer
{
public:
    TriangleLayer();
    virtual ~TriangleLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnEvent(GGEngine::Event& event) override;

private:
    GGEngine::Scope<GGEngine::Material> m_Material;
    GGEngine::Scope<GGEngine::VertexBuffer> m_VertexBuffer;
    GGEngine::Scope<GGEngine::IndexBuffer> m_IndexBuffer;
    GGEngine::VertexLayout m_VertexLayout;

    // Grid geometry
    GGEngine::Scope<GGEngine::VertexBuffer> m_QuadVertexBuffer;
    GGEngine::Scope<GGEngine::IndexBuffer> m_QuadIndexBuffer;

    // Textured quad
    GGEngine::Scope<GGEngine::Material> m_TextureMaterial;
    GGEngine::Scope<GGEngine::VertexBuffer> m_TexturedQuadVertexBuffer;
    GGEngine::Scope<GGEngine::IndexBuffer> m_TexturedQuadIndexBuffer;
    GGEngine::VertexLayout m_TexturedVertexLayout;
    GGEngine::AssetHandle<GGEngine::Texture> m_CheckerboardTexture;
    GGEngine::Scope<GGEngine::DescriptorSetLayout> m_TextureDescriptorLayout;
    GGEngine::Scope<GGEngine::DescriptorSet> m_TextureDescriptorSet;

    // Camera system
    GGEngine::OrthographicCameraController m_CameraController;
    GGEngine::Scope<GGEngine::DescriptorSetLayout> m_CameraDescriptorLayout;
    GGEngine::Scope<GGEngine::DescriptorSet> m_CameraDescriptorSet;
    GGEngine::Scope<GGEngine::UniformBuffer> m_CameraUniformBuffer;

    // Triangle transform
    float m_Position[3] = { 0.0f, 0.0f, 0.0f };
    float m_TriangleMoveSpeed = 2.0f;

    // Color multiplier
    float m_ColorMultiplier[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
};
