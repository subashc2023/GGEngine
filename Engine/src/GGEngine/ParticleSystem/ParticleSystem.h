#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Timestep.h"
#include "GGEngine/Renderer/Camera.h"

#include <vector>
#include <cstdint>

namespace GGEngine {

    struct GG_API ParticleProps
    {
        float Position[2] = { 0.0f, 0.0f };
        float Velocity[2] = { 0.0f, 0.0f };
        float VelocityVariation[2] = { 0.0f, 0.0f };
        float ColorBegin[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
        float ColorEnd[4] = { 1.0f, 1.0f, 1.0f, 0.0f };
        float SizeBegin = 1.0f;
        float SizeEnd = 0.0f;
        float SizeVariation = 0.0f;
        float LifeTime = 1.0f;
    };

    class GG_API ParticleSystem
    {
    public:
        ParticleSystem(uint32_t maxParticles = 10000);

        void Emit(const ParticleProps& props);
        void OnUpdate(Timestep ts);
        void OnRender(const Camera& camera);

    private:
        struct Particle
        {
            float Position[2] = { 0.0f, 0.0f };
            float Velocity[2] = { 0.0f, 0.0f };
            float ColorBegin[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
            float ColorEnd[4] = { 1.0f, 1.0f, 1.0f, 0.0f };
            float Rotation = 0.0f;
            float SizeBegin = 1.0f;
            float SizeEnd = 0.0f;

            float LifeTime = 1.0f;
            float LifeRemaining = 0.0f;

            bool Active = false;
        };

        std::vector<Particle> m_ParticlePool;
        uint32_t m_PoolIndex = 0;
    };

}
