# Contributing to GGEngine

Thank you for your interest in contributing to GGEngine! This document provides guidelines for contributing to the project.

## Getting Started

1. Fork the repository
2. Clone your fork locally
3. Set up the build environment following the instructions in `README.md`
4. Create a new branch for your feature or fix

## Build Requirements

- CMake 3.20+
- LLVM/Clang
- Ninja
- Vulkan SDK
- MSYS2 MinGW-w64 (Windows)

## Code Style

- Use consistent indentation (4 spaces)
- Follow the existing naming conventions:
  - Classes: `PascalCase`
  - Functions/Methods: `PascalCase`
  - Variables: `camelCase`
  - Member variables: `m_PascalCase`
  - Static variables: `s_PascalCase`
- Keep lines under 120 characters when practical
- Use `#pragma once` for header guards

## Submitting Changes

1. Ensure your code compiles without warnings
2. Test your changes with both Sandbox and Editor applications
3. Write clear, descriptive commit messages
4. Push to your fork and submit a pull request

## Pull Request Guidelines

- Keep PRs focused on a single feature or fix
- Include a clear description of what the PR does
- Reference any related issues
- Be responsive to feedback and code review comments

## Reporting Issues

When reporting issues, please include:
- A clear description of the problem
- Steps to reproduce
- Expected vs actual behavior
- System information (OS, GPU, driver version)

## Questions?

Feel free to open an issue for questions or discussions about the codebase.
