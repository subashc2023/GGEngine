#include "ggpch.h"
#include "FileDialogs.h"
#include <tinyfiledialogs.h>

namespace GGEngine {

    std::string FileDialogs::OpenFile(const char* filter, const char* title)
    {
        const char* filterPatterns[] = { filter };
        const char* result = tinyfd_openFileDialog(
            title,
            "",
            1,
            filterPatterns,
            "Scene Files",
            0  // Single file select
        );

        if (result)
            return std::string(result);

        return std::string();
    }

    std::string FileDialogs::SaveFile(const char* filter, const char* title)
    {
        const char* filterPatterns[] = { filter };
        const char* result = tinyfd_saveFileDialog(
            title,
            "",
            1,
            filterPatterns,
            "Scene Files"
        );

        if (result)
            return std::string(result);

        return std::string();
    }

}
