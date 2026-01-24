#pragma once

namespace GGEngine {

    class Timestep
    {
    public:
        Timestep(float time = 0.0f, float alpha = 1.0f)
            : m_Time(time), m_Alpha(alpha)
        {
        }

        operator float() const { return m_Time; }

        float GetSeconds() const { return m_Time; }
        float GetMilliseconds() const { return m_Time * 1000.0f; }

        // Interpolation alpha for fixed timestep rendering
        // Range [0, 1] representing position between previous and current physics state
        // Use this to interpolate positions for smooth rendering:
        //   renderPos = lerp(previousPos, currentPos, alpha)
        float GetAlpha() const { return m_Alpha; }

    private:
        float m_Time;
        float m_Alpha;  // Interpolation factor for fixed timestep
    };

}
