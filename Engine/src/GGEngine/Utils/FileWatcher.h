#pragma once

#include "GGEngine/Core/Core.h"

#include <filesystem>
#include <functional>
#include <string>
#include <vector>
#include <unordered_map>
#include <chrono>

#ifdef GG_PLATFORM_WINDOWS
    #include <Windows.h>
#endif

namespace GGEngine {

    // File change event types
    enum class FileChangeType : uint8_t
    {
        Modified = 0,
        Created,
        Deleted,
        Renamed
    };

    // Callback for file changes: (path, changeType)
    using FileChangedCallback = std::function<void(const std::filesystem::path& path, FileChangeType type)>;

    // Cross-platform file watcher for hot reload support.
    // Uses native APIs where available (Windows: ReadDirectoryChangesW),
    // falls back to polling on other platforms.
    class GG_API FileWatcher
    {
    public:
        FileWatcher();
        ~FileWatcher();

        // Start watching a directory (recursively)
        // Returns true if watch was successfully set up
        bool Watch(const std::filesystem::path& directory, FileChangedCallback callback);

        // Stop watching a directory
        void Unwatch(const std::filesystem::path& directory);

        // Stop watching all directories
        void UnwatchAll();

        // Poll for changes - call this from the main loop
        // Returns number of changes detected
        uint32_t Update();

        // Check if a directory is being watched
        bool IsWatching(const std::filesystem::path& directory) const;

        // Get number of watched directories
        size_t GetWatchCount() const;

        // Enable/disable the watcher entirely
        void SetEnabled(bool enabled) { m_Enabled = enabled; }
        bool IsEnabled() const { return m_Enabled; }

    private:
        bool m_Enabled = true;

#ifdef GG_PLATFORM_WINDOWS
        // Windows-specific implementation using ReadDirectoryChangesW
        struct WatchEntry
        {
            std::filesystem::path directory;
            HANDLE directoryHandle = INVALID_HANDLE_VALUE;
            OVERLAPPED overlapped = {};
            std::vector<uint8_t> buffer;
            FileChangedCallback callback;
            bool pending = false;
        };

        std::vector<std::unique_ptr<WatchEntry>> m_Watches;

        bool SetupWindowsWatch(WatchEntry& entry);
        void ProcessWindowsChanges(WatchEntry& entry);
        void CleanupWindowsWatch(WatchEntry& entry);

#else
        // Polling fallback for non-Windows platforms
        struct PolledFile
        {
            std::filesystem::path path;
            std::filesystem::file_time_type lastModified;
        };

        struct PolledDirectory
        {
            std::filesystem::path directory;
            FileChangedCallback callback;
            std::unordered_map<std::string, PolledFile> files;
            std::chrono::steady_clock::time_point lastScan;
        };

        std::vector<PolledDirectory> m_PolledDirectories;
        std::chrono::milliseconds m_PollInterval{500};  // Poll every 500ms

        void ScanDirectory(PolledDirectory& dir);
#endif
    };

}
