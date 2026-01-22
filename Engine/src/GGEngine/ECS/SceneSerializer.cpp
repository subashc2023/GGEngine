#include "ggpch.h"
#include "SceneSerializer.h"
#include "Components.h"

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

            // Tag component (required)
            const auto* tag = m_Scene->GetComponent<TagComponent>(entityId);
            if (tag)
            {
                entityJson["GUID"] = tag->ID.ToString();
                entityJson["TagComponent"]["Name"] = tag->Name;
            }

            // Transform component
            if (m_Scene->HasComponent<TransformComponent>(entityId))
            {
                const auto* transform = m_Scene->GetComponent<TransformComponent>(entityId);
                entityJson["TransformComponent"]["Position"] = {
                    transform->Position[0],
                    transform->Position[1],
                    transform->Position[2]
                };
                entityJson["TransformComponent"]["Rotation"] = transform->Rotation;
                entityJson["TransformComponent"]["Scale"] = {
                    transform->Scale[0],
                    transform->Scale[1]
                };
            }

            // Sprite renderer component
            if (m_Scene->HasComponent<SpriteRendererComponent>(entityId))
            {
                const auto* sprite = m_Scene->GetComponent<SpriteRendererComponent>(entityId);
                entityJson["SpriteRendererComponent"]["Color"] = {
                    sprite->Color[0],
                    sprite->Color[1],
                    sprite->Color[2],
                    sprite->Color[3]
                };
                entityJson["SpriteRendererComponent"]["TextureName"] = sprite->TextureName;
                entityJson["SpriteRendererComponent"]["TilingFactor"] = sprite->TilingFactor;
            }

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

                // Create entity with preserved GUID
                EntityID entity = m_Scene->CreateEntityWithGUID(name, guid);

                // Read transform component
                if (entityJson.contains("TransformComponent"))
                {
                    auto* transform = m_Scene->GetComponent<TransformComponent>(entity);
                    if (transform)
                    {
                        const auto& t = entityJson["TransformComponent"];
                        if (t.contains("Position"))
                        {
                            transform->Position[0] = t["Position"][0].get<float>();
                            transform->Position[1] = t["Position"][1].get<float>();
                            transform->Position[2] = t["Position"][2].get<float>();
                        }
                        if (t.contains("Rotation"))
                        {
                            transform->Rotation = t["Rotation"].get<float>();
                        }
                        if (t.contains("Scale"))
                        {
                            transform->Scale[0] = t["Scale"][0].get<float>();
                            transform->Scale[1] = t["Scale"][1].get<float>();
                        }
                    }
                }

                // Read sprite renderer component
                if (entityJson.contains("SpriteRendererComponent"))
                {
                    auto& sprite = m_Scene->AddComponent<SpriteRendererComponent>(entity);
                    const auto& s = entityJson["SpriteRendererComponent"];
                    if (s.contains("Color"))
                    {
                        sprite.Color[0] = s["Color"][0].get<float>();
                        sprite.Color[1] = s["Color"][1].get<float>();
                        sprite.Color[2] = s["Color"][2].get<float>();
                        sprite.Color[3] = s["Color"][3].get<float>();
                    }
                    if (s.contains("TextureName"))
                    {
                        sprite.TextureName = s["TextureName"].get<std::string>();
                    }
                    if (s.contains("TilingFactor"))
                    {
                        sprite.TilingFactor = s["TilingFactor"].get<float>();
                    }
                }
            }
        }

        GG_CORE_INFO("Scene deserialized from: {}", filepath);
        return true;
    }

}
