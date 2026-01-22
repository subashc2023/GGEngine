#pragma once

#include "GGEngine/Renderer/SceneCamera.h"

namespace GGEngine {

    struct CameraComponent
    {
        SceneCamera Camera;
        bool Primary = true;           // Is this the primary camera for the scene?
        bool FixedAspectRatio = false; // Should aspect ratio stay fixed on resize?

        CameraComponent() = default;
        CameraComponent(const CameraComponent&) = default;
    };

}
