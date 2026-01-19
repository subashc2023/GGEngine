#include "GGEngine.h"
#include <iostream>

class Editor : public GGEngine::Application {
public:
    Editor() {
        std::cout << "Editor created" << std::endl;
    }
    ~Editor() {
        std::cout << "Editor destroyed" << std::endl;
    }
};

int main() {
    Editor* editor = new Editor();
    editor->Run();
    delete editor;
    return 0;
}