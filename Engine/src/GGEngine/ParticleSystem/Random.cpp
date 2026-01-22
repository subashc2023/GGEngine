#include "ggpch.h"
#include "Random.h"

#include <random>

namespace GGEngine {

    static std::mt19937 s_RandomEngine;
    static std::uniform_real_distribution<float> s_Distribution(0.0f, 1.0f);

    void Random::Init()
    {
        s_RandomEngine.seed(std::random_device()());
    }

    float Random::Float()
    {
        return s_Distribution(s_RandomEngine);
    }

}
