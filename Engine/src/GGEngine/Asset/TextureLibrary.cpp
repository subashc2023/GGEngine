#include "ggpch.h"
#include "TextureLibrary.h"

#include <filesystem>

namespace GGEngine {

    TextureLibrary& TextureLibrary::Get()
    {
        static TextureLibrary instance;
        return instance;
    }

    void TextureLibrary::Init()
    {
        // Create built-in textures
        CreateBuiltInTextures();

        // Scan default textures directory
        ScanDirectory("assets/textures");
    }

    void TextureLibrary::CreateBuiltInTextures()
    {
        // 1. White texture (1x1)
        {
            uint8_t whitePixel[4] = { 255, 255, 255, 255 };
            auto whiteTex = Texture::Create(1, 1, whitePixel);
            m_BuiltInTextures["White"] = std::move(whiteTex);
            GG_CORE_TRACE("TextureLibrary: Created built-in 'White' texture");
        }

        // 2. Checkerboard texture (8x8 magenta/black)
        {
            const uint32_t size = 8;
            const uint32_t checkerSize = 2;
            std::vector<uint8_t> pixels(size * size * 4);

            for (uint32_t y = 0; y < size; y++)
            {
                for (uint32_t x = 0; x < size; x++)
                {
                    uint32_t index = (y * size + x) * 4;
                    bool isMagenta = ((x / checkerSize) + (y / checkerSize)) % 2 == 0;

                    if (isMagenta)
                    {
                        pixels[index + 0] = 255; // R
                        pixels[index + 1] = 0;   // G
                        pixels[index + 2] = 255; // B
                        pixels[index + 3] = 255; // A
                    }
                    else
                    {
                        pixels[index + 0] = 0;   // R
                        pixels[index + 1] = 0;   // G
                        pixels[index + 2] = 0;   // B
                        pixels[index + 3] = 255; // A
                    }
                }
            }

            auto checkerTex = Texture::Create(size, size, pixels.data());
            m_BuiltInTextures["Checkerboard"] = std::move(checkerTex);
            GG_CORE_TRACE("TextureLibrary: Created built-in 'Checkerboard' texture");
        }

        // 3. UV Test texture (gradient)
        {
            const uint32_t size = 64;
            std::vector<uint8_t> pixels(size * size * 4);

            for (uint32_t y = 0; y < size; y++)
            {
                for (uint32_t x = 0; x < size; x++)
                {
                    uint32_t index = (y * size + x) * 4;
                    pixels[index + 0] = static_cast<uint8_t>((x * 255) / (size - 1)); // R = U
                    pixels[index + 1] = static_cast<uint8_t>((y * 255) / (size - 1)); // G = V
                    pixels[index + 2] = 128; // B
                    pixels[index + 3] = 255; // A
                }
            }

            auto uvTex = Texture::Create(size, size, pixels.data());
            m_BuiltInTextures["UV_Test"] = std::move(uvTex);
            GG_CORE_TRACE("TextureLibrary: Created built-in 'UV_Test' texture");
        }
    }

    void TextureLibrary::Shutdown()
    {
        m_Textures.clear();
        m_BuiltInTextures.clear();
    }

    AssetHandle<Texture> TextureLibrary::Load(const std::string& name, const std::string& path)
    {
        if (Exists(name))
        {
            GG_CORE_WARN("TextureLibrary: Texture '{}' already exists, returning existing", name);
            return m_Textures[name];
        }

        auto texture = AssetManager::Get().Load<Texture>(path);
        if (texture.IsValid())
        {
            m_Textures[name] = texture;
            GG_CORE_TRACE("TextureLibrary: Loaded '{}' from '{}'", name, path);
        }
        return texture;
    }

    AssetHandle<Texture> TextureLibrary::Load(const std::string& path)
    {
        std::filesystem::path fsPath(path);
        std::string name = fsPath.stem().string();
        return Load(name, path);
    }

    void TextureLibrary::ScanDirectory(const std::string& directory)
    {
        std::filesystem::path dirPath = AssetManager::Get().ResolvePath(directory);

        if (!std::filesystem::exists(dirPath) || !std::filesystem::is_directory(dirPath))
        {
            GG_CORE_TRACE("TextureLibrary: Directory '{}' not found, skipping scan", directory);
            return;
        }

        GG_CORE_INFO("TextureLibrary: Scanning '{}'", dirPath.string());

        for (const auto& entry : std::filesystem::directory_iterator(dirPath))
        {
            if (!entry.is_regular_file())
                continue;

            std::string ext = entry.path().extension().string();
            // Convert to lowercase for comparison
            std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);

            // Check for supported image formats
            if (ext == ".png" || ext == ".jpg" || ext == ".jpeg" ||
                ext == ".bmp" || ext == ".tga")
            {
                std::string name = entry.path().stem().string();
                std::string relativePath = directory + "/" + entry.path().filename().string();

                if (!Exists(name))
                {
                    Load(name, relativePath);
                }
            }
        }
    }

    void TextureLibrary::Add(const std::string& name, AssetHandle<Texture> texture)
    {
        if (Exists(name))
        {
            GG_CORE_WARN("TextureLibrary: Overwriting texture '{}'", name);
        }
        m_Textures[name] = texture;
    }

    AssetHandle<Texture> TextureLibrary::Get(const std::string& name) const
    {
        auto it = m_Textures.find(name);
        if (it != m_Textures.end())
        {
            return it->second;
        }
        // Note: Built-in textures can't be returned as AssetHandle
        if (IsBuiltIn(name))
        {
            GG_CORE_WARN("TextureLibrary: '{}' is a built-in texture, use GetTexturePtr() instead", name);
        }
        return AssetHandle<Texture>();
    }

    Texture* TextureLibrary::GetTexturePtr(const std::string& name) const
    {
        // Check built-in textures first
        auto builtInIt = m_BuiltInTextures.find(name);
        if (builtInIt != m_BuiltInTextures.end())
        {
            return builtInIt->second.get();
        }

        // Check loaded textures
        auto it = m_Textures.find(name);
        if (it != m_Textures.end() && it->second.IsValid())
        {
            return it->second.Get();
        }

        return nullptr;
    }

    bool TextureLibrary::Exists(const std::string& name) const
    {
        return m_Textures.find(name) != m_Textures.end() ||
               m_BuiltInTextures.find(name) != m_BuiltInTextures.end();
    }

    bool TextureLibrary::IsBuiltIn(const std::string& name) const
    {
        return m_BuiltInTextures.find(name) != m_BuiltInTextures.end();
    }

    std::vector<std::string> TextureLibrary::GetAllNames() const
    {
        std::vector<std::string> names;
        names.reserve(m_Textures.size() + m_BuiltInTextures.size());

        // Add built-in textures first (with prefix for clarity)
        for (const auto& [name, _] : m_BuiltInTextures)
        {
            names.push_back(name);
        }

        // Add loaded textures
        for (const auto& [name, _] : m_Textures)
        {
            names.push_back(name);
        }

        std::sort(names.begin(), names.end());
        return names;
    }

}
