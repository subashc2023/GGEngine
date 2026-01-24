#include "ggpch.h"
#include "FileWatcher.h"

namespace GGEngine {

    FileWatcher::FileWatcher()
    {
    }

    FileWatcher::~FileWatcher()
    {
        UnwatchAll();
    }

#ifdef GG_PLATFORM_WINDOWS
    // ========================================================================
    // Windows Implementation using ReadDirectoryChangesW
    // ========================================================================

    bool FileWatcher::Watch(const std::filesystem::path& directory, FileChangedCallback callback)
    {
        if (!std::filesystem::exists(directory) || !std::filesystem::is_directory(directory))
        {
            GG_CORE_ERROR("FileWatcher::Watch - directory does not exist: {}", directory.string());
            return false;
        }

        // Check if already watching
        for (const auto& watch : m_Watches)
        {
            if (watch->directory == directory)
            {
                GG_CORE_WARN("FileWatcher::Watch - already watching: {}", directory.string());
                return true;
            }
        }

        auto entry = std::make_unique<WatchEntry>();
        entry->directory = directory;
        entry->callback = std::move(callback);
        entry->buffer.resize(64 * 1024);  // 64KB buffer for notifications

        if (!SetupWindowsWatch(*entry))
        {
            return false;
        }

        m_Watches.push_back(std::move(entry));
        GG_CORE_INFO("FileWatcher: watching directory {}", directory.string());
        return true;
    }

