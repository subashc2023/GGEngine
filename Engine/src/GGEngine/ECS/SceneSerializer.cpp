#include "ggpch.h"
#include "SceneSerializer.h"
#include "ComponentTraits.h"
#include "GGEngine/Renderer/SceneCamera.h"

#include <json.hpp>
#include <fstream>

using json = nlohmann::json;

namespace GGEngine {

    SceneSerializer::SceneSerializer(Scene* scene)
        : m_Scene(scene)
    {
    }

    void SceneSerializer::Serialize(const std::string& filepath)
    {
        json root;
        root["Scene"] = m_Scene->GetName();

        json entitiesArray = json::array();

        for (Entity index : m_Scene->GetAllEntities())
        {
            EntityID entityId = m_Scene->GetEntityID(index);

            json entityJson;

            // Tag component (special handling - GUID is at entity level)
            const auto* tag = m_Scene->GetComponent<TagComponent>(entityId);
            if (tag)
            {
                entityJson["GUID"] = tag->ID.ToString();
                json tagJson;
                ComponentSerializer<TagComponent>::ToJson(*tag, tagJson);
                entityJson["TagComponent"] = tagJson;
            }

            // TransformComponent is auto-added with entity, so use GetComponent instead of HasComponent
            if (const auto* transform = m_Scene->GetComponent<TransformComponent>(entityId))
            {
                json compJson;
                ComponentSerializer<TransformComponent>::ToJson(*transform, compJson);
                entityJson["TransformComponent"] = compJson;
            }

            // Optional components using trait-based serialization
            SerializeComponentIfPresent<SpriteRendererComponent>(m_Scene, entityId, entityJson);
            SerializeComponentIfPresent<TilemapComponent>(m_Scene, entityId, entityJson);
            SerializeComponentIfPresent<CameraComponent>(m_Scene, entityId, entityJson);

            entitiesArray.push_back(entityJson);
        }

        root["Entities"] = entitiesArray;

        std::ofstream file(filepath);
        if (file.is_open())
        {
            file << root.dump(2);  // Pretty print with 2-space indent
            file.close();
            GG_CORE_INFO("Scene serialized to: {}", filepath);
        }
        else
        {
            GG_CORE_ERROR("Failed to open file for writing: {}", filepath);
        }
    }

    bool SceneSerializer::Deserialize(const std::string& filepath)
    {
        std::ifstream file(filepath);
        if (!file.is_open())
        {
            GG_CORE_ERROR("Failed to open scene file: {}", filepath);
            return false;
        }

        json root;
        try
        {
            file >> root;
        }
        catch (const json::parse_error& e)
        {
            GG_CORE_ERROR("Failed to parse scene file: {}", e.what());
            return false;
        }

        // Clear existing scene
        m_Scene->Clear();

        // Set scene name
        if (root.contains("Scene"))
        {
            m_Scene->SetName(root["Scene"].get<std::string>());
        }

        // Load entities
        if (root.contains("Entities"))
        {
            for (const auto& entityJson : root["Entities"])
            {
                std::string name = "Entity";
                GUID guid;

                // Read tag component
                if (entityJson.contains("TagComponent"))
                {
                    name = entityJson["TagComponent"]["Name"].get<std::string>();
                }
                if (entityJson.contains("GUID"))
                {
                    guid = GUID::FromString(entityJson["GUID"].get<std::string>());
                }

                // Generate new GUID if file didn't have one (prevents collisions)
                if (!guid.IsValid())
                {
                    guid = GUID::Generate();
                    GG_CORE_WARN("Entity '{}' missing GUID in scene file, generated new one", name);
                }

                // Create entity with preserved GUID
                EntityID entity = m_Scene->CreateEntityWithGUID(name, guid);

                // TransformComponent - entity already has one, just update it
                if (entityJson.contains("TransformComponent"))
                {
                    auto* transform = m_Scene->GetComponent<TransformComponent>(entity);
                    if (transform)
                    {
                        ComponentSerializer<TransformComponent>::FromJson(*transform, entityJson["TransformComponent"]);
                    }
                }

                // Optional components using trait-based deserialization
                DeserializeComponentIfPresent<SpriteRendererComponent>(m_Scene, entity, entityJson);
                DeserializeComponentIfPresent<TilemapComponent>(m_Scene, entity, entityJson);
                DeserializeComponentIfPresent<CameraComponent>(m_Scene, entity, entityJson);
            }
        }

        GG_CORE_INFO("Scene deserialized from: {}", filepath);
        return true;
    }

}
