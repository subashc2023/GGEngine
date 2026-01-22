#pragma once

#include "GGEngine/ECS/GUID.h"
#include <string>

namespace GGEngine {

    // Human-readable name and persistent GUID
    struct TagComponent
    {
        std::string Name = "Entity";
        GUID ID;

        TagComponent() : ID(GUID::Generate()) {}
        TagComponent(const std::string& name) : Name(name), ID(GUID::Generate()) {}
    };

}