    bool FileWatcher::SetupWindowsWatch(WatchEntry& entry)
    {
        // Open directory handle for async I/O
        entry.directoryHandle = CreateFileW(
            entry.directory.wstring().c_str(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            nullptr,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            nullptr
        );

        if (entry.directoryHandle == INVALID_HANDLE_VALUE)
        {
            GG_CORE_ERROR("FileWatcher: failed to open directory handle for {}", entry.directory.string());
            return false;
        }

        // Create event for overlapped I/O
        entry.overlapped.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (entry.overlapped.hEvent == nullptr)
        {
            CloseHandle(entry.directoryHandle);
            entry.directoryHandle = INVALID_HANDLE_VALUE;
            GG_CORE_ERROR("FileWatcher: failed to create event for {}", entry.directory.string());
            return false;
        }

        // Start async read
        BOOL result = ReadDirectoryChangesW(
            entry.directoryHandle,
            entry.buffer.data(),
            static_cast<DWORD>(entry.buffer.size()),
            TRUE,  // Watch subdirectories
            FILE_NOTIFY_CHANGE_FILE_NAME |
            FILE_NOTIFY_CHANGE_DIR_NAME |
            FILE_NOTIFY_CHANGE_LAST_WRITE |
            FILE_NOTIFY_CHANGE_SIZE,
            nullptr,
            &entry.overlapped,
            nullptr
        );

        if (!result && GetLastError() != ERROR_IO_PENDING)
        {
            CloseHandle(entry.overlapped.hEvent);
            CloseHandle(entry.directoryHandle);
            entry.directoryHandle = INVALID_HANDLE_VALUE;
            GG_CORE_ERROR("FileWatcher: ReadDirectoryChangesW failed for {}", entry.directory.string());
            return false;
        }

        entry.pending = true;
        return true;
    }

    void FileWatcher::Unwatch(const std::filesystem::path& directory)
    {
        for (auto it = m_Watches.begin(); it != m_Watches.end(); ++it)
        {
            if ((*it)->directory == directory)
            {
                CleanupWindowsWatch(**it);
                m_Watches.erase(it);
                GG_CORE_INFO("FileWatcher: stopped watching {}", directory.string());
                return;
            }
        }
    }

    void FileWatcher::UnwatchAll()
    {
        for (auto& watch : m_Watches)
        {
            CleanupWindowsWatch(*watch);
        }
        m_Watches.clear();
    }

    void FileWatcher::CleanupWindowsWatch(WatchEntry& entry)
    {
        if (entry.pending)
        {
            CancelIo(entry.directoryHandle);
            entry.pending = false;
        }

        if (entry.overlapped.hEvent)
        {
            CloseHandle(entry.overlapped.hEvent);
            entry.overlapped.hEvent = nullptr;
        }

        if (entry.directoryHandle != INVALID_HANDLE_VALUE)
        {
            CloseHandle(entry.directoryHandle);
            entry.directoryHandle = INVALID_HANDLE_VALUE;
        }
    }

    uint32_t FileWatcher::Update()
    {
        if (!m_Enabled)
            return 0;

        uint32_t changeCount = 0;

        for (auto& watch : m_Watches)
        {
            if (!watch->pending)
                continue;

            // Check if there are results available (non-blocking)
            DWORD bytesReturned = 0;
            BOOL result = GetOverlappedResult(
                watch->directoryHandle,
                &watch->overlapped,
                &bytesReturned,
                FALSE  // Don't wait
            );

            if (!result)
            {
                DWORD error = GetLastError();
                if (error == ERROR_IO_INCOMPLETE)
                {
                    // Still pending, no changes yet
                    continue;
                }
                else
                {
                    GG_CORE_WARN("FileWatcher: GetOverlappedResult failed for {}", watch->directory.string());
                    continue;
                }
            }

            // Process the changes
            if (bytesReturned > 0)
            {
                ProcessWindowsChanges(*watch);
                changeCount++;
            }

            // Reset event and start new async read
            ResetEvent(watch->overlapped.hEvent);

            result = ReadDirectoryChangesW(
                watch->directoryHandle,
                watch->buffer.data(),
                static_cast<DWORD>(watch->buffer.size()),
                TRUE,
                FILE_NOTIFY_CHANGE_FILE_NAME |
                FILE_NOTIFY_CHANGE_DIR_NAME |
                FILE_NOTIFY_CHANGE_LAST_WRITE |
                FILE_NOTIFY_CHANGE_SIZE,
                nullptr,
                &watch->overlapped,
                nullptr
            );

            watch->pending = (result || GetLastError() == ERROR_IO_PENDING);
        }

        return changeCount;
    }

    void FileWatcher::ProcessWindowsChanges(WatchEntry& entry)
    {
        uint8_t* ptr = entry.buffer.data();
        FILE_NOTIFY_INFORMATION* info = nullptr;

        do
        {
            info = reinterpret_cast<FILE_NOTIFY_INFORMATION*>(ptr);

            // Convert filename to path
            std::wstring fileName(info->FileName, info->FileNameLength / sizeof(WCHAR));
            std::filesystem::path fullPath = entry.directory / fileName;

            // Determine change type
            FileChangeType changeType;
            switch (info->Action)
            {
                case FILE_ACTION_ADDED:
                    changeType = FileChangeType::Created;
                    break;
                case FILE_ACTION_REMOVED:
                    changeType = FileChangeType::Deleted;
                    break;
                case FILE_ACTION_MODIFIED:
                    changeType = FileChangeType::Modified;
                    break;
                case FILE_ACTION_RENAMED_OLD_NAME:
                case FILE_ACTION_RENAMED_NEW_NAME:
                    changeType = FileChangeType::Renamed;
                    break;
                default:
                    changeType = FileChangeType::Modified;
                    break;
            }

            // Call the callback
            if (entry.callback)
            {
                entry.callback(fullPath, changeType);
            }

            // Move to next entry
            if (info->NextEntryOffset == 0)
                break;

            ptr += info->NextEntryOffset;
        }
        while (true);
    }

    bool FileWatcher::IsWatching(const std::filesystem::path& directory) const
    {
        for (const auto& watch : m_Watches)
        {
            if (watch->directory == directory)
                return true;
        }
        return false;
    }

    size_t FileWatcher::GetWatchCount() const
    {
        return m_Watches.size();
    }

#else
    // ========================================================================
    // Polling Fallback Implementation
    // ========================================================================

    bool FileWatcher::Watch(const std::filesystem::path& directory, FileChangedCallback callback)
    {
        if (!std::filesystem::exists(directory) || !std::filesystem::is_directory(directory))
        {
            GG_CORE_ERROR("FileWatcher::Watch - directory does not exist: {}", directory.string());
            return false;
        }

        // Check if already watching
        for (const auto& dir : m_PolledDirectories)
        {
            if (dir.directory == directory)
            {
                GG_CORE_WARN("FileWatcher::Watch - already watching: {}", directory.string());
                return true;
            }
        }

        PolledDirectory polled;
        polled.directory = directory;
        polled.callback = std::move(callback);
        polled.lastScan = std::chrono::steady_clock::now();

        // Initial scan
        ScanDirectory(polled);

        m_PolledDirectories.push_back(std::move(polled));
        GG_CORE_INFO("FileWatcher: watching directory {} (polling mode)", directory.string());
        return true;
    }

    void FileWatcher::Unwatch(const std::filesystem::path& directory)
    {
        for (auto it = m_PolledDirectories.begin(); it != m_PolledDirectories.end(); ++it)
        {
            if (it->directory == directory)
            {
                m_PolledDirectories.erase(it);
                GG_CORE_INFO("FileWatcher: stopped watching {}", directory.string());
                return;
            }
        }
    }

    void FileWatcher::UnwatchAll()
    {
        m_PolledDirectories.clear();
    }

    uint32_t FileWatcher::Update()
    {
        if (!m_Enabled)
            return 0;

        uint32_t changeCount = 0;
        auto now = std::chrono::steady_clock::now();

        for (auto& dir : m_PolledDirectories)
        {
            // Check if it's time to poll
            auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(now - dir.lastScan);
            if (elapsed < m_PollInterval)
                continue;

            dir.lastScan = now;

            // Scan for changes
            std::unordered_map<std::string, PolledFile> currentFiles;

            try
            {
                for (const auto& entry : std::filesystem::recursive_directory_iterator(dir.directory))
                {
                    if (!entry.is_regular_file())
                        continue;

                    PolledFile file;
                    file.path = entry.path();
                    file.lastModified = entry.last_write_time();
                    currentFiles[file.path.string()] = file;
                }
            }
            catch (const std::filesystem::filesystem_error& e)
            {
                GG_CORE_WARN("FileWatcher: error scanning directory: {}", e.what());
                continue;
            }

            // Check for new and modified files
            for (const auto& [path, file] : currentFiles)
            {
                auto it = dir.files.find(path);
                if (it == dir.files.end())
                {
                    // New file
                    if (dir.callback)
                        dir.callback(file.path, FileChangeType::Created);
                    changeCount++;
                }
                else if (file.lastModified != it->second.lastModified)
                {
                    // Modified file
                    if (dir.callback)
                        dir.callback(file.path, FileChangeType::Modified);
                    changeCount++;
                }
            }

            // Check for deleted files
            for (const auto& [path, file] : dir.files)
            {
                if (currentFiles.find(path) == currentFiles.end())
                {
                    // Deleted file
                    if (dir.callback)
                        dir.callback(file.path, FileChangeType::Deleted);
                    changeCount++;
                }
            }

            // Update file list
            dir.files = std::move(currentFiles);
        }

        return changeCount;
    }

    void FileWatcher::ScanDirectory(PolledDirectory& dir)
    {
        dir.files.clear();

        try
        {
            for (const auto& entry : std::filesystem::recursive_directory_iterator(dir.directory))
            {
                if (!entry.is_regular_file())
                    continue;

                PolledFile file;
                file.path = entry.path();
                file.lastModified = entry.last_write_time();
                dir.files[file.path.string()] = file;
            }
        }
        catch (const std::filesystem::filesystem_error& e)
        {
            GG_CORE_WARN("FileWatcher: error scanning directory: {}", e.what());
        }
    }

    bool FileWatcher::IsWatching(const std::filesystem::path& directory) const
    {
        for (const auto& dir : m_PolledDirectories)
        {
            if (dir.directory == directory)
                return true;
        }
        return false;
    }

    size_t FileWatcher::GetWatchCount() const
    {
        return m_PolledDirectories.size();
    }

#endif

}
