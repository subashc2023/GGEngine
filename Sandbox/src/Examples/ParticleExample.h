#pragma once

#include "Example.h"
#include "GGEngine/ParticleSystem/ParticleSystem.h"

// Demonstrates the Particle System:
// - Emitting particles
// - Configuring particle properties
// - Different particle effects (fire, smoke, sparkles)
class ParticleExample : public Example
{
public:
    ParticleExample();

    void OnAttach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    void SetPreset(int preset);

    GGEngine::ParticleSystem m_ParticleSystem;
    GGEngine::ParticleProps m_ParticleProps;

    // Emitter settings
    float m_EmitterPosition[2] = { 0.0f, 0.0f };
    int m_EmitRate = 5;  // Particles per frame
    bool m_AutoEmit = true;
    int m_CurrentPreset = 0;
};
