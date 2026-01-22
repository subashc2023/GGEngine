#include "ggpch.h"
#include "DebugUI.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/Core/Profiler.h"

#include <imgui.h>

namespace GGEngine {

    // Smoothed frame time for stable FPS display (EMA with ~20 frame response)
    static float s_SmoothedFrameTime = 0.0f;
    constexpr float EMA_ALPHA = 0.05f;

    void DebugUI::ShowStats(Timestep ts)
    {
        ImGui::Begin("Stats");
        ShowStatsContent(ts);
        ImGui::End();
    }

    void DebugUI::ShowStatsContent(Timestep ts)
    {
        float rawFrameTime = ts.GetSeconds();

        // Initialize on first frame, then apply EMA
        if (s_SmoothedFrameTime == 0.0f)
            s_SmoothedFrameTime = rawFrameTime;
        else
            s_SmoothedFrameTime = EMA_ALPHA * rawFrameTime + (1.0f - EMA_ALPHA) * s_SmoothedFrameTime;

        float smoothedFPS = (s_SmoothedFrameTime > 0.0f) ? 1.0f / s_SmoothedFrameTime : 0.0f;
        float smoothedMs = s_SmoothedFrameTime * 1000.0f;

        ImGui::Text("FPS: %.1f (%.2f ms)", smoothedFPS, smoothedMs);

        ImGui::Separator();

        auto& window = Application::Get().GetWindow();
        bool vsync = window.IsVSync();
        if (ImGui::Checkbox("VSync", &vsync))
        {
            window.SetVSync(vsync);
        }
    }

    void DebugUI::ShowProfiler()
    {
        ImGui::Begin("Profiler");
        ShowProfilerContent();
        ImGui::End();
    }

    void DebugUI::ShowProfilerContent()
    {
        const auto& results = Profiler::GetResults();

        ImGui::Text("Profile Results (%zu scopes)", results.size());
        ImGui::Separator();

        if (results.empty())
        {
            ImGui::TextDisabled("No profiling data");
            return;
        }

        // Find max time for bar scaling
        float maxTime = 0.001f;
        for (const auto& result : results)
        {
            if (result.DurationMs > maxTime)
                maxTime = result.DurationMs;
        }

        // Display each result
        for (const auto& result : results)
        {
            // Format: "0.123 ms  FunctionName"
            char label[256];
            snprintf(label, sizeof(label), "%.3f ms  %s", result.DurationMs, result.Name);

            float fraction = result.DurationMs / maxTime;
            ImGui::ProgressBar(fraction, ImVec2(-1, 0), label);
        }
    }

}
