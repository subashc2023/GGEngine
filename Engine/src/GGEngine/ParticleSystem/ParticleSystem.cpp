#include "ggpch.h"
#include "ParticleSystem.h"
#include "Random.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Core/Math.h"

#include <cmath>

namespace GGEngine {

    ParticleSystem::ParticleSystem(uint32_t maxParticles)
    {
        m_ParticlePool.resize(maxParticles);
    }

    void ParticleSystem::Emit(const ParticleProps& props)
    {
        Particle& particle = m_ParticlePool[m_PoolIndex];
        particle.Active = true;
        particle.Position[0] = props.Position[0];
        particle.Position[1] = props.Position[1];
        particle.Rotation = Random::Float() * Math::TwoPi;

        // Velocity with variation
        particle.Velocity[0] = props.Velocity[0] + props.VelocityVariation[0] * (Random::Float() - 0.5f);
        particle.Velocity[1] = props.Velocity[1] + props.VelocityVariation[1] * (Random::Float() - 0.5f);

        // Color
        particle.ColorBegin[0] = props.ColorBegin[0];
        particle.ColorBegin[1] = props.ColorBegin[1];
        particle.ColorBegin[2] = props.ColorBegin[2];
        particle.ColorBegin[3] = props.ColorBegin[3];
        particle.ColorEnd[0] = props.ColorEnd[0];
        particle.ColorEnd[1] = props.ColorEnd[1];
        particle.ColorEnd[2] = props.ColorEnd[2];
        particle.ColorEnd[3] = props.ColorEnd[3];

        // Size with variation
        particle.SizeBegin = props.SizeBegin + props.SizeVariation * (Random::Float() - 0.5f);
        particle.SizeEnd = props.SizeEnd;

        // Life
        particle.LifeTime = props.LifeTime;
        particle.LifeRemaining = props.LifeTime;

        m_PoolIndex = (m_PoolIndex + 1) % m_ParticlePool.size();
    }

    void ParticleSystem::OnUpdate(Timestep ts)
    {
        for (auto& particle : m_ParticlePool)
        {
            if (!particle.Active)
                continue;

            if (particle.LifeRemaining <= 0.0f)
            {
                particle.Active = false;
                continue;
            }

            particle.LifeRemaining -= ts;
            particle.Position[0] += particle.Velocity[0] * ts;
            particle.Position[1] += particle.Velocity[1] * ts;
            particle.Rotation += 0.01f * ts;
        }
    }

    void ParticleSystem::OnRender(const Camera& camera)
    {
        Renderer2D::BeginScene(camera);

        for (auto& particle : m_ParticlePool)
        {
            if (!particle.Active)
                continue;

            // Calculate life progress (1.0 = just born, 0.0 = dead)
            float life = particle.LifeRemaining / particle.LifeTime;

            // Interpolate color
            float r = particle.ColorEnd[0] + (particle.ColorBegin[0] - particle.ColorEnd[0]) * life;
            float g = particle.ColorEnd[1] + (particle.ColorBegin[1] - particle.ColorEnd[1]) * life;
            float b = particle.ColorEnd[2] + (particle.ColorBegin[2] - particle.ColorEnd[2]) * life;
            float a = particle.ColorEnd[3] + (particle.ColorBegin[3] - particle.ColorEnd[3]) * life;

            // Interpolate size
            float size = particle.SizeEnd + (particle.SizeBegin - particle.SizeEnd) * life;

            // Draw particle using QuadSpec
            QuadSpec spec;
            spec.x = particle.Position[0];
            spec.y = particle.Position[1];
            spec.z = 0.0f;
            spec.width = size;
            spec.height = size;
            spec.rotation = particle.Rotation;
            spec.color[0] = r;
            spec.color[1] = g;
            spec.color[2] = b;
            spec.color[3] = a;
            Renderer2D::DrawQuad(spec);
        }

        Renderer2D::EndScene();
    }

}
