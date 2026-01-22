#pragma once

// Component Registry - Single source of truth for ECS component types
// Using X-macros to avoid repetitive boilerplate when adding new components
//
// To add a new component:
// 1. Add it to GG_COMPONENT_LIST below (Type, StorageName)
// 2. Include the component header in Components.h
// 3. Add serialization trait specialization in ComponentTraits.h (if needed)

namespace GGEngine {

// Master list of all components: (ComponentType, StorageMemberName)
// This list generates:
// - Storage member declarations in Scene
// - GetStorage() template specializations
// - Iteration helpers for serialization/copying
#define GG_COMPONENT_LIST \
    GG_COMPONENT(TagComponent, m_Tags) \
    GG_COMPONENT(TransformComponent, m_Transforms) \
    GG_COMPONENT(SpriteRendererComponent, m_Sprites) \
    GG_COMPONENT(TilemapComponent, m_Tilemaps) \
    GG_COMPONENT(CameraComponent, m_Cameras)

// Macro to declare component storage members in Scene class
// Usage: GG_DECLARE_COMPONENT_STORAGES
#define GG_DECLARE_COMPONENT_STORAGES \
    GG_COMPONENT(TagComponent, m_Tags) \
    GG_COMPONENT(TransformComponent, m_Transforms) \
    GG_COMPONENT(SpriteRendererComponent, m_Sprites) \
    GG_COMPONENT(TilemapComponent, m_Tilemaps) \
    GG_COMPONENT(CameraComponent, m_Cameras)

// Helper macro: Generates storage member declaration
// ComponentStorage<Type> name;
#define GG_STORAGE_MEMBER(Type, Name) ComponentStorage<Type> Name;

// Helper macro: Generates mutable GetStorage specialization
#define GG_STORAGE_ACCESSOR(Type, Name) \
    template<> \
    inline ComponentStorage<Type>& Scene::GetStorage<Type>() { return Name; }

// Helper macro: Generates const GetStorage specialization
#define GG_STORAGE_ACCESSOR_CONST(Type, Name) \
    template<> \
    inline const ComponentStorage<Type>& Scene::GetStorage<Type>() const { return Name; }

} // namespace GGEngine
