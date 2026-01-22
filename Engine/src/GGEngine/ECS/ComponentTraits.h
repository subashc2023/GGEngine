#pragma once

#include "Scene.h"
#include "Components.h"
#include "GGEngine/Renderer/SceneCamera.h"
#include <json.hpp>

namespace GGEngine {

    // Component serialization traits
    // Each component type specializes this template to provide:
    // - Name(): Human-readable component name for JSON
    // - ToJson(): Serialize component data to JSON
    // - FromJson(): Deserialize component data from JSON

    template<typename T>
    struct ComponentSerializer
    {
        // Default implementation - components without specialization won't serialize
        static constexpr const char* Name() { return "Unknown"; }
        static void ToJson(const T&, nlohmann::json&) {}
        static void FromJson(T&, const nlohmann::json&) {}
    };

    // TagComponent serialization
    // Note: TagComponent is handled specially since GUID is serialized at entity level
    template<>
    struct ComponentSerializer<TagComponent>
    {
        static constexpr const char* Name() { return "TagComponent"; }

        static void ToJson(const TagComponent& comp, nlohmann::json& j)
        {
            j["Name"] = comp.Name;
        }

        static void FromJson(TagComponent& comp, const nlohmann::json& j)
        {
            if (j.contains("Name"))
                comp.Name = j["Name"].get<std::string>();
        }
    };

    // TransformComponent serialization
    template<>
    struct ComponentSerializer<TransformComponent>
    {
        static constexpr const char* Name() { return "TransformComponent"; }

        static void ToJson(const TransformComponent& comp, nlohmann::json& j)
        {
            j["Position"] = { comp.Position[0], comp.Position[1], comp.Position[2] };
            j["Rotation"] = comp.Rotation;
            j["Scale"] = { comp.Scale[0], comp.Scale[1] };
        }

        static void FromJson(TransformComponent& comp, const nlohmann::json& j)
        {
            if (j.contains("Position"))
            {
                comp.Position[0] = j["Position"][0].get<float>();
                comp.Position[1] = j["Position"][1].get<float>();
                comp.Position[2] = j["Position"][2].get<float>();
            }
            if (j.contains("Rotation"))
                comp.Rotation = j["Rotation"].get<float>();
            if (j.contains("Scale"))
            {
                comp.Scale[0] = j["Scale"][0].get<float>();
                comp.Scale[1] = j["Scale"][1].get<float>();
            }
        }
    };

    // SpriteRendererComponent serialization
    template<>
    struct ComponentSerializer<SpriteRendererComponent>
    {
        static constexpr const char* Name() { return "SpriteRendererComponent"; }

        static void ToJson(const SpriteRendererComponent& comp, nlohmann::json& j)
        {
            j["Color"] = { comp.Color[0], comp.Color[1], comp.Color[2], comp.Color[3] };
            j["TextureName"] = comp.TextureName;
            j["TilingFactor"] = comp.TilingFactor;
            j["UseAtlas"] = comp.UseAtlas;
            j["AtlasCellX"] = comp.AtlasCellX;
            j["AtlasCellY"] = comp.AtlasCellY;
            j["AtlasCellWidth"] = comp.AtlasCellWidth;
            j["AtlasCellHeight"] = comp.AtlasCellHeight;
            j["AtlasSpriteWidth"] = comp.AtlasSpriteWidth;
            j["AtlasSpriteHeight"] = comp.AtlasSpriteHeight;
        }

        static void FromJson(SpriteRendererComponent& comp, const nlohmann::json& j)
        {
            if (j.contains("Color"))
            {
                comp.Color[0] = j["Color"][0].get<float>();
                comp.Color[1] = j["Color"][1].get<float>();
                comp.Color[2] = j["Color"][2].get<float>();
                comp.Color[3] = j["Color"][3].get<float>();
            }
            if (j.contains("TextureName"))
                comp.TextureName = j["TextureName"].get<std::string>();
            if (j.contains("TilingFactor"))
                comp.TilingFactor = j["TilingFactor"].get<float>();
            if (j.contains("UseAtlas"))
                comp.UseAtlas = j["UseAtlas"].get<bool>();
            if (j.contains("AtlasCellX"))
                comp.AtlasCellX = j["AtlasCellX"].get<uint32_t>();
            if (j.contains("AtlasCellY"))
                comp.AtlasCellY = j["AtlasCellY"].get<uint32_t>();
            if (j.contains("AtlasCellWidth"))
                comp.AtlasCellWidth = j["AtlasCellWidth"].get<float>();
            if (j.contains("AtlasCellHeight"))
                comp.AtlasCellHeight = j["AtlasCellHeight"].get<float>();
            if (j.contains("AtlasSpriteWidth"))
                comp.AtlasSpriteWidth = j["AtlasSpriteWidth"].get<float>();
            if (j.contains("AtlasSpriteHeight"))
                comp.AtlasSpriteHeight = j["AtlasSpriteHeight"].get<float>();
        }
    };

    // TilemapComponent serialization
    template<>
    struct ComponentSerializer<TilemapComponent>
    {
        static constexpr const char* Name() { return "TilemapComponent"; }

