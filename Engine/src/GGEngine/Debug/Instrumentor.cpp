#include "ggpch.h"
#include "Instrumentor.h"

namespace GGEngine {

    // Singleton instance - must be in .cpp file for proper DLL export
    Instrumentor& Instrumentor::Get()
    {
        static Instrumentor instance;
        return instance;
    }

    void Instrumentor::BeginSession(const std::string& name, const std::string& filepath)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        if (m_CurrentSession)
        {
            InternalEndSession();
        }
        m_OutputStream.open(filepath);
        if (m_OutputStream.is_open())
        {
            m_CurrentSession = new InstrumentationSession({ name });
            WriteHeader();
        }
    }

    void Instrumentor::EndSession()
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        InternalEndSession();
    }

    void Instrumentor::WriteProfile(const ProfileResult& result)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        if (!m_CurrentSession)
            return;

        std::stringstream json;

        std::string name = result.Name;
        std::replace(name.begin(), name.end(), '"', '\'');

        json << std::setprecision(3) << std::fixed;
        json << ",{";
        json << "\"cat\":\"function\",";
        json << "\"dur\":" << result.ElapsedTime.count() << ",";
        json << "\"name\":\"" << name << "\",";
        json << "\"ph\":\"X\",";
        json << "\"pid\":0,";
        json << "\"tid\":" << result.ThreadID << ",";
        json << "\"ts\":" << result.Start.count();
        json << "}";

        m_OutputStream << json.str();
        m_OutputStream.flush();
    }

    void Instrumentor::WriteHeader()
    {
        m_OutputStream << "{\"otherData\": {},\"traceEvents\":[{}";
        m_OutputStream.flush();
    }

    void Instrumentor::WriteFooter()
    {
        m_OutputStream << "]}";
        m_OutputStream.flush();
    }

    void Instrumentor::InternalEndSession()
    {
        if (m_CurrentSession)
        {
            WriteFooter();
            m_OutputStream.close();
            delete m_CurrentSession;
            m_CurrentSession = nullptr;
        }
    }

} // namespace GGEngine
