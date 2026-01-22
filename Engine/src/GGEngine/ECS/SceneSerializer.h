#pragma once

#include "Scene.h"
#include "GGEngine/Core/Core.h"
#include <string>

namespace GGEngine {

    class GG_API SceneSerializer
    {
    public:
        SceneSerializer(Scene* scene);

        void Serialize(const std::string& filepath);
        bool Deserialize(const std::string& filepath);

    private:
        Scene* m_Scene;
    };

}
