#include "Application.h"
#include <iostream>

namespace GGEngine {
    Application::Application() {
        std::cout << "Application created" << std::endl;
    }
    Application::~Application() {
        std::cout << "Application destroyed" << std::endl;
    }

    void Application::Run() {
        std::cout << "Application running" << std::endl;
        while (true);
    }
}