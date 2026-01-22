#pragma once

#include "GGEngine/Core/Core.h"
#include "Material.h"

#include <string>
#include <unordered_map>

namespace GGEngine {

    struct MaterialSpecification;

    // Registry for named materials
    // Owned by Application - access via Application::Get().GetMaterialLibrary()
    class GG_API MaterialLibrary
    {
    public:
        MaterialLibrary() = default;
        ~MaterialLibrary() = default;

        // Non-copyable
        MaterialLibrary(const MaterialLibrary&) = delete;
        MaterialLibrary& operator=(const MaterialLibrary&) = delete;

        void Init();
        void Shutdown();

        // Create and register a material with a name
        // Returns pointer for continued access (owned by library)
        Material* Create(const std::string& name, const MaterialSpecification& spec);

        // Get material by name (returns nullptr if not found)
        Material* Get(const std::string& name) const;

        // Check if material exists
        bool Exists(const std::string& name) const;

        // Remove a material by name
        void Remove(const std::string& name);

        // Remove all materials
        void Clear();

    private:
        std::unordered_map<std::string, Scope<Material>> m_Materials;
    };

}
