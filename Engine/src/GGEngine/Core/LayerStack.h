#pragma once

#include "Core.h"
#include "Layer.h"

#include <vector>

namespace GGEngine
{

    // LayerStack manages the lifetime of all pushed layers.
    //
    // Ownership: LayerStack takes ownership of all layers passed to PushLayer/PushOverlay.
    // Layers are deleted in the destructor. PopLayer/PopOverlay remove layers from the stack
    // but do NOT delete them - caller takes ownership of popped layers.
    //
    // Order: Layers are processed first-to-last for updates, last-to-first for events.
    // Overlays are always processed after regular layers.
    class GG_API LayerStack
    {
    public:
        LayerStack();
        ~LayerStack();

        // Push a layer - LayerStack takes ownership
        void PushLayer(Layer* layer);
        // Push an overlay - LayerStack takes ownership
        void PushOverlay(Layer* overlay);
        // Remove layer from stack - caller takes ownership of the returned layer
        void PopLayer(Layer* layer);
        // Remove overlay from stack - caller takes ownership of the returned layer
        void PopOverlay(Layer* overlay);

        std::vector<Layer*>::iterator begin() { return m_Layers.begin(); }
        std::vector<Layer*>::iterator end() { return m_Layers.end(); }

    private:
        std::vector<Layer*> m_Layers;
        unsigned int m_LayerInsertIndex = 0;
    };

}