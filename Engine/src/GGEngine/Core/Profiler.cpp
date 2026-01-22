#include "ggpch.h"
#include "Profiler.h"

namespace GGEngine {

    std::vector<FrameProfileResult> Profiler::s_Results;

    void Profiler::BeginFrame()
    {
        s_Results.clear();
        s_Results.reserve(64);
    }

    void Profiler::SubmitResult(const FrameProfileResult& result)
    {
        s_Results.push_back(result);
    }

    const std::vector<FrameProfileResult>& Profiler::GetResults()
    {
        return s_Results;
    }

} // namespace GGEngine
