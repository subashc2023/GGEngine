#pragma once

#include "Asset.h"
#include "AssetManager.h"
#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Result.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"

#include <string>
#include <vector>

namespace GGEngine {

    // Template specialization for Shader type
    class Shader;
    template<>
    constexpr AssetType GetAssetType<Shader>() { return AssetType::Shader; }

    // Represents a compiled SPIR-V shader stage module (backend-agnostic)
    struct GG_API ShaderStageInfo
    {
        ShaderStage stage = ShaderStage::None;
        RHIShaderModuleHandle handle;
        std::string entryPoint = "main";
    };

    // A shader asset that can contain multiple stages (vert, frag, etc.)
    class GG_API Shader : public Asset
    {
    public:
        Shader() = default;
        ~Shader();

        // Factory methods - create shader via asset system
        static AssetHandle<Shader> Create(const std::string& path);
        static AssetHandle<Shader> Create(const std::string& name, const std::string& path);

        AssetType GetType() const override { return AssetType::Shader; }

        // Load from SPIR-V files (e.g., "assets/shaders/compiled/triangle" loads triangle.vert.spv and triangle.frag.spv)
        Result<void> Load(const std::string& basePath);

        // Load individual stages
        Result<void> LoadStage(ShaderStage stage, const std::vector<char>& spirvCode);

        // Load stage from file - returns false if file doesn't exist (for optional stages)
        bool LoadStageFromFile(ShaderStage stage, const std::string& path);

        void Unload() override;

        // Get source path (base path) for hot reload
        const std::string& GetSourcePath() const { return m_SourcePath; }

#ifndef GG_DIST
        // Reload shader from disk
        // Available in Debug and Release builds, excluded from Dist
        Result<void> Reload();
#endif

        const std::vector<ShaderStageInfo>& GetStages() const { return m_Stages; }
        bool HasStage(ShaderStage stage) const;
        RHIShaderModuleHandle GetStageHandle(ShaderStage stage) const;

        // Name accessors
        const std::string& GetName() const { return m_Name; }
        void SetName(const std::string& name) { m_Name = name; }

    private:
        std::vector<ShaderStageInfo> m_Stages;
        std::string m_Name;
        std::string m_SourcePath;  // Base path for hot reload
    };

}
