#include "GGEngine.h"
#include <iostream>

class Sandbox : public GGEngine::Application {
public:
    Sandbox() {
        std::cout << "Sandbox created" << std::endl;
    }
    ~Sandbox() {
        std::cout << "Sandbox destroyed" << std::endl;
    }
};

int main() {
    Sandbox* sandbox = new Sandbox();
    sandbox->Run();
    delete sandbox;
    return 0;
}