        static void ToJson(const TilemapComponent& comp, nlohmann::json& j)
        {
            j["Width"] = comp.Width;
            j["Height"] = comp.Height;
            j["TileWidth"] = comp.TileWidth;
            j["TileHeight"] = comp.TileHeight;
            j["TextureName"] = comp.TextureName;
            j["AtlasCellWidth"] = comp.AtlasCellWidth;
            j["AtlasCellHeight"] = comp.AtlasCellHeight;
            j["AtlasColumns"] = comp.AtlasColumns;
            j["ZOffset"] = comp.ZOffset;
            j["Color"] = { comp.Color[0], comp.Color[1], comp.Color[2], comp.Color[3] };
            j["Tiles"] = comp.Tiles;  // nlohmann::json handles vector<int32_t> directly
        }

        static void FromJson(TilemapComponent& comp, const nlohmann::json& j)
        {
            if (j.contains("Width"))
                comp.Width = j["Width"].get<uint32_t>();
            if (j.contains("Height"))
                comp.Height = j["Height"].get<uint32_t>();
            if (j.contains("TileWidth"))
                comp.TileWidth = j["TileWidth"].get<float>();
            if (j.contains("TileHeight"))
                comp.TileHeight = j["TileHeight"].get<float>();
            if (j.contains("TextureName"))
                comp.TextureName = j["TextureName"].get<std::string>();
            if (j.contains("AtlasCellWidth"))
                comp.AtlasCellWidth = j["AtlasCellWidth"].get<float>();
            if (j.contains("AtlasCellHeight"))
                comp.AtlasCellHeight = j["AtlasCellHeight"].get<float>();
            if (j.contains("AtlasColumns"))
                comp.AtlasColumns = j["AtlasColumns"].get<uint32_t>();
            if (j.contains("ZOffset"))
                comp.ZOffset = j["ZOffset"].get<float>();
            if (j.contains("Color"))
            {
                comp.Color[0] = j["Color"][0].get<float>();
                comp.Color[1] = j["Color"][1].get<float>();
                comp.Color[2] = j["Color"][2].get<float>();
                comp.Color[3] = j["Color"][3].get<float>();
            }
            if (j.contains("Tiles"))
                comp.Tiles = j["Tiles"].get<std::vector<int32_t>>();

            // Ensure tiles vector matches dimensions
            comp.ResizeTiles();
        }
    };

    // CameraComponent serialization
    template<>
    struct ComponentSerializer<CameraComponent>
    {
        static constexpr const char* Name() { return "CameraComponent"; }

        static void ToJson(const CameraComponent& comp, nlohmann::json& j)
        {
            j["Primary"] = comp.Primary;
            j["FixedAspectRatio"] = comp.FixedAspectRatio;
            j["ProjectionType"] = static_cast<int>(comp.Camera.GetProjectionType());

            // Perspective params
            j["PerspectiveFOV"] = comp.Camera.GetPerspectiveFOV();
            j["PerspectiveNear"] = comp.Camera.GetPerspectiveNearClip();
            j["PerspectiveFar"] = comp.Camera.GetPerspectiveFarClip();

            // Orthographic params
            j["OrthographicSize"] = comp.Camera.GetOrthographicSize();
            j["OrthographicNear"] = comp.Camera.GetOrthographicNearClip();
            j["OrthographicFar"] = comp.Camera.GetOrthographicFarClip();
        }

        static void FromJson(CameraComponent& comp, const nlohmann::json& j)
        {
            if (j.contains("Primary"))
                comp.Primary = j["Primary"].get<bool>();
            if (j.contains("FixedAspectRatio"))
                comp.FixedAspectRatio = j["FixedAspectRatio"].get<bool>();
            if (j.contains("ProjectionType"))
                comp.Camera.SetProjectionType(
                    static_cast<SceneCamera::ProjectionType>(j["ProjectionType"].get<int>()));

            // Perspective params
            if (j.contains("PerspectiveFOV"))
                comp.Camera.SetPerspectiveFOV(j["PerspectiveFOV"].get<float>());
            if (j.contains("PerspectiveNear"))
                comp.Camera.SetPerspectiveNearClip(j["PerspectiveNear"].get<float>());
            if (j.contains("PerspectiveFar"))
                comp.Camera.SetPerspectiveFarClip(j["PerspectiveFar"].get<float>());

            // Orthographic params
            if (j.contains("OrthographicSize"))
                comp.Camera.SetOrthographicSize(j["OrthographicSize"].get<float>());
            if (j.contains("OrthographicNear"))
                comp.Camera.SetOrthographicNearClip(j["OrthographicNear"].get<float>());
            if (j.contains("OrthographicFar"))
                comp.Camera.SetOrthographicFarClip(j["OrthographicFar"].get<float>());
        }
    };

    // Helper to serialize a component if the entity has it
    template<typename T>
    void SerializeComponentIfPresent(const Scene* scene, EntityID entity, nlohmann::json& entityJson)
    {
        if (scene->HasComponent<T>(entity))
        {
            const T* comp = scene->GetComponent<T>(entity);
            nlohmann::json componentJson;
            ComponentSerializer<T>::ToJson(*comp, componentJson);
            entityJson[ComponentSerializer<T>::Name()] = componentJson;
        }
    }

    // Helper to deserialize a component if present in JSON
    template<typename T>
    void DeserializeComponentIfPresent(Scene* scene, EntityID entity, const nlohmann::json& entityJson)
    {
        const char* name = ComponentSerializer<T>::Name();
        if (entityJson.contains(name))
        {
            T& comp = scene->AddComponent<T>(entity);
            ComponentSerializer<T>::FromJson(comp, entityJson[name]);
        }
    }

} // namespace GGEngine
