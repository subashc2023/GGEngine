#include "ggpch.h"
#include "MaterialLibrary.h"
#include "Material.h"

namespace GGEngine {

    MaterialLibrary& MaterialLibrary::Get()
    {
        static MaterialLibrary instance;
        return instance;
    }

    void MaterialLibrary::Init()
    {
        GG_CORE_TRACE("MaterialLibrary initialized");
    }

    void MaterialLibrary::Shutdown()
    {
        Clear();
        GG_CORE_TRACE("MaterialLibrary shutdown");
    }

    Material* MaterialLibrary::Create(const std::string& name, const MaterialSpecification& spec)
    {
        // Check if already exists
        auto it = m_Materials.find(name);
        if (it != m_Materials.end())
        {
            GG_CORE_WARN("Material '{}' already exists, returning existing", name);
            return it->second.get();
        }

        // Create new material
        auto material = CreateScope<Material>();

        // Note: Properties must be registered by caller before calling Create
        // MaterialLibrary just provides storage and lookup

        m_Materials[name] = std::move(material);
        GG_CORE_INFO("Material '{}' registered in library", name);

        return m_Materials[name].get();
    }

    Material* MaterialLibrary::Get(const std::string& name) const
    {
        auto it = m_Materials.find(name);
        if (it != m_Materials.end())
        {
            return it->second.get();
        }
        return nullptr;
    }

    bool MaterialLibrary::Exists(const std::string& name) const
    {
        return m_Materials.find(name) != m_Materials.end();
    }

    void MaterialLibrary::Remove(const std::string& name)
    {
        auto it = m_Materials.find(name);
        if (it != m_Materials.end())
        {
            m_Materials.erase(it);
            GG_CORE_TRACE("Material '{}' removed from library", name);
        }
    }

    void MaterialLibrary::Clear()
    {
        m_Materials.clear();
        GG_CORE_TRACE("MaterialLibrary cleared all materials");
    }

}
