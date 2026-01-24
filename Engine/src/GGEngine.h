#pragma once

//for use by GGEngine Applications


#include "GGEngine/Core/Application.h"
#include "GGEngine/Core/Layer.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/MouseButtonCodes.h"
#include "GGEngine/Core/Profiler.h"
#include "GGEngine/Core/JobSystem.h"

#include "GGEngine/Asset/AssetManager.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/Asset/TextureLibrary.h"

#include "GGEngine/ECS/Entity.h"
#include "GGEngine/ECS/GUID.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Components.h"
#include "GGEngine/ECS/SceneSerializer.h"

#include "GGEngine/Utils/FileDialogs.h"

#include "GGEngine/ParticleSystem/Random.h"
#include "GGEngine/ParticleSystem/ParticleSystem.h"

#include "GGEngine/Renderer/Pipeline.h"
#include "GGEngine/Renderer/Material.h"
#include "GGEngine/Renderer/MaterialLibrary.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Renderer/SubTexture2D.h"
#include "GGEngine/Renderer/TextureAtlas.h"

#include "GGEngine/ImGui/ImGuiLayer.h"

// NOTE: EntryPoint.h is NOT included here to avoid "main already defined" linker errors.
// Client applications must include it manually after GGEngine.h:
//   #include "GGEngine.h"
//   #include "GGEngine/Core/EntryPoint.h"