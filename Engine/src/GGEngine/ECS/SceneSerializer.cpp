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

                // Atlas/spritesheet settings
                entityJson["SpriteRendererComponent"]["UseAtlas"] = sprite->UseAtlas;
                entityJson["SpriteRendererComponent"]["AtlasCellX"] = sprite->AtlasCellX;
                entityJson["SpriteRendererComponent"]["AtlasCellY"] = sprite->AtlasCellY;
                entityJson["SpriteRendererComponent"]["AtlasCellWidth"] = sprite->AtlasCellWidth;
                entityJson["SpriteRendererComponent"]["AtlasCellHeight"] = sprite->AtlasCellHeight;
                entityJson["SpriteRendererComponent"]["AtlasSpriteWidth"] = sprite->AtlasSpriteWidth;
                entityJson["SpriteRendererComponent"]["AtlasSpriteHeight"] = sprite->AtlasSpriteHeight;
            }

            // Tilemap component
            if (m_Scene->HasComponent<TilemapComponent>(entityId))
            {
                const auto* tilemap = m_Scene->GetComponent<TilemapComponent>(entityId);
                auto& tm = entityJson["TilemapComponent"];

                tm["Width"] = tilemap->Width;
                tm["Height"] = tilemap->Height;
                tm["TileWidth"] = tilemap->TileWidth;
                tm["TileHeight"] = tilemap->TileHeight;
                tm["TextureName"] = tilemap->TextureName;
                tm["AtlasCellWidth"] = tilemap->AtlasCellWidth;
                tm["AtlasCellHeight"] = tilemap->AtlasCellHeight;
                tm["AtlasColumns"] = tilemap->AtlasColumns;
                tm["ZOffset"] = tilemap->ZOffset;
                tm["Color"] = {
                    tilemap->Color[0],
                    tilemap->Color[1],
                    tilemap->Color[2],
                    tilemap->Color[3]
                };
                tm["Tiles"] = tilemap->Tiles;  // nlohmann::json handles vector<int32_t> directly
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

                // Generate new GUID if file didn't have one (prevents collisions)
                if (!guid.IsValid())
                {
                    guid = GUID::Generate();
                    GG_CORE_WARN("Entity '{}' missing GUID in scene file, generated new one", name);
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

                    // Atlas/spritesheet settings
                    if (s.contains("UseAtlas"))
                        sprite.UseAtlas = s["UseAtlas"].get<bool>();
                    if (s.contains("AtlasCellX"))
                        sprite.AtlasCellX = s["AtlasCellX"].get<uint32_t>();
                    if (s.contains("AtlasCellY"))
                        sprite.AtlasCellY = s["AtlasCellY"].get<uint32_t>();
                    if (s.contains("AtlasCellWidth"))
                        sprite.AtlasCellWidth = s["AtlasCellWidth"].get<float>();
                    if (s.contains("AtlasCellHeight"))
                        sprite.AtlasCellHeight = s["AtlasCellHeight"].get<float>();
                    if (s.contains("AtlasSpriteWidth"))
                        sprite.AtlasSpriteWidth = s["AtlasSpriteWidth"].get<float>();
                    if (s.contains("AtlasSpriteHeight"))
                        sprite.AtlasSpriteHeight = s["AtlasSpriteHeight"].get<float>();
                }

                // Read tilemap component
                if (entityJson.contains("TilemapComponent"))
                {
                    auto& tilemap = m_Scene->AddComponent<TilemapComponent>(entity);
                    const auto& tm = entityJson["TilemapComponent"];

                    if (tm.contains("Width"))
                        tilemap.Width = tm["Width"].get<uint32_t>();
                    if (tm.contains("Height"))
                        tilemap.Height = tm["Height"].get<uint32_t>();
                    if (tm.contains("TileWidth"))
                        tilemap.TileWidth = tm["TileWidth"].get<float>();
                    if (tm.contains("TileHeight"))
                        tilemap.TileHeight = tm["TileHeight"].get<float>();
                    if (tm.contains("TextureName"))
                        tilemap.TextureName = tm["TextureName"].get<std::string>();
                    if (tm.contains("AtlasCellWidth"))
                        tilemap.AtlasCellWidth = tm["AtlasCellWidth"].get<float>();
                    if (tm.contains("AtlasCellHeight"))
                        tilemap.AtlasCellHeight = tm["AtlasCellHeight"].get<float>();
                    if (tm.contains("AtlasColumns"))
                        tilemap.AtlasColumns = tm["AtlasColumns"].get<uint32_t>();
                    if (tm.contains("ZOffset"))
                        tilemap.ZOffset = tm["ZOffset"].get<float>();
                    if (tm.contains("Color"))
                    {
                        tilemap.Color[0] = tm["Color"][0].get<float>();
                        tilemap.Color[1] = tm["Color"][1].get<float>();
                        tilemap.Color[2] = tm["Color"][2].get<float>();
                        tilemap.Color[3] = tm["Color"][3].get<float>();
                    }
                    if (tm.contains("Tiles"))
                    {
                        tilemap.Tiles = tm["Tiles"].get<std::vector<int32_t>>();
                    }

                    // Ensure tiles vector matches dimensions
                    tilemap.ResizeTiles();
                }
            }
        }

        GG_CORE_INFO("Scene deserialized from: {}", filepath);
        return true;
    }

}
