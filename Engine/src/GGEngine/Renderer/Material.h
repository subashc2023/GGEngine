#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Renderer/Pipeline.h"
#include "GGEngine/Renderer/Camera.h"

#include <vulkan/vulkan.h>
#include <string>
#include <unordered_map>
#include <array>
#include <cstring>

namespace GGEngine {

    class Shader;

    // Property types supported by materials
    enum class PropertyType : uint8_t
    {
        Float,
        Vec2,
        Vec3,
        Vec4,
        Mat4
    };

    // Returns size in bytes for each property type
    inline uint32_t GetPropertyTypeSize(PropertyType type)
    {
        switch (type)
        {
            case PropertyType::Float: return sizeof(float);
            case PropertyType::Vec2:  return sizeof(float) * 2;
            case PropertyType::Vec3:  return sizeof(float) * 3;
            case PropertyType::Vec4:  return sizeof(float) * 4;
            case PropertyType::Mat4:  return sizeof(float) * 16;
        }
        return 0;
    }

    // Metadata for a registered material property
    struct GG_API PropertyMetadata
    {
        PropertyType type;
        uint32_t offset;            // Byte offset in push constant buffer
        uint32_t size;              // Size in bytes
        VkShaderStageFlags stage;   // Shader stage(s) this property is used in
    };

    // Specification for creating a material
    struct GG_API MaterialSpecification
    {
        // Required
        Shader* shader = nullptr;
        VkRenderPass renderPass = VK_NULL_HANDLE;

        // Optional vertex layout (nullptr = shader-defined vertices)
        const VertexLayout* vertexLayout = nullptr;

        // Pipeline configuration (sensible defaults)
        VkPrimitiveTopology topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
        VkCullModeFlags cullMode = VK_CULL_MODE_NONE;
        VkFrontFace frontFace = VK_FRONT_FACE_CLOCKWISE;
        BlendMode blendMode = BlendMode::None;
        bool depthTestEnable = false;
        bool depthWriteEnable = false;

        // Descriptor set layouts for global bindings (e.g., camera at set 0)
        // Material will add its own layouts after these
        std::vector<VkDescriptorSetLayout> descriptorSetLayouts;

        // Debug name
        std::string name;
    };

    class GG_API Material
    {
    public:
        Material() = default;
        ~Material();

        // Property registration (call before Create)
        void RegisterProperty(const std::string& name, PropertyType type,
                              VkShaderStageFlags stage, uint32_t offset);

        // Create pipeline with registered properties
        bool Create(const MaterialSpecification& spec);

        // Type-safe property setters
        void SetFloat(const std::string& name, float value);
        void SetVec2(const std::string& name, float x, float y);
        void SetVec3(const std::string& name, float x, float y, float z);
        void SetVec4(const std::string& name, float x, float y, float z, float w);
        void SetMat4(const std::string& name, const Mat4& matrix);

        // Overloads accepting arrays/pointers
        void SetVec2(const std::string& name, const float* values);
        void SetVec3(const std::string& name, const float* values);
        void SetVec4(const std::string& name, const float* values);

        // Check if property exists
        bool HasProperty(const std::string& name) const;
        const PropertyMetadata* GetPropertyMetadata(const std::string& name) const;

        // Bind pipeline and push all constants
        void Bind(VkCommandBuffer cmd) const;

        // Access underlying pipeline (for advanced use)
        Pipeline* GetPipeline() const { return m_Pipeline.get(); }
        VkPipelineLayout GetPipelineLayout() const;

        // Accessors
        const std::string& GetName() const { return m_Name; }
        Shader* GetShader() const { return m_Shader; }

    private:
        // Build push constant ranges from registered properties
        std::vector<PushConstantRange> BuildPushConstantRanges() const;

        // Validate property type matches registration
        bool ValidateProperty(const std::string& name, PropertyType expectedType) const;

        // Write data to push constant buffer
        void WriteProperty(uint32_t offset, const void* data, uint32_t size);

        std::string m_Name;
        Shader* m_Shader = nullptr;

        // Property storage
        std::unordered_map<std::string, PropertyMetadata> m_Properties;
        std::array<uint8_t, 128> m_PushConstantBuffer{};  // Max push constant size

        // Vulkan resources
        Scope<Pipeline> m_Pipeline;
    };

}
