#include <gtest/gtest.h>
#include "GGEngine/Core/Log.h"

int main(int argc, char** argv)
{
    // Initialize engine logging for tests that use engine systems
    GGEngine::Log::Init();

    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